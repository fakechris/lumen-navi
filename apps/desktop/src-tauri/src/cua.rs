//! Lumen Cua lifecycle and permission coordination for the desktop shell.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lumen_cua::{CuaClient, CuaPaths, CuaStatus};
use lumen_platform::PermissionState;

#[derive(Clone)]
pub struct CuaController {
    paths: CuaPaths,
}

impl CuaController {
    pub fn open() -> Result<Self> {
        let paths = CuaPaths::for_current_user();
        lumen_cua::ensure_token_file(&paths.token_file).context("initialize Lumen Cua token")?;
        Ok(Self { paths })
    }

    pub fn socket_path(&self) -> &Path {
        &self.paths.socket
    }

    pub fn token_file(&self) -> &Path {
        &self.paths.token_file
    }

    pub fn status(&self) -> Result<CuaStatus, String> {
        self.ensure_running()?.status().map_err(|e| e.to_string())
    }

    pub fn ensure_running(&self) -> Result<CuaClient, String> {
        lumen_cua::ensure_token_file(&self.paths.token_file).map_err(|e| e.to_string())?;
        let client = CuaClient::new(&self.paths.socket, &self.paths.token_file);
        let probe = client.clone().with_timeout(Duration::from_millis(500));
        if probe.status().is_ok() {
            return Ok(client);
        }

        #[cfg(target_os = "macos")]
        {
            let app = resolve_cua_app().ok_or_else(|| {
                "Lumen Cua.app was not found; run scripts/macos/prepare-cua-app.sh first"
                    .to_string()
            })?;
            let status = Command::new("open")
                .arg("-n")
                .arg("-g")
                .arg(&app)
                .arg("--args")
                .arg("serve")
                .status()
                .map_err(|e| format!("launch {}: {e}", app.display()))?;
            if !status.success() {
                return Err(format!("launch {} failed with {status}", app.display()));
            }
            wait_for_path_state(&self.paths.socket, true, Duration::from_secs(5));
            for _ in 0..20 {
                if probe.status().is_ok() {
                    return Ok(client);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err("Lumen Cua launched but its IPC endpoint did not become ready".into())
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Lumen Cua screen capture currently requires macOS".into())
        }
    }

    pub fn request_screen_permission(&self) -> Result<bool, String> {
        let client = self.ensure_running()?;
        let permission_client = client.clone().with_timeout(Duration::from_secs(300));
        let _ = permission_client
            .request_screen_permission()
            .map_err(|e| e.to_string())?;

        // TCC may expose the new grant only after the requesting process exits.
        // Restart the small capability app, never the Navi UI or daemon.
        let _ = client.shutdown();
        wait_for_path_state(&self.paths.socket, false, Duration::from_secs(2));
        let active = self
            .ensure_running()?
            .status()
            .map(|status| status.screen_recording == PermissionState::Granted)
            .unwrap_or(false);
        Ok(active)
    }
}

fn wait_for_path_state(path: &Path, exists: bool, timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() == exists {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "macos")]
fn resolve_cua_app() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(explicit) = std::env::var_os("LUMEN_CUA_APP") {
        candidates.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents) = macos_dir.parent() {
                candidates.push(contents.join("Resources/helpers/Lumen Cua.app"));
            }
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("helpers/Lumen Cua.app"));
    candidates.into_iter().find(|path| path.is_dir())
}
