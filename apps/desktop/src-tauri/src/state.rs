//! Shared app state: durable store + optional observe daemon child + shell prefs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use anyhow::{Context, Result};
use lumen_config::Config;
use lumen_store::SqliteStore;

use crate::cua::CuaController;
use crate::shell::{self, ShellConfig};

pub struct AppState {
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    pub cua: CuaController,
    pub store: SqliteStore,
    pub paused: Mutex<bool>,
    pub shell: Mutex<ShellConfig>,
    /// Child `lumen-daemon` when Observe is running from the shell.
    pub observe_child: Mutex<Option<Child>>,
    /// True while the user (or app shutdown) intentionally stopped Observe.
    /// The supervisor reads this to distinguish an intentional stop (don't
    /// restart) from a crash (alert + auto-restart). Set by observe_stop_inner,
    /// cleared by observe_start_inner.
    pub observe_stopping: AtomicBool,
    /// In-flight assistant (selection popup) streaming requests, by id.
    pub assistant_tasks:
        Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
}

impl AppState {
    pub fn open() -> Result<Self> {
        let data_dir = default_data_dir();
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("create data_dir {}", data_dir.display()))?;
        let _ = std::fs::create_dir_all(data_dir.join("logs"));
        let config_path = data_dir.join("navi.toml");
        // Cua (screen-capture helper) is optional — time tracking and most
        // features work without it. If init fails (dev mode bundle mismatch,
        // stale socket, protocol version skew), log and continue with a
        // best-effort controller so the app still starts.
        let cua = match CuaController::open() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "Lumen Cua init failed; screen capture disabled, time tracking still works");
                match CuaController::open_lenient() {
                    Ok(c) => c,
                    Err(e2) => {
                        tracing::error!(error = ?e2, "Lumen Cua lenient init also failed");
                        return Err(e).context("open Lumen Cua");
                    }
                }
            }
        };
        let config = load_or_write_config(&config_path, &data_dir)?;
        let shell_cfg = shell::load_shell(&data_dir)?;
        let store = SqliteStore::open(&config.data_dir)
            .with_context(|| format!("open store {}", config.data_dir.display()))?;
        Ok(Self {
            data_dir: config.data_dir.clone(),
            config_path,
            cua,
            store,
            paused: Mutex::new(config.privacy.paused),
            shell: Mutex::new(shell_cfg),
            observe_child: Mutex::new(None),
            observe_stopping: AtomicBool::new(false),
            assistant_tasks: Mutex::new(HashMap::new()),
        })
    }

    pub fn load_config(&self) -> Result<Config> {
        load_or_write_config(&self.config_path, &self.data_dir)
    }

    pub fn save_config(&self, cfg: &Config) -> Result<()> {
        let raw = toml::to_string_pretty(cfg).context("serialize config")?;
        std::fs::write(&self.config_path, raw)
            .with_context(|| format!("write {}", self.config_path.display()))?;
        Ok(())
    }

    pub fn save_shell(&self) -> Result<()> {
        let guard = self.shell.lock().map_err(|_| anyhow::anyhow!("shell lock"))?;
        shell::save_shell(&self.data_dir, &guard)
    }

    pub fn observe_running(&self) -> bool {
        let mut guard = self.observe_child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {
                    *guard = None;
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    *guard = None;
                    false
                }
            }
        } else {
            false
        }
    }

    pub fn stop_owned_observe(&self) {
        if let Ok(mut child) = self.observe_child.lock() {
            stop_observe_child(&mut child);
        }
    }
}

impl Drop for AppState {
    fn drop(&mut self) {
        if let Ok(child) = self.observe_child.get_mut() {
            stop_observe_child(child);
        }
    }
}

fn stop_observe_child(slot: &mut Option<Child>) {
    if let Some(child) = slot.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *slot = None;
}

fn default_data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(home).join("Library/Application Support/LumenNavi")
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(home).join(".lumen-navi")
    }
}

fn load_or_write_config(path: &Path, data_dir: &Path) -> Result<Config> {
    if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        let mut cfg: Config = toml::from_str(&raw)?;
        if cfg.data_dir.as_os_str().is_empty() || cfg.data_dir == PathBuf::from("data") {
            cfg.data_dir = data_dir.to_path_buf();
        }
        return Ok(cfg);
    }
    let mut cfg = Config::default();
    cfg.data_dir = data_dir.to_path_buf();
    cfg.api.enabled = true;
    cfg.api.bind = "127.0.0.1:7420".into();
    cfg.capture.screen_ticks = 0;
    cfg.audio.ticks = 0;
    let raw = toml::to_string_pretty(&cfg)?;
    std::fs::write(path, raw)?;
    Ok(cfg)
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn stopping_an_owned_observe_child_reaps_the_process() {
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let mut slot = Some(child);

        stop_observe_child(&mut slot);

        assert!(slot.is_none());
    }
}
