//! Desktop-only shell state (onboarding, launch prefs) — not product navi.toml.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub onboarding_completed: bool,
    pub onboarding_skipped: bool,
    /// Step index 0..=3 for the wizard.
    pub onboarding_step: u32,
    /// Start Observe when the app launches (after onboarding).
    pub launch_observe: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            onboarding_completed: false,
            onboarding_skipped: false,
            onboarding_step: 0,
            launch_observe: false,
        }
    }
}

impl ShellConfig {
    pub fn needs_onboarding(&self) -> bool {
        !self.onboarding_completed && !self.onboarding_skipped
    }
}

pub fn shell_path(data_dir: &Path) -> PathBuf {
    data_dir.join("shell.toml")
}

pub fn load_shell(data_dir: &Path) -> Result<ShellConfig> {
    let path = shell_path(data_dir);
    if !path.exists() {
        let cfg = ShellConfig::default();
        save_shell(data_dir, &cfg)?;
        return Ok(cfg);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(toml::from_str(&raw)?)
}

pub fn save_shell(data_dir: &Path, cfg: &ShellConfig) -> Result<()> {
    let path = shell_path(data_dir);
    let raw = toml::to_string_pretty(cfg).context("serialize shell")?;
    std::fs::write(&path, raw).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}
