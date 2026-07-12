//! Daemon and intake configuration — media-first Observe defaults.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub data_dir: PathBuf,
    pub sources: SourcesConfig,
    pub capture: CaptureConfig,
    pub privacy: PrivacyConfig,
    pub retention: RetentionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcesConfig {
    pub screen: bool,
    pub audio: bool,
    pub video: bool,
    pub browser: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureConfig {
    pub screen_interval_ms: u64,
    /// Legacy hash window (secondary). Primary dedup is grayscale probe.
    pub screen_dedup_window_ms: u64,
    pub screen_max_edge: u32,
    /// 0 = until Ctrl+C.
    pub screen_ticks: u64,
    /// Divisor for visual probe resolution (Yansu uses 6).
    pub probe_scale: u32,
    /// Mean abs gray distance threshold in [0,1] (Yansu 0.05).
    pub visual_change_threshold: f64,
    pub debounce_default_ms: u64,
    pub debounce_churn_ms: u64,
    pub same_app_min_ms: u64,
    pub idle_session_ms: u64,
    pub queue_capacity: usize,
    pub focus_poll_ms: u64,
    /// `all` | `main`
    pub displays: String,
    /// `jpeg` | `png`
    pub encode: String,
    pub jpeg_quality: u8,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            screen_interval_ms: 3_000,
            screen_dedup_window_ms: 5_000,
            screen_max_edge: 1920,
            screen_ticks: 0,
            probe_scale: 6,
            visual_change_threshold: 0.05,
            debounce_default_ms: 1_000,
            debounce_churn_ms: 3_000,
            same_app_min_ms: 10_000,
            idle_session_ms: 300_000,
            queue_capacity: 8,
            focus_poll_ms: 500,
            displays: "all".into(),
            encode: "jpeg".into(),
            jpeg_quality: 75,
        }
    }
}

impl CaptureConfig {
    pub fn all_displays(&self) -> bool {
        !self.displays.eq_ignore_ascii_case("main")
    }

    pub fn use_jpeg(&self) -> bool {
        self.encode.eq_ignore_ascii_case("jpeg")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub paused: bool,
    /// Product privacy mode: never capture screen pixels.
    pub closed_eyes: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            paused: false,
            closed_eyes: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    pub max_blob_mb: u64,
    pub wipe_on_request: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            sources: SourcesConfig {
                screen: true,
                audio: true,
                video: false,
                browser: false,
            },
            capture: CaptureConfig::default(),
            privacy: PrivacyConfig::default(),
            retention: RetentionConfig {
                max_blob_mb: 20_480,
                wipe_on_request: true,
            },
        }
    }
}

impl Config {
    pub fn load_or_default(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_product_observe() {
        let c = Config::default();
        assert!(c.sources.screen);
        assert!(!c.privacy.closed_eyes);
        assert_eq!(c.capture.probe_scale, 6);
        assert!((c.capture.visual_change_threshold - 0.05).abs() < 1e-9);
        assert!(c.capture.all_displays());
        assert!(c.capture.use_jpeg());
    }
}
