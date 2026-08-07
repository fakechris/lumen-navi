//! Lumen Cua lifecycle and permission coordination for the desktop shell.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use lumen_cua::{CuaClient, CuaPaths, CuaStatus, DirectCaptureStatus, PERMISSION_HOST_ARG};
use lumen_platform::PermissionState;
use uuid::Uuid;

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
        let status = self.request_screen_permission_via_launch_services()?;
        if !permission_setup_is_ready(&status) {
            return Ok(false);
        }

        // TCC may expose the new grant only after the requesting process exits.
        // Restart the small capability app, never the Navi UI or data daemon,
        // and require a real frame before reporting success.
        self.restart_and_verify_capture(client)
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

    fn request_screen_permission_via_launch_services(&self) -> Result<CuaStatus, String> {
        #[cfg(target_os = "macos")]
        {
            use std::io::{Read, Seek};

            let mut result = create_permission_result_file().map_err(|error| error.to_string())?;
            let status = Command::new("/usr/bin/open")
                .arg("-n")
                .arg("-W")
                .arg("-g")
                .arg(&self.app)
                .arg("--args")
                .arg(PERMISSION_HOST_ARG)
                .arg("--result-file")
                .arg(&result.path)
                .arg("--result-device")
                .arg(result.device.to_string())
                .arg("--result-inode")
                .arg(result.inode.to_string())
                .status()
                .map_err(|error| {
                    let _ = std::fs::remove_file(&result.path);
                    format!("launch Lumen Cua permission host: {error}")
                })?;
            if !status.success() {
                let _ = std::fs::remove_file(&result.path);
                return Err(format!(
                    "Lumen Cua permission host failed to launch with {status}"
                ));
            }
            result
                .file
                .rewind()
                .map_err(|error| format!("rewind Lumen Cua permission result: {error}"))?;
            let mut payload = Vec::new();
            let read = result
                .file
                .read_to_end(&mut payload)
                .map_err(|error| format!("read Lumen Cua permission result: {error}"));
            let _ = std::fs::remove_file(&result.path);
            read?;
            serde_json::from_slice(&payload)
                .map_err(|error| format!("decode Lumen Cua permission result: {error}"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Lumen Cua screen permission setup currently requires macOS".into())
        }
    }

    fn restart_and_verify_capture(&self, client: CuaClient) -> Result<bool, String> {
        client
            .shutdown()
            .map_err(|error| format!("stop Lumen Cua before permission refresh: {error}"))?;
        if !wait_for_path_state(&self.paths.socket, false, Duration::from_secs(5)) {
            return Err("Lumen Cua acknowledged shutdown but its IPC endpoint remained".into());
        }
        let client = self.ensure_running_unlocked()?;
        let status = client.status().map_err(|e| e.to_string())?;
        if status.screen_recording != PermissionState::Granted {
            return Ok(false);
        }
        let displays = client
            .list_displays()
            .map_err(|error| format!("verify Lumen Cua display access: {error}"))?;
        let display = displays
            .iter()
            .find(|display| display.is_main)
            .or_else(|| displays.first())
            .ok_or_else(|| "Lumen Cua returned no displays during verification".to_string())?;
        let frame = client
            .with_timeout(Duration::from_secs(30))
            .capture_encoded(display.id, 320, true, 70)
            .map_err(|error| format!("verify Lumen Cua screen capture: {error}"))?;
        Ok(frame.width > 0 && frame.height > 0 && !frame.png_or_jpeg_bytes.is_empty())
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
            .map_err(|error| error.to_string())?;
        Ok(status.screen_recording == PermissionState::Granted)
    }
}

fn permission_setup_is_ready(status: &CuaStatus) -> bool {
    status.screen_recording == PermissionState::Granted
        && status.screen_recording_capturable == Some(true)
        && status.direct_capture_status == DirectCaptureStatus::Ready
}

#[cfg(target_os = "macos")]
struct PermissionResultFile {
    path: PathBuf,
    file: std::fs::File,
    device: u64,
    inode: u64,
}

#[cfg(target_os = "macos")]
impl Drop for PermissionResultFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(target_os = "macos")]
fn create_permission_result_file() -> Result<PermissionResultFile> {
    use std::fs::OpenOptions;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let path = std::env::temp_dir().join(format!(
        "lumen-cua-permission-{}-{}.json",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .with_context(|| format!("create Lumen Cua permission result {}", path.display()))?;
    let metadata = file
        .metadata()
        .context("inspect newly-created Lumen Cua permission result")?;
    Ok(PermissionResultFile {
        path,
        file,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
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

fn activate_staged_bundle<F>(staging: &Path, target: &Path, verify: F) -> Result<()>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let parent = target
        .parent()
        .context("Lumen Cua target has no parent directory")?;
    let backup = parent.join(format!(".Lumen Cua.previous-{}", std::process::id()));
    if backup.exists() {
        std::fs::remove_dir_all(&backup)
            .with_context(|| format!("remove stale Lumen Cua backup {}", backup.display()))?;
    }
    let had_previous = target.exists();
    if had_previous {
        std::fs::rename(target, &backup)
            .with_context(|| format!("move previous Lumen Cua to {}", backup.display()))?;
    }
    if let Err(error) = std::fs::rename(staging, target) {
        if had_previous && backup.exists() {
            let _ = std::fs::rename(&backup, target);
        }
        return Err(error).with_context(|| format!("activate Lumen Cua at {}", target.display()));
    }

    if let Err(error) = verify(target) {
        let _ = std::fs::remove_dir_all(target);
        if had_previous && backup.exists() {
            std::fs::rename(&backup, target)
                .with_context(|| format!("restore previous Lumen Cua from {}", backup.display()))?;
        }
        return Err(error);
    }

    if backup.exists() {
        std::fs::remove_dir_all(&backup)
            .with_context(|| format!("remove previous Lumen Cua {}", backup.display()))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn prepare_runtime_cua_app(paths: &CuaPaths) -> Result<PathBuf> {
    let expected_certificate = current_app_certificate_constraint()?;
    if let Some(explicit) = std::env::var_os("LUMEN_CUA_APP") {
        let explicit = PathBuf::from(explicit);
        let requirement = validate_cua_bundle(&explicit, true)?;
        require_certificate_constraint(&requirement, &expected_certificate)?;
        register_launch_services(&explicit)?;
        return Ok(explicit);
    }

    let source = resolve_cua_payload_app().ok_or_else(|| {
        anyhow::anyhow!(
            "Lumen Cua payload was not found; run scripts/macos/prepare-cua-app.sh first"
        )
    })?;
    let target = paths.app.clone();
    let source_requirement = validate_cua_bundle(&source, false)?;
    require_certificate_constraint(&source_requirement, &expected_certificate)?;
    if bundles_match(&source, &target) {
        if let Ok(target_requirement) = validate_cua_bundle(&target, true) {
            if target_requirement == source_requirement {
                register_launch_services(&target)?;
                return Ok(target);
            }
        }
        tracing::warn!(path = %target.display(), "repairing invalid Lumen Cua installation");
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
    install_app_bundle(
        &source,
        &target,
        paths,
        &source_requirement,
        &expected_certificate,
    )?;
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
fn install_app_bundle(
    source: &Path,
    target: &Path,
    paths: &CuaPaths,
    source_requirement: &str,
    expected_certificate: &str,
) -> Result<()> {
    let parent = target
        .parent()
        .context("Lumen Cua install path has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create Lumen Cua install directory {}", parent.display()))?;
    let staging = parent.join(format!(".Lumen Cua.installing-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("remove stale install {}", staging.display()))?;
    }
    let copied = Command::new("/usr/bin/ditto")
        .arg(source)
        .arg(&staging)
        .status()
        .with_context(|| format!("copy Lumen Cua from {}", source.display()))?;
    if !copied.success() {
        bail!("copy Lumen Cua failed with {copied}");
    }
    let staged_requirement = match validate_cua_bundle(&staging, true) {
        Ok(requirement) => requirement,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error).context("validate staged Lumen Cua");
        }
    };
    if let Err(error) = require_certificate_constraint(&staged_requirement, expected_certificate) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error).context("validate staged Lumen Cua certificate");
    }
    if staged_requirement != source_requirement {
        let _ = std::fs::remove_dir_all(&staging);
        bail!("staged Lumen Cua changed its designated requirement during copy");
    }

    let activation = activate_staged_bundle(&staging, target, |installed| {
        let installed_requirement = validate_cua_bundle(installed, true)?;
        require_certificate_constraint(&installed_requirement, expected_certificate)?;
        if installed_requirement != source_requirement {
            bail!("installed Lumen Cua does not preserve its designated requirement");
        }
        register_launch_services(installed)?;
        verify_installed_runtime(paths, installed)
    });
    if let Err(error) = activation {
        if !target.exists() {
            return Err(error);
        }
        // A failed verification restores the previous bundle. Register that
        // restored path and verify its process before returning so neither
        // LaunchServices nor the live daemon retains the rejected generation.
        register_launch_services(target).context("register restored Lumen Cua")?;
        verify_installed_runtime(paths, target).context("verify restored Lumen Cua")?;
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_cua_bundle(path: &Path, require_standalone: bool) -> Result<String> {
    if require_standalone {
        validate_standalone_app(path)?;
    }
    if !path.is_dir() {
        bail!("Lumen Cua app does not exist: {}", path.display());
    }
    let plist = path.join("Contents/Info.plist");
    let bundle_id = plist_value(&plist, "CFBundleIdentifier")?;
    if bundle_id != "com.lumenopen.cua" {
        bail!(
            "unexpected Lumen Cua bundle id {bundle_id:?} at {}",
            path.display()
        );
    }
    let executable_name = plist_value(&plist, "CFBundleExecutable")?;
    if executable_name != "lumen-cua" {
        bail!("unexpected Lumen Cua executable {executable_name:?}");
    }
    let executable = path.join("Contents/MacOS/lumen-cua");
    if !executable.is_file() {
        bail!("Lumen Cua executable is missing: {}", executable.display());
    }

    let verified = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(path)
        .status()
        .with_context(|| format!("verify Lumen Cua signature at {}", path.display()))?;
    if !verified.success() {
        bail!("Lumen Cua code signature is invalid at {}", path.display());
    }
    let requirement = designated_requirement(path)?;
    if requirement.contains("cdhash") || !requirement.contains("certificate") {
        bail!("Lumen Cua requires a certificate-backed designated requirement; got {requirement}");
    }
    Ok(requirement)
}

#[cfg(target_os = "macos")]
fn plist_value(plist: &Path, key: &str) -> Result<String> {
    let output = Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg(format!("Print :{key}"))
        .arg(plist)
        .output()
        .with_context(|| format!("read {key} from {}", plist.display()))?;
    if !output.status.success() {
        bail!("missing {key} in {}", plist.display());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(target_os = "macos")]
fn designated_requirement(path: &Path) -> Result<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-"])
        .arg(path)
        .output()
        .with_context(|| format!("read designated requirement for {}", path.display()))?;
    if !output.status.success() {
        bail!(
            "could not read designated requirement for {}",
            path.display()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_designated_requirement(&stdout, &stderr)
}

fn parse_designated_requirement(stdout: &str, stderr: &str) -> Result<String> {
    stdout
        .lines()
        .chain(stderr.lines())
        .find_map(|line| {
            line.split_once("designated => ")
                .map(|(_, value)| value.trim().to_owned())
        })
        .context("codesign returned no designated requirement")
}

fn certificate_constraint(requirement: &str) -> Result<String> {
    requirement
        .split_once(" and certificate")
        .map(|(_, suffix)| format!("certificate{suffix}"))
        .context("designated requirement has no certificate constraint")
}

fn require_certificate_constraint(requirement: &str, expected: &str) -> Result<()> {
    let actual = certificate_constraint(requirement)?;
    if actual != expected {
        bail!("Lumen Cua signing certificate changed; expected {expected}, got {actual}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn current_app_certificate_constraint() -> Result<String> {
    let executable = std::env::current_exe().context("resolve current Navi executable")?;
    let app = executable
        .ancestors()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("app"))
        .context("Lumen Navi is not running from an app bundle")?;
    certificate_constraint(&designated_requirement(app)?)
}

#[cfg(target_os = "macos")]
fn register_launch_services(path: &Path) -> Result<()> {
    const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister";
    let status = Command::new(LSREGISTER)
        .arg("-f")
        .arg(path)
        .status()
        .with_context(|| format!("register {} with LaunchServices", path.display()))?;
    if !status.success() {
        bail!("LaunchServices registration failed with {status}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_installed_runtime(paths: &CuaPaths, app: &Path) -> Result<()> {
    let executable = app.join("Contents/MacOS/lumen-cua");
    let mut launcher = Command::new("/usr/bin/open")
        .arg("-n")
        .arg("-W")
        .arg("-g")
        .arg(app)
        .arg("--args")
        .arg("serve")
        .spawn()
        .with_context(|| {
            format!(
                "launch installed Lumen Cua through LaunchServices at {}",
                app.display()
            )
        })?;
    let verification = (|| {
        if !wait_for_path_state(&paths.socket, true, Duration::from_secs(5)) {
            bail!("installed Lumen Cua did not create its IPC endpoint");
        }
        let client =
            CuaClient::new(&paths.socket, &paths.token_file).with_timeout(Duration::from_secs(2));
        client
            .status()
            .context("query installed Lumen Cua status")?;
        client
            .shutdown()
            .context("stop installed Lumen Cua after verification")?;
        if !wait_for_path_state(&paths.socket, false, Duration::from_secs(5)) {
            bail!("installed Lumen Cua did not remove its IPC endpoint");
        }
        if !wait_for_child_exit(&mut launcher, Duration::from_secs(5))? {
            bail!("LaunchServices did not observe Lumen Cua exit after verification");
        }
        Ok(())
    })();

    if verification.is_err() {
        if paths.socket.exists() {
            let client = CuaClient::new(&paths.socket, &paths.token_file)
                .with_timeout(Duration::from_secs(1));
            let _ = client.shutdown();
        }
        let _ = terminate_exact_cua_process(&executable);
        if !wait_for_child_exit(&mut launcher, Duration::from_secs(2)).unwrap_or(false) {
            let _ = launcher.kill();
            let _ = launcher.wait();
        }
        if paths.socket.exists() {
            let _ = std::fs::remove_file(&paths.socket);
        }
    }
    verification
}

#[cfg(target_os = "macos")]
fn terminate_exact_cua_process(executable: &Path) -> Result<()> {
    let command_line = format!("{} serve", executable.display());
    let output = Command::new("/usr/bin/pgrep")
        .args(["-f", "-x", &command_line])
        .output()
        .context("find installed Lumen Cua process")?;
    if !output.status.success() && output.status.code() != Some(1) {
        bail!("pgrep failed while locating installed Lumen Cua");
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let pid: u32 = line
            .trim()
            .parse()
            .context("parse installed Lumen Cua process id")?;
        let status = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .with_context(|| format!("terminate installed Lumen Cua process {pid}"))?;
        if !status.success() {
            bail!("could not terminate installed Lumen Cua process {pid}");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn wait_for_child_exit(child: &mut std::process::Child, timeout: Duration) -> Result<bool> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if child
            .try_wait()
            .context("poll Lumen Cua process")?
            .is_some()
        {
            return Ok(true);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(child
        .try_wait()
        .context("poll Lumen Cua process")?
        .is_some())
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
    use lumen_cua::DirectCaptureStatus;
    use lumen_platform::PermissionState;

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

    #[test]
    fn permission_setup_is_ready_only_after_a_live_capture_probe() {
        let read_only = CuaStatus {
            screen_recording: PermissionState::Granted,
            screen_recording_capturable: None,
            direct_capture_status: DirectCaptureStatus::NotChecked,
            direct_capture_error: None,
        };
        let ready = CuaStatus {
            screen_recording_capturable: Some(true),
            direct_capture_status: DirectCaptureStatus::Ready,
            ..read_only.clone()
        };

        assert!(!permission_setup_is_ready(&read_only));
        assert!(permission_setup_is_ready(&ready));
    }

    #[test]
    fn failed_install_verification_restores_the_previous_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("Lumen Cua.app");
        let staging = temp.path().join("Lumen Cua.staging.app");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("generation"), "old").unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("generation"), "new").unwrap();

        let error =
            activate_staged_bundle(&staging, &target, |_| anyhow::bail!("health check failed"))
                .unwrap_err();

        assert!(error.to_string().contains("health check failed"));
        assert_eq!(
            std::fs::read_to_string(target.join("generation")).unwrap(),
            "old"
        );
        assert!(!staging.exists());
    }

    #[test]
    fn successful_install_verification_commits_the_new_bundle() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("Lumen Cua.app");
        let staging = temp.path().join("Lumen Cua.staging.app");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("generation"), "old").unwrap();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("generation"), "new").unwrap();

        activate_staged_bundle(&staging, &target, |installed| {
            assert_eq!(
                std::fs::read_to_string(installed.join("generation")).unwrap(),
                "new"
            );
            Ok(())
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(target.join("generation")).unwrap(),
            "new"
        );
        assert!(!staging.exists());
    }

    #[test]
    fn designated_requirement_is_read_from_codesign_stdout() {
        let requirement = parse_designated_requirement(
            "designated => identifier \"com.lumenopen.cua\" and certificate root = H\"abc\"\n",
            "Executable=/Applications/Lumen Cua.app/Contents/MacOS/lumen-cua\n",
        )
        .unwrap();

        assert_eq!(
            requirement,
            "identifier \"com.lumenopen.cua\" and certificate root = H\"abc\""
        );
    }

    #[test]
    fn cua_certificate_must_match_the_current_app_constraint() {
        let expected = certificate_constraint(
            "identifier \"com.lumenopen.navi\" and certificate root = H\"stable\"",
        )
        .unwrap();

        require_certificate_constraint(
            "identifier \"com.lumenopen.cua\" and certificate root = H\"stable\"",
            &expected,
        )
        .unwrap();
        assert!(require_certificate_constraint(
            "identifier \"com.lumenopen.cua\" and certificate root = H\"changed\"",
            &expected,
        )
        .is_err());
    }
}
