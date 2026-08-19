use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub filter: FilterConfig,
    #[serde(default)]
    pub capture: CaptureConfig,
    #[serde(default)]
    pub device: Vec<DeviceRule>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FilterConfig {
    #[serde(
        default = "keyboard_repress_gap_default",
        alias = "keyboard_debounce_ms"
    )]
    pub keyboard_repress_gap_ms: u64,
    #[serde(default = "keyboard_bounce_pulse_default")]
    pub keyboard_bounce_pulse_ms: u64,
    #[serde(default = "scroll_default")]
    pub scroll_to_middle_ms: u64,
    #[serde(default = "middle_default")]
    pub middle_to_middle_ms: u64,
    /// Drop every middle-button press and release. Useful when the switch is dead
    /// and middle-click / primary-paste is never needed.
    #[serde(default)]
    pub block_middle: bool,
    /// Only emit a middle press after it has been held this long (milliseconds).
    /// `0` disables the hold gate (legacy behavior). Accidental chatter is usually
    /// far shorter than an intentional click; try 80–120 ms.
    #[serde(default)]
    pub middle_min_hold_ms: u64,
}
fn keyboard_repress_gap_default() -> u64 {
    45
}
fn keyboard_bounce_pulse_default() -> u64 {
    12
}
fn scroll_default() -> u64 {
    250
}
fn middle_default() -> u64 {
    350
}
impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            keyboard_repress_gap_ms: keyboard_repress_gap_default(),
            keyboard_bounce_pulse_ms: keyboard_bounce_pulse_default(),
            scroll_to_middle_ms: scroll_default(),
            middle_to_middle_ms: middle_default(),
            block_middle: false,
            middle_min_hold_ms: 0,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct CaptureConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "capture_duration_default")]
    pub duration_seconds: u64,
    #[serde(default = "capture_report_path_default")]
    pub keyboard_timing_report_path: PathBuf,
}

fn capture_duration_default() -> u64 {
    300
}
fn capture_report_path_default() -> PathBuf {
    PathBuf::from("/var/lib/shakehands/keyboard-timing.json")
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            duration_seconds: capture_duration_default(),
            keyboard_timing_report_path: capture_report_path_default(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceRole {
    Keyboard,
    Mouse,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DeviceRule {
    pub role: DeviceRole,
    pub by_id_contains: Option<String>,
    pub name_contains: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let source =
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let config: Self =
            toml::from_str(&source).with_context(|| format!("parsing {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.device.is_empty() {
            bail!("config needs at least one [[device]] rule");
        }
        for rule in &self.device {
            if rule.by_id_contains.is_none()
                && rule.name_contains.is_none()
                && rule.vendor_id.is_none()
                && rule.product_id.is_none()
            {
                bail!("each device rule needs at least one matcher");
            }
            if rule.vendor_id.is_some() != rule.product_id.is_some() {
                bail!("vendor_id and product_id must be specified together");
            }
        }
        if self.filter.keyboard_repress_gap_ms == 0 || self.filter.keyboard_bounce_pulse_ms == 0 {
            bail!("keyboard timing thresholds must be greater than zero");
        }
        if self.capture.enabled && self.capture.duration_seconds == 0 {
            bail!("capture duration_seconds must be greater than zero when capture is enabled");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_keyboard_debounce_name_remains_supported() {
        let config: Config = toml::from_str(
            "[filter]\nkeyboard_debounce_ms = 45\n[[device]]\nrole = 'keyboard'\nname_contains = 'AK820'",
        ).unwrap();
        assert_eq!(config.filter.keyboard_repress_gap_ms, 45);
        assert_eq!(config.filter.keyboard_bounce_pulse_ms, 12);
    }

    #[test]
    fn middle_options_default_off_and_parse() {
        let bare: Config = toml::from_str(
            "[[device]]\nrole = 'mouse'\nname_contains = 'G502'",
        )
        .unwrap();
        assert!(!bare.filter.block_middle);
        assert_eq!(bare.filter.middle_min_hold_ms, 0);

        let set: Config = toml::from_str(
            "[filter]\nblock_middle = true\nmiddle_min_hold_ms = 100\n[[device]]\nrole = 'mouse'\nname_contains = 'G502'",
        )
        .unwrap();
        assert!(set.filter.block_middle);
        assert_eq!(set.filter.middle_min_hold_ms, 100);
    }
}
