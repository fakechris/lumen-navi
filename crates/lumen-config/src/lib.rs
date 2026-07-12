//! Daemon and intake configuration.
//!
//! Defaults are media-first: screen + audio enabled; browser off until later phases.

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
    pub retention: RetentionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcesConfig {
    /// Continuous / interval screenshots.
    pub screen: bool,
    /// Microphone (system audio is a later flag).
    pub audio: bool,
    /// Optional higher-cost video segments.
    pub video: bool,
    /// Chrome extension path — off until Phase B1.
    pub browser: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Screenshot interval in milliseconds when idle interval mode is used.
    pub screen_interval_ms: u64,
    /// Skip writing when pixel_hash matches within this window.
    pub screen_dedup_window_ms: u64,
    /// Longest edge for stored screenshots (0 = native resolution).
    #[serde(default = "default_max_edge")]
    pub screen_max_edge: u32,
    /// How many screen ticks the daemon captures before exit.
    /// `0` = run until Ctrl+C.
    #[serde(default = "default_screen_ticks")]
    pub screen_ticks: u64,
}

fn default_max_edge() -> u32 {
    1920
}

fn default_screen_ticks() -> u64 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Soft disk budget in megabytes (PolicyGate uses this later).
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
            capture: CaptureConfig {
                screen_interval_ms: 3_000,
                screen_dedup_window_ms: 5_000,
                screen_max_edge: default_max_edge(),
                screen_ticks: default_screen_ticks(),
            },
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
    fn defaults_are_media_first() {
        let c = Config::default();
        assert!(c.sources.screen);
        assert!(c.sources.audio);
        assert!(!c.sources.browser);
        assert!(!c.sources.video);
        assert_eq!(c.capture.screen_ticks, 3);
        assert_eq!(c.capture.screen_max_edge, 1920);
    }
}
