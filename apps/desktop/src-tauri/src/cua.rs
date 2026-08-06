//! Lumen Cua lifecycle and permission coordination for the desktop shell.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use lumen_cua::{CuaClient, CuaPaths, CuaStatus};
use lumen_platform::PermissionState;

#[derive(Clone)]
pub struct CuaController {
    paths: CuaPaths,
    app: PathBuf,
    lifecycle: Arc<Mutex<()>>,
}

impl CuaController {
    pub fn open() -> Result<Self> {
        let paths = CuaPaths::for_current_user();
        lumen_cua::ensure_token_file(&paths.token_file).context("initialize Lumen Cua token")?;
        let app = prepare_runtime_cua_app(&paths)?;
        Ok(Self {
            paths,
            app,
            lifecycle: Arc::new(Mutex::new(())),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.paths.socket
    }

    pub fn token_file(&self) -> &Path {
        &self.paths.token_file
    }

    pub fn status(&self) -> Result<CuaStatus, String> {
        let _guard = self
            .lifecycle
            .lock()
            .map_err(|_| "Lumen Cua lifecycle lock was poisoned".to_string())?;
        self.ensure_running_unlocked()?
            .status()
            .map_err(|e| e.to_string())
    }

    pub fn ensure_running(&self) -> Result<CuaClient, String> {
        let _guard = self
            .lifecycle
            .lock()
            .map_err(|_| "Lumen Cua lifecycle lock was poisoned".to_string())?;
        self.ensure_running_unlocked()
    }

    fn ensure_running_unlocked(&self) -> Result<CuaClient, String> {
        lumen_cua::ensure_token_file(&self.paths.token_file).map_err(|e| e.to_string())?;
        let client = CuaClient::new(&self.paths.socket, &self.paths.token_file);
        let probe = client.clone().with_timeout(Duration::from_millis(500));
        if probe.status().is_ok() {
            return Ok(client);
        }

        #[cfg(target_os = "macos")]
        {
            let status = Command::new("open")
                .arg("-n")
                .arg("-g")
                .arg(&self.app)
                .arg("--args")
                .arg("serve")
                .status()
                .map_err(|e| format!("launch {}: {e}", self.app.display()))?;
            if !status.success() {
                return Err(format!(
                    "launch {} failed with {status}",
                    self.app.display()
                ));
            }
            if !wait_for_path_state(&self.paths.socket, true, Duration::from_secs(5)) {
                return Err("Lumen Cua launched but its IPC endpoint did not appear".into());
            }
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
        let _guard = self
            .lifecycle
            .lock()
            .map_err(|_| "Lumen Cua lifecycle lock was poisoned".to_string())?;
        let client = self.ensure_running_unlocked()?;
        let permission_client = client.clone().with_timeout(Duration::from_secs(300));
        permission_client
            .request_screen_permission()
            .map_err(|e| e.to_string())?;

        // TCC may expose the new grant only after the requesting process exits.
        // Restart the small capability app, never the Navi UI or daemon.
        self.restart_and_read_permission(client)
    }

    /// Re-read a grant changed in System Settings. Screen Recording grants are
    /// process-scoped, so a status poll alone is insufficient.
    pub fn refresh_screen_permission(&self) -> Result<bool, String> {
        let _guard = self
            .lifecycle
            .lock()
            .map_err(|_| "Lumen Cua lifecycle lock was poisoned".to_string())?;
        let client = self.ensure_running_unlocked()?;
        self.restart_and_read_permission(client)
    }

    fn restart_and_read_permission(&self, client: CuaClient) -> Result<bool, String> {
        client
            .shutdown()
            .map_err(|error| format!("stop Lumen Cua before permission refresh: {error}"))?;
        if !wait_for_path_state(&self.paths.socket, false, Duration::from_secs(5)) {
            return Err("Lumen Cua acknowledged shutdown but its IPC endpoint remained".into());
        }
        let status = self
            .ensure_running_unlocked()?
            .status()
            .map_err(|e| e.to_string())?;
        Ok(status.screen_recording == PermissionState::Granted)
    }
}

fn wait_for_path_state(path: &Path, exists: bool, timeout: Duration) -> bool {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if path.exists() == exists {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    path.exists() == exists
}

#[cfg(target_os = "macos")]
fn prepare_runtime_cua_app(paths: &CuaPaths) -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("LUMEN_CUA_APP") {
        let explicit = PathBuf::from(explicit);
        validate_standalone_app(&explicit)?;
        return Ok(explicit);
    }

    let source = resolve_cua_payload_app().ok_or_else(|| {
        anyhow::anyhow!(
            "Lumen Cua payload was not found; run scripts/macos/prepare-cua-app.sh first"
        )
    })?;
    let target = paths.app.clone();
    validate_standalone_app(&target)?;
    if bundles_match(&source, &target) {
        return Ok(target);
    }

    if paths.socket.exists() {
        let client =
            CuaClient::new(&paths.socket, &paths.token_file).with_timeout(Duration::from_secs(2));
        client
            .shutdown()
            .map_err(|error| anyhow::anyhow!("stop existing Lumen Cua before update: {error}"))?;
        if !wait_for_path_state(&paths.socket, false, Duration::from_secs(5)) {
            bail!("existing Lumen Cua did not exit before update");
        }
    }
    install_app_bundle(&source, &target)?;
    Ok(target)
}

#[cfg(not(target_os = "macos"))]
fn prepare_runtime_cua_app(paths: &CuaPaths) -> Result<PathBuf> {
    Ok(paths.app.clone())
}

#[cfg(target_os = "macos")]
fn resolve_cua_payload_app() -> Option<PathBuf> {
    let mut candidates = Vec::new();
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

#[cfg(target_os = "macos")]
fn install_app_bundle(source: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .context("Lumen Cua install path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create Lumen Cua install directory {}", parent.display()))?;
    let staging = parent.join(format!(".Lumen Cua.installing-{}", std::process::id()));
    let backup = parent.join(format!(".Lumen Cua.previous-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("remove stale install {}", staging.display()))?;
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup)
            .with_context(|| format!("remove stale backup {}", backup.display()))?;
    }

    let copied = Command::new("/usr/bin/ditto")
        .arg(source)
        .arg(&staging)
        .status()
        .with_context(|| format!("copy Lumen Cua from {}", source.display()))?;
    if !copied.success() {
        bail!("copy Lumen Cua failed with {copied}");
    }
    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&staging)
        .status()
        .context("verify installed Lumen Cua signature")?;
    if !verified.success() {
        let _ = std::fs::remove_dir_all(&staging);
        bail!("copied Lumen Cua failed code-signature verification");
    }

    if target.exists() {
        std::fs::rename(target, &backup)
            .with_context(|| format!("move previous Lumen Cua to {}", backup.display()))?;
    }
    if let Err(error) = std::fs::rename(&staging, target) {
        if backup.exists() {
            let _ = std::fs::rename(&backup, target);
        }
        return Err(error).with_context(|| format!("activate Lumen Cua at {}", target.display()));
    }
    if backup.exists() {
        std::fs::remove_dir_all(&backup)
            .with_context(|| format!("remove previous Lumen Cua {}", backup.display()))?;
    }
    Ok(())
}

fn bundles_match(source: &Path, target: &Path) -> bool {
    ["Contents/Info.plist", "Contents/MacOS/lumen-cua"]
        .into_iter()
        .all(|relative| {
            let Ok(source_file) = std::fs::read(source.join(relative)) else {
                return false;
            };
            let Ok(target_file) = std::fs::read(target.join(relative)) else {
                return false;
            };
            source_file == target_file
        })
}

fn validate_standalone_app(path: &Path) -> Result<()> {
    if !path.is_dir() && std::env::var_os("LUMEN_CUA_APP").is_some() {
        bail!("Lumen Cua app does not exist: {}", path.display());
    }
    if is_nested_in_other_app(path) {
        bail!(
            "Lumen Cua must run outside another app bundle so macOS assigns its own permissions: {}",
            path.display()
        );
    }
    Ok(())
}

fn is_nested_in_other_app(path: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent.ancestors().any(|ancestor| {
            ancestor
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_permission_runtime_must_not_be_nested_in_another_app_bundle() {
        assert!(is_nested_in_other_app(Path::new(
            "/Applications/Lumen Navi.app/Contents/Resources/helpers/Lumen Cua.app"
        )));
        assert!(!is_nested_in_other_app(Path::new(
            "/Users/test/Library/Application Support/Lumen/Cua/Lumen Cua.app"
        )));
    }

    #[test]
    fn controller_clones_serialize_lifecycle_operations() {
        let paths = CuaPaths::under("/tmp/lumen-cua-controller-test");
        let controller = CuaController {
            app: paths.app.clone(),
            paths,
            lifecycle: Arc::new(Mutex::new(())),
        };
        let cloned = controller.clone();

        assert!(Arc::ptr_eq(&controller.lifecycle, &cloned.lifecycle));
    }
}
