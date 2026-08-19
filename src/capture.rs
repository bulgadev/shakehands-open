use crate::config::{CaptureConfig, FilterConfig};
use crate::filter::{EV_KEY, RawEvent};
use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// Aggregates timing characteristics without retaining a key-event timeline.
pub struct KeyboardTimingCapture {
    duration: Duration,
    report_path: PathBuf,
    bounce_pulse: Duration,
    repress_gap: Duration,
    states: HashMap<u16, KeyState>,
    report: TimingReport,
    completed: bool,
}

#[derive(Default)]
struct KeyState {
    press_started: Option<Duration>,
    last_hold: Option<Duration>,
    last_release: Option<Duration>,
}

#[derive(Debug, Serialize)]
pub struct TimingReport {
    schema_version: u8,
    capture_duration_ms: u64,
    completed_after_ms: u64,
    keyboard_bounce_pulse_ms: u64,
    keyboard_repress_gap_ms: u64,
    keys: BTreeMap<u16, KeyTimingStats>,
}

#[derive(Debug, Default, Serialize)]
pub struct KeyTimingStats {
    completed_presses: u64,
    repress_sequences: u64,
    pulse_gate_candidates: u64,
    hold_duration_ms_histogram: BTreeMap<u64, u64>,
    repress_gap_ms_histogram: BTreeMap<u64, u64>,
}

impl KeyboardTimingCapture {
    pub fn from_config(config: &CaptureConfig, filter: &FilterConfig) -> Option<Self> {
        config.enabled.then(|| Self {
            duration: Duration::from_secs(config.duration_seconds),
            report_path: config.keyboard_timing_report_path.clone(),
            bounce_pulse: Duration::from_millis(filter.keyboard_bounce_pulse_ms),
            repress_gap: Duration::from_millis(filter.keyboard_repress_gap_ms),
            states: HashMap::new(),
            report: TimingReport {
                schema_version: 1,
                capture_duration_ms: config.duration_seconds.saturating_mul(1_000),
                completed_after_ms: 0,
                keyboard_bounce_pulse_ms: filter.keyboard_bounce_pulse_ms,
                keyboard_repress_gap_ms: filter.keyboard_repress_gap_ms,
                keys: BTreeMap::new(),
            },
            completed: false,
        })
    }

    pub fn observe(&mut self, event: RawEvent, now: Duration) {
        if self.completed || now >= self.duration || event.kind != EV_KEY {
            return;
        }
        let state = self.states.entry(event.code).or_default();
        match event.value {
            1 => {
                if let (Some(hold), Some(release)) = (state.last_hold, state.last_release) {
                    let gap = now.saturating_sub(release);
                    let stats = self.report.keys.entry(event.code).or_default();
                    stats.repress_sequences += 1;
                    add_bucket(&mut stats.repress_gap_ms_histogram, gap);
                    if hold <= self.bounce_pulse && gap <= self.repress_gap {
                        stats.pulse_gate_candidates += 1;
                    }
                }
                state.press_started = Some(now);
            }
            0 => {
                if let Some(pressed_at) = state.press_started.take() {
                    let hold = now.saturating_sub(pressed_at);
                    state.last_hold = Some(hold);
                    state.last_release = Some(now);
                    let stats = self.report.keys.entry(event.code).or_default();
                    stats.completed_presses += 1;
                    add_bucket(&mut stats.hold_duration_ms_histogram, hold);
                }
            }
            _ => {}
        }
    }

    pub fn finish_if_due(&mut self, now: Duration) -> Result<bool> {
        if self.completed || now < self.duration {
            return Ok(false);
        }
        self.completed = true;
        self.report.completed_after_ms = duration_ms(now);
        let json = serde_json::to_vec_pretty(&self.report)
            .context("serializing keyboard timing report")?;
        let parent = self
            .report_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        let temporary_path = self.report_path.with_file_name(format!(
            ".{}.tmp",
            self.report_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        fs::write(&temporary_path, json)
            .with_context(|| format!("writing {}", temporary_path.display()))?;
        fs::rename(&temporary_path, &self.report_path)
            .with_context(|| format!("publishing {}", self.report_path.display()))?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn report(&self) -> &TimingReport {
        &self.report
    }
}

fn add_bucket(histogram: &mut BTreeMap<u64, u64>, duration: Duration) {
    *histogram.entry(duration_ms(duration)).or_default() += 1;
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CaptureConfig, FilterConfig};
    use crate::filter::RawEvent;

    fn key(code: u16, value: i32) -> RawEvent {
        RawEvent {
            kind: EV_KEY,
            code,
            value,
        }
    }
    fn at(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn capture_aggregates_pulses_without_storing_event_history() {
        let config = CaptureConfig {
            enabled: true,
            duration_seconds: 300,
            keyboard_timing_report_path: PathBuf::from("/tmp/unused.json"),
        };
        let filter = FilterConfig::default();
        let mut capture = KeyboardTimingCapture::from_config(&config, &filter).unwrap();
        capture.observe(key(14, 1), at(0));
        capture.observe(key(14, 0), at(5));
        capture.observe(key(14, 1), at(15));
        let stats = &capture.report().keys[&14];
        assert_eq!(stats.completed_presses, 1);
        assert_eq!(stats.repress_sequences, 1);
        assert_eq!(stats.pulse_gate_candidates, 1);
        assert_eq!(stats.hold_duration_ms_histogram[&5], 1);
        assert_eq!(stats.repress_gap_ms_histogram[&10], 1);
    }

    #[test]
    fn capture_is_not_created_when_disabled() {
        assert!(
            KeyboardTimingCapture::from_config(&CaptureConfig::default(), &FilterConfig::default())
                .is_none()
        );
    }

    #[test]
    fn capture_writes_an_aggregate_json_report_when_due() {
        let directory = std::env::temp_dir().join(format!(
            "shakehands-capture-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let report_path = directory.join("keyboard.json");
        let config = CaptureConfig {
            enabled: true,
            duration_seconds: 1,
            keyboard_timing_report_path: report_path.clone(),
        };
        let mut capture =
            KeyboardTimingCapture::from_config(&config, &FilterConfig::default()).unwrap();
        capture.observe(key(14, 1), at(0));
        capture.observe(key(14, 0), at(5));
        assert!(capture.finish_if_due(at(1_000)).unwrap());
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
        assert_eq!(report["schema_version"], 1);
        assert_eq!(report["keys"]["14"]["completed_presses"], 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
