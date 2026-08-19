use std::collections::HashMap;
use std::time::Duration;

pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const BTN_MIDDLE: u16 = 0x112;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SuppressionReason {
    KeyboardBounce,
    ScrollThenMiddle,
    RepeatedMiddle,
    BlockedMiddle,
    MiddleTooShort,
}

/// How to handle the event currently being processed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Forward the current event unchanged.
    Forward,
    /// Drop the current event and log the reason.
    Suppress(SuppressionReason),
    /// Drop the current event and emit these events instead (empty = silent drop).
    Replace(Vec<RawEvent>),
}

/// Full filter result: optional events that must be emitted before the decision
/// on the current input (e.g. a deferred middle press that has matured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub leading: Vec<RawEvent>,
    pub decision: Decision,
}

impl Outcome {
    pub fn forward() -> Self {
        Self {
            leading: Vec::new(),
            decision: Decision::Forward,
        }
    }

    pub fn suppress(reason: SuppressionReason) -> Self {
        Self {
            leading: Vec::new(),
            decision: Decision::Suppress(reason),
        }
    }

    pub fn replace(events: Vec<RawEvent>) -> Self {
        Self {
            leading: Vec::new(),
            decision: Decision::Replace(events),
        }
    }

    pub fn drop_silent() -> Self {
        Self::replace(Vec::new())
    }

    fn with_leading(mut self, leading: Vec<RawEvent>) -> Self {
        if leading.is_empty() {
            return self;
        }
        self.leading = leading;
        self
    }
}

fn middle_press() -> RawEvent {
    RawEvent {
        kind: EV_KEY,
        code: BTN_MIDDLE,
        value: 1,
    }
}

fn middle_release() -> RawEvent {
    RawEvent {
        kind: EV_KEY,
        code: BTN_MIDDLE,
        value: 0,
    }
}

#[derive(Debug)]
pub struct KeyboardFilter {
    bounce_pulse: Duration,
    repress_gap: Duration,
    keys: HashMap<u16, KeyState>,
}

#[derive(Debug, Default)]
struct KeyState {
    press_started: Option<Duration>,
    last_hold: Option<Duration>,
    last_release: Option<Duration>,
    suppress_release: bool,
}

impl KeyboardFilter {
    pub fn new(bounce_pulse: Duration, repress_gap: Duration) -> Self {
        Self {
            bounce_pulse,
            repress_gap,
            keys: HashMap::new(),
        }
    }

    pub fn process(&mut self, event: RawEvent, now: Duration) -> Outcome {
        Outcome {
            leading: Vec::new(),
            decision: self.decide(event, now),
        }
    }

    fn decide(&mut self, event: RawEvent, now: Duration) -> Decision {
        if event.kind != EV_KEY {
            return Decision::Forward;
        }
        let state = self.keys.entry(event.code).or_default();
        match event.value {
            1 => {
                let is_bounce = state
                    .last_hold
                    .is_some_and(|hold| hold <= self.bounce_pulse)
                    && state
                        .last_release
                        .is_some_and(|release| now.saturating_sub(release) <= self.repress_gap);
                state.press_started = Some(now);
                if is_bounce {
                    state.suppress_release = true;
                    Decision::Suppress(SuppressionReason::KeyboardBounce)
                } else {
                    Decision::Forward
                }
            }
            0 => {
                if std::mem::take(&mut state.suppress_release) {
                    state.press_started = None;
                    Decision::Suppress(SuppressionReason::KeyboardBounce)
                } else {
                    if let Some(pressed_at) = state.press_started.take() {
                        state.last_hold = Some(now.saturating_sub(pressed_at));
                        state.last_release = Some(now);
                    }
                    Decision::Forward
                }
            }
            _ => Decision::Forward, // EV_KEY value 2: kernel typematic repeat
        }
    }
}

#[derive(Debug)]
pub struct MouseFilter {
    scroll_debounce: Duration,
    middle_debounce: Duration,
    min_hold: Duration,
    block_middle: bool,
    last_scroll: Option<Duration>,
    last_middle: Option<Duration>,
    /// Drop the next middle release (paired with a suppressed/blocked press).
    suppress_middle_release: bool,
    /// Physical middle press time while we wait for min_hold (press not yet emitted).
    pending_middle_at: Option<Duration>,
    /// Deferred press was already flushed; forward the matching release.
    middle_down_emitted: bool,
}

impl MouseFilter {
    pub fn new(
        scroll_debounce: Duration,
        middle_debounce: Duration,
        min_hold: Duration,
        block_middle: bool,
    ) -> Self {
        Self {
            scroll_debounce,
            middle_debounce,
            min_hold,
            block_middle,
            last_scroll: None,
            last_middle: None,
            suppress_middle_release: false,
            pending_middle_at: None,
            middle_down_emitted: false,
        }
    }

    /// Convenience constructor matching the historical two-window defaults.
    pub fn with_defaults(scroll_debounce: Duration, middle_debounce: Duration) -> Self {
        Self::new(scroll_debounce, middle_debounce, Duration::ZERO, false)
    }

