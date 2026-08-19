use crate::capture::KeyboardTimingCapture;
use crate::config::{CaptureConfig, Config, DeviceRole, DeviceRule};
use crate::filter::{
    Decision, KeyboardFilter, MouseFilter, Outcome, RawEvent, SuppressionReason,
};
use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, InputEvent};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    mpsc::{self},
};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub path: PathBuf,
    pub by_id: Vec<String>,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
}

pub fn discover() -> Result<Vec<DeviceInfo>> {
    let mut by_target: HashMap<PathBuf, Vec<String>> = HashMap::new();
    if let Ok(entries) = fs::read_dir("/dev/input/by-id") {
        for entry in entries.flatten() {
            if let Ok(target) = fs::canonicalize(entry.path()) {
                by_target
                    .entry(target)
                    .or_default()
                    .push(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    let mut devices = Vec::new();
    for entry in fs::read_dir("/dev/input")
        .context("reading /dev/input")?
        .flatten()
    {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("event") {
            continue;
        }
        let path = entry.path();
        let Ok(device) = Device::open(&path) else {
            continue;
        }; // devices can disappear during a scan
        let id = device.input_id();
        let canonical = fs::canonicalize(&path).unwrap_or(path.clone());
        devices.push(DeviceInfo {
            path,
            by_id: by_target.remove(&canonical).unwrap_or_default(),
            name: device.name().unwrap_or("<unnamed>").to_owned(),
            vendor_id: id.vendor(),
            product_id: id.product(),
        });
    }
    devices.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(devices)
}

pub fn matches(rule: &DeviceRule, device: &DeviceInfo) -> bool {
    rule.by_id_contains.as_ref().is_none_or(|needle| {
        device
            .by_id
            .iter()
            .any(|id| contains_ignore_case(id, needle))
    }) && rule
        .name_contains
        .as_ref()
        .is_none_or(|needle| contains_ignore_case(&device.name, needle))
        && rule
            .vendor_id
            .is_none_or(|vendor| vendor == device.vendor_id)
        && rule
            .product_id
            .is_none_or(|product| product == device.product_id)
}
fn contains_ignore_case(value: &str, needle: &str) -> bool {
    value.to_lowercase().contains(&needle.to_lowercase())
}

pub fn print_devices(config: Option<&Config>) -> Result<()> {
    for device in discover()? {
        let matched = config
            .and_then(|c| c.device.iter().find(|rule| matches(rule, &device)))
            .map(|r| format!(" matched as {:?}", r.role))
            .unwrap_or_default();
        println!(
            "{}: {} (vendor={:#06x}, product={:#06x}){}",
            device.path.display(),
            device.name,
            device.vendor_id,
            device.product_id,
            matched
        );
        for by_id in device.by_id {
            println!("  by-id: {by_id}");
        }
    }
    Ok(())
}

pub fn monitor(config: &Config) -> Result<()> {
    let candidates = matched_devices(config)?;
    if candidates.is_empty() {
        anyhow::bail!("no configured input devices found");
    }
    let mut workers = Vec::new();
    for (info, _) in candidates {
        println!(
            "Monitoring {} ({}). Ctrl-C to stop.",
            info.path.display(),
            info.name
        );
        workers.push(thread::spawn(move || -> Result<()> {
            let mut dev = Device::open(&info.path)?;
            loop {
                for event in dev.fetch_events()? {
                    println!(
                        "{}: {:?} code={} value={}",
                        info.name,
                        event.event_type(),
                        event.code(),
                        event.value()
                    );
                }
            }
        }));
    }
    for worker in workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("monitor worker panicked"))??;
    }
    Ok(())
}

pub fn service_status(config: &Config) -> Result<()> {
    let active = Command::new("systemctl")
        .args(["is-active", "--quiet", "shakehands.service"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    let matched = discover()?
        .into_iter()
        .filter(|device| config.device.iter().any(|rule| matches(rule, device)))
        .count();
    println!(
        "service: {}; configured devices present: {matched}",
        if active {
            "active"
        } else {
            "inactive or not installed"
        }
    );
    Ok(())
}

pub fn run_daemon(config: Config) -> Result<()> {
    let (done_tx, done_rx) = mpsc::channel();
    let mut active = HashSet::new();
    loop {
        while let Ok(path) = done_rx.try_recv() {
            active.remove(&path);
        }
        for (info, role) in matched_devices(&config)? {
            if active.insert(info.path.clone()) {
                let cfg = config.filter.clone();
                let capture = config.capture.clone();
                let tx = done_tx.clone();
                thread::spawn(move || {
                    if let Err(error) = run_device(info.clone(), role, cfg, capture) {
                        eprintln!("shakehands: {}: {error:#}", info.path.display());
                    }
                    let _ = tx.send(info.path);
                });
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn matched_devices(config: &Config) -> Result<Vec<(DeviceInfo, DeviceRole)>> {
    let mut results = Vec::new();
    for info in discover()? {
        if let Some(rule) = config.device.iter().find(|rule| matches(rule, &info)) {
            results.push((info, rule.role));
        }
    }
    Ok(results)
}

fn run_device(
    info: DeviceInfo,
    role: DeviceRole,
    config: crate::config::FilterConfig,
    capture_config: CaptureConfig,
) -> Result<()> {
    let mut source =
        Device::open(&info.path).with_context(|| format!("opening {}", info.path.display()))?;
    let mut target = virtual_device(&source, &format!("Shakehands {}", info.name))?;
    source
        .grab()
        .with_context(|| format!("grabbing {}", info.path.display()))?;
    eprintln!(
        "shakehands: filtering {} as {:?}",
        info.path.display(),
        role
    );
    let started = Instant::now();
    let capture = if role == DeviceRole::Keyboard {
        let duration = Duration::from_secs(capture_config.duration_seconds);
        KeyboardTimingCapture::from_config(&capture_config, &config)
            .map(|capture| start_capture_timer(capture, duration))
    } else {
        None
    };
    let mut filter = match role {
        DeviceRole::Keyboard => ActiveFilter::Keyboard(KeyboardFilter::new(
            Duration::from_millis(config.keyboard_bounce_pulse_ms),
            Duration::from_millis(config.keyboard_repress_gap_ms),
        )),
        DeviceRole::Mouse => ActiveFilter::Mouse(MouseFilter::new(
            Duration::from_millis(config.scroll_to_middle_ms),
            Duration::from_millis(config.middle_to_middle_ms),
            Duration::from_millis(config.middle_min_hold_ms),
            config.block_middle,
        )),
    };
    let mut logger = SuppressionLogger::default();
    let mut batch = Vec::new();
    loop {
        for event in source.fetch_events()? {
            if event.event_type() == EventType::SYNCHRONIZATION {
                if !batch.is_empty() {
                    target.emit(&batch)?;
                    batch.clear();
                }
                continue;
            }
            // MSC_SCAN and similar source metadata are not application input and are not
            // advertised by this virtual device. Key/relative events are sufficient here.
            if event.event_type() != EventType::KEY && event.event_type() != EventType::RELATIVE {
                continue;
            }
            let raw = RawEvent {
                kind: event.event_type().0,
                code: event.code(),
                value: event.value(),
            };
            let elapsed = started.elapsed();
            if let Some(capture) = &capture
                && let Ok(mut capture) = capture.lock()
            {
                capture.observe(raw, elapsed);
            }
            let outcome = filter.process(raw, elapsed);
            apply_outcome(&mut batch, &mut logger, &info, raw, outcome);
        }
    }
}

fn apply_outcome(
    batch: &mut Vec<InputEvent>,
    logger: &mut SuppressionLogger,
    info: &DeviceInfo,
    current: RawEvent,
    outcome: Outcome,
) {
    for raw in outcome.leading {
        batch.push(InputEvent::new(raw.kind, raw.code, raw.value));
    }
    match outcome.decision {
        Decision::Forward => {
            batch.push(InputEvent::new(current.kind, current.code, current.value));
        }
        Decision::Suppress(reason) => {
            logger.log(info, reason);
        }
        Decision::Replace(events) => {
            for raw in events {
                batch.push(InputEvent::new(raw.kind, raw.code, raw.value));
            }
        }
    }
}

fn start_capture_timer(
    capture: KeyboardTimingCapture,
    duration: Duration,
) -> Arc<Mutex<KeyboardTimingCapture>> {
    let capture = Arc::new(Mutex::new(capture));
    let completion_capture = Arc::clone(&capture);
    thread::spawn(move || {
        // Completion is driven by elapsed wall time, even when no later key event arrives.
        thread::sleep(duration);
        match completion_capture.lock() {
            Ok(mut capture) => match capture.finish_if_due(duration) {
                Ok(true) => eprintln!("shakehands: keyboard timing capture completed"),
                Ok(false) => {}
                Err(error) => eprintln!("shakehands: keyboard timing capture failed: {error:#}"),
            },
            Err(_) => eprintln!("shakehands: keyboard timing capture lock failed"),
        }
    });
    capture
}

fn virtual_device(source: &Device, name: &str) -> Result<VirtualDevice> {
    let mut builder = VirtualDevice::builder()?
        .name(name)
        .input_id(source.input_id());
    if let Some(keys) = source.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(axes) = source.supported_relative_axes() {
        builder = builder.with_relative_axes(axes)?;
    }
    Ok(builder.build()?)
}

enum ActiveFilter {
    Keyboard(KeyboardFilter),
    Mouse(MouseFilter),
}
impl ActiveFilter {
    fn process(&mut self, event: RawEvent, now: Duration) -> Outcome {
        match self {
            Self::Keyboard(filter) => filter.process(event, now),
            Self::Mouse(filter) => filter.process(event, now),
        }
    }
}

#[derive(Default)]
struct SuppressionLogger {
    last: HashMap<SuppressionReason, Instant>,
}
impl SuppressionLogger {
    fn log(&mut self, device: &DeviceInfo, reason: SuppressionReason) {
        let now = Instant::now();
        if self
            .last
            .get(&reason)
            .is_none_or(|then| now.duration_since(*then) >= Duration::from_secs(1))
        {
            eprintln!("shakehands: suppressed {reason:?} on {}", device.name);
            self.last.insert(reason, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DeviceRole;
    fn info() -> DeviceInfo {
        DeviceInfo {
            path: PathBuf::from("/dev/input/event0"),
            by_id: vec!["usb-AK820-event-kbd".into()],
            name: "AK820 Keyboard".into(),
            vendor_id: 0x1234,
            product_id: 0x5678,
        }
    }
    #[test]
    fn device_rule_requires_every_configured_matcher() {
        let mut rule = DeviceRule {
            role: DeviceRole::Keyboard,
            by_id_contains: Some("ak820".into()),
            name_contains: Some("keyboard".into()),
            vendor_id: Some(0x1234),
            product_id: Some(0x5678),
        };
        assert!(matches(&rule, &info()));
        rule.product_id = Some(1);
        assert!(!matches(&rule, &info()));
    }
}