    pub fn process(&mut self, event: RawEvent, now: Duration) -> Outcome {
        let leading = self.flush_due_middle(now);

        if event.kind == EV_REL
            && matches!(event.code, 0x06 | 0x08 | 0x0b | 0x0c)
            && event.value != 0
        {
            self.last_scroll = Some(now);
            return Outcome::forward().with_leading(leading);
        }

        if event.kind != EV_KEY || event.code != BTN_MIDDLE {
            return Outcome::forward().with_leading(leading);
        }

        let decision = match event.value {
            1 => self.on_middle_press(now),
            0 => self.on_middle_release(now),
            _ => Decision::Forward,
        };
        Outcome { leading, decision }
    }

    /// If a deferred middle press has been held long enough, emit it.
    fn flush_due_middle(&mut self, now: Duration) -> Vec<RawEvent> {
        if self.middle_down_emitted {
            return Vec::new();
        }
        let Some(pressed_at) = self.pending_middle_at else {
            return Vec::new();
        };
        if self.min_hold > Duration::ZERO && now.saturating_sub(pressed_at) < self.min_hold {
            return Vec::new();
        }
        // min_hold == 0 is handled on press (immediate forward); only flush when pending.
        if self.min_hold == Duration::ZERO {
            return Vec::new();
        }
        self.middle_down_emitted = true;
        vec![middle_press()]
    }

    fn on_middle_press(&mut self, now: Duration) -> Decision {
        if self.block_middle {
            self.last_middle = Some(now);
            self.suppress_middle_release = true;
            self.pending_middle_at = None;
            self.middle_down_emitted = false;
            return Decision::Suppress(SuppressionReason::BlockedMiddle);
        }

        let after_scroll = self
            .last_scroll
            .is_some_and(|time| now.saturating_sub(time) <= self.scroll_debounce);
        let repeated = self
            .last_middle
            .is_some_and(|time| now.saturating_sub(time) <= self.middle_debounce);
        self.last_middle = Some(now); // physical presses count even when rejected

        let reason = if after_scroll {
            Some(SuppressionReason::ScrollThenMiddle)
        } else if repeated {
            Some(SuppressionReason::RepeatedMiddle)
        } else {
            None
        };
        if let Some(reason) = reason {
            self.suppress_middle_release = true;
            self.pending_middle_at = None;
            self.middle_down_emitted = false;
            return Decision::Suppress(reason);
        }

        if self.min_hold == Duration::ZERO {
            self.pending_middle_at = None;
            self.middle_down_emitted = true;
            return Decision::Forward;
        }

        // Defer until min_hold elapses (flushed on later events or confirmed on release).
        self.pending_middle_at = Some(now);
        self.middle_down_emitted = false;
        Decision::Replace(Vec::new())
    }

    fn on_middle_release(&mut self, now: Duration) -> Decision {
        if std::mem::take(&mut self.suppress_middle_release) {
            self.pending_middle_at = None;
            self.middle_down_emitted = false;
            // Prefer BlockedMiddle when blocking; otherwise reuse RepeatedMiddle for
            // scroll/repeat release pairs (historical reason string).
            let reason = if self.block_middle {
                SuppressionReason::BlockedMiddle
            } else {
                SuppressionReason::RepeatedMiddle
            };
            return Decision::Suppress(reason);
        }

        if self.middle_down_emitted {
            // Press already went out (immediate or flushed); forward the release.
            self.pending_middle_at = None;
            self.middle_down_emitted = false;
            return Decision::Forward;
        }

        if let Some(pressed_at) = self.pending_middle_at.take() {
            if now.saturating_sub(pressed_at) >= self.min_hold {
                // Confirm on release: emit press then release together.
                return Decision::Replace(vec![middle_press(), middle_release()]);
            }
            return Decision::Suppress(SuppressionReason::MiddleTooShort);
        }

        Decision::Forward
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }
    fn key(code: u16, value: i32) -> RawEvent {
        RawEvent {
            kind: EV_KEY,
            code,
            value,
        }
    }
    fn scroll(code: u16, value: i32) -> RawEvent {
        RawEvent {
            kind: EV_REL,
            code,
            value,
        }
    }

    #[test]
    fn keyboard_drops_a_short_pulse_bounce_and_its_release() {
        let mut f = KeyboardFilter::new(at(12), at(45));
        assert_eq!(f.process(key(30, 1), at(0)).decision, Decision::Forward);
        assert_eq!(f.process(key(30, 0), at(5)).decision, Decision::Forward);
        assert_eq!(
            f.process(key(30, 1), at(15)).decision,
            Decision::Suppress(SuppressionReason::KeyboardBounce)
        );
        assert_eq!(
            f.process(key(30, 0), at(20)).decision,
            Decision::Suppress(SuppressionReason::KeyboardBounce)
        );
    }

    #[test]
    fn keyboard_keeps_intentional_fast_retaps_and_typematic_repeats() {
        let mut f = KeyboardFilter::new(at(12), at(45));
        assert_eq!(f.process(key(14, 1), at(0)).decision, Decision::Forward);
        assert_eq!(f.process(key(14, 0), at(25)).decision, Decision::Forward);
        assert_eq!(f.process(key(14, 1), at(35)).decision, Decision::Forward);
        assert_eq!(f.process(key(14, 2), at(36)).decision, Decision::Forward);
    }

    #[test]
    fn keyboard_keeps_chords_and_different_key_sequences() {
        let mut f = KeyboardFilter::new(at(12), at(45));
        assert_eq!(f.process(key(17, 1), at(0)).decision, Decision::Forward);
        assert_eq!(f.process(key(30, 1), at(1)).decision, Decision::Forward);
        assert_eq!(f.process(key(30, 0), at(2)).decision, Decision::Forward);
        assert_eq!(f.process(key(17, 0), at(3)).decision, Decision::Forward);
        assert_eq!(f.process(key(31, 1), at(4)).decision, Decision::Forward);
    }

    #[test]
    fn mouse_filters_scroll_and_repeat_and_keeps_later_click() {
        let mut f = MouseFilter::with_defaults(at(250), at(350));
        assert_eq!(
            f.process(scroll(0x08, 1), at(0)).decision,
            Decision::Forward
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(250)).decision,
            Decision::Suppress(SuppressionReason::ScrollThenMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(251)).decision,
            Decision::Suppress(SuppressionReason::RepeatedMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(600)).decision,
            Decision::Suppress(SuppressionReason::RepeatedMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(601)).decision,
            Decision::Suppress(SuppressionReason::RepeatedMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(951)).decision,
            Decision::Forward
        );
    }

    #[test]
    fn mouse_allows_click_outside_windows() {
        let mut f = MouseFilter::with_defaults(at(250), at(350));
        f.process(scroll(0x0b, 1), at(0));
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(251)).decision,
            Decision::Forward
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(252)).decision,
            Decision::Forward
        );
    }

    #[test]
    fn block_middle_drops_press_and_release() {
        let mut f = MouseFilter::new(at(250), at(350), at(0), true);
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(0)).decision,
            Decision::Suppress(SuppressionReason::BlockedMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(10)).decision,
            Decision::Suppress(SuppressionReason::BlockedMiddle)
        );
        // Other buttons still pass.
        assert_eq!(f.process(key(0x110, 1), at(20)).decision, Decision::Forward);
    }

    #[test]
    fn min_hold_drops_short_phantom_clicks() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), false);
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(0)).decision,
            Decision::Replace(vec![])
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(20)).decision,
            Decision::Suppress(SuppressionReason::MiddleTooShort)
        );
    }

    #[test]
    fn min_hold_confirms_on_release_when_held_long_enough() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), false);
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(0)).decision,
            Decision::Replace(vec![])
        );
        // Hold matured by release time: flush press as leading, then forward release.
        let outcome = f.process(key(BTN_MIDDLE, 0), at(100));
        assert_eq!(outcome.leading, vec![middle_press()]);
        assert_eq!(outcome.decision, Decision::Forward);
    }

    #[test]
    fn min_hold_just_under_threshold_on_release_is_still_short() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), false);
        f.process(key(BTN_MIDDLE, 1), at(0));
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(79)).decision,
            Decision::Suppress(SuppressionReason::MiddleTooShort)
        );
    }

    #[test]
    fn min_hold_flushes_press_before_later_motion() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), false);
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(0)).decision,
            Decision::Replace(vec![])
        );
        let move_event = RawEvent {
            kind: EV_REL,
            code: 0x00, // REL_X
            value: 3,
        };
        let outcome = f.process(move_event, at(100));
        assert_eq!(outcome.leading, vec![middle_press()]);
        assert_eq!(outcome.decision, Decision::Forward);
        // Release after the flushed press is forwarded normally.
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(150)).decision,
            Decision::Forward
        );
    }

    #[test]
    fn min_hold_does_not_flush_before_threshold() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), false);
        f.process(key(BTN_MIDDLE, 1), at(0));
        let move_event = RawEvent {
            kind: EV_REL,
            code: 0x00,
            value: 1,
        };
        let outcome = f.process(move_event, at(50));
        assert!(outcome.leading.is_empty());
        assert_eq!(outcome.decision, Decision::Forward);
        // Still short overall → suppress on release.
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(60)).decision,
            Decision::Suppress(SuppressionReason::MiddleTooShort)
        );
    }

    #[test]
    fn min_hold_still_respects_scroll_window() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), false);
        f.process(scroll(0x08, 1), at(0));
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(100)).decision,
            Decision::Suppress(SuppressionReason::ScrollThenMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(200)).decision,
            Decision::Suppress(SuppressionReason::RepeatedMiddle)
        );
    }

    #[test]
    fn block_middle_wins_over_min_hold() {
        let mut f = MouseFilter::new(at(250), at(350), at(80), true);
        assert_eq!(
            f.process(key(BTN_MIDDLE, 1), at(0)).decision,
            Decision::Suppress(SuppressionReason::BlockedMiddle)
        );
        assert_eq!(
            f.process(key(BTN_MIDDLE, 0), at(200)).decision,
            Decision::Suppress(SuppressionReason::BlockedMiddle)
        );
    }
}
