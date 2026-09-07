//! Embedded MIT cua-driver daemon.
//!
//! Spawned as a child of Lumen Cua (`CUA_DRIVER_EMBEDDED=1`) so TCC stays on
//! `com.lumenopen.cua`. Observe never starts this. Missing binary is a soft
//! skip — HID replay does not need it.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::act_driver_policy::{self, MCP_SERVER_NAME};
use crate::protocol::{ActDriverInfo, ACT_DRIVER_HOST_BUNDLE_ID};
use crate::CuaPaths;

struct DriverProcess {
    child: Child,
    binary: PathBuf,
    socket: PathBuf,
    /// Held so `CUA_DRIVER_PARENT_LIVENESS_STDIN` sees a live parent pipe.
    _stdin: Option<ChildStdin>,
}

static DRIVER: Mutex<Option<DriverProcess>> = Mutex::new(None);

pub fn status(paths: &CuaPaths) -> ActDriverInfo {
    let binary = resolve_binary(paths);
    let running = socket_live(&paths.driver_socket) && child_alive();
    let version = binary.as_ref().and_then(|p| probe_version(p));
    let mut info = base_info(binary.as_deref(), &paths.driver_socket, version);
    info.present = binary.is_some();
    info.running = running;
    if binary.is_none() {
        info.error = Some("cua-driver binary is not bundled in Lumen Cua.app".into());
    }
    info
}

pub fn ensure(paths: &CuaPaths) -> Result<ActDriverInfo> {
    let Some(binary) = resolve_binary(paths) else {
        return Ok(status(paths));
    };
    let policy = policy_path(paths);
    let managed = managed_policy_path(paths);
    act_driver_policy::write_policy_files(&policy, &managed)?;
    write_skill_file(paths);
    if socket_live(&paths.driver_socket) && child_alive() {
        return Ok(status(paths));
    }
    stop();
    spawn(&binary, paths, &policy, &managed)?;
    wait_for_socket(&paths.driver_socket, Duration::from_secs(8))?;
    Ok(status(paths))
}

/// Invoke one cua-driver tool against the running embedded daemon.
pub fn call(paths: &CuaPaths, tool: &str, arguments: Value) -> Result<Value> {
    let Some(binary) = resolve_binary(paths) else {
        bail!("cua-driver binary is not bundled in Lumen Cua.app");
    };
    if !socket_live(&paths.driver_socket) || !child_alive() {
        ensure(paths)?;
    }
    let output = Command::new(&binary)
        .arg("call")
        .arg(tool)
        .arg(arguments.to_string())
        .arg("--socket")
        .arg(&paths.driver_socket)
        .env("CUA_DRIVER_EMBEDDED", "1")
        .env("CUA_DRIVER_HOST_BUNDLE_ID", ACT_DRIVER_HOST_BUNDLE_ID)
        .output()
        .with_context(|| format!("cua-driver call {tool}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "cua-driver call {tool} failed ({status}): {stderr}{stdout}",
            status = output.status,
            stderr = stderr.trim(),
            stdout = stdout.trim()
        );
    }
    let text = stdout.trim();
    if text.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(text).or_else(|_| Ok(json_string(text)))
}

fn json_string(text: &str) -> Value {
    Value::String(text.to_string())
}

pub fn policy_path(paths: &CuaPaths) -> PathBuf {
    paths
        .driver_socket
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("driver-policy.yaml")
}

pub fn managed_policy_path(paths: &CuaPaths) -> PathBuf {
    paths
        .driver_socket
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("driver-managed-policy.rego")
}

fn write_skill_file(paths: &CuaPaths) {
    let run_dir = paths
        .driver_socket
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or_else(|| Path::new("."));
    let skill_dir = run_dir.join("skills/computer-use");
    if fs::create_dir_all(&skill_dir).is_ok() {
        let _ = fs::write(
            skill_dir.join("SKILL.md"),
            include_str!("../skills/computer-use.md"),
        );
    }
}

pub fn stop() {
    let mut slot = DRIVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(mut driver) = slot.take() {
        let _ = driver.child.kill();
        let _ = driver.child.wait();
        let _ = fs::remove_file(&driver.socket);
        let _ = driver.binary;
    }
}

pub fn resolve_binary(paths: &CuaPaths) -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("LUMEN_CUA_DRIVER_BIN") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // Contents/MacOS/lumen-cua → Contents/Helpers/cua-driver
        if let Some(macos_dir) = exe.parent() {
            if let Some(contents) = macos_dir.parent() {
                let nested = contents.join("Helpers/cua-driver");
                if nested.is_file() {
                    return Some(nested);
                }
            }
        }
    }
    let nested = paths.app.join("Contents/Helpers/cua-driver");
    if nested.is_file() {
        return Some(nested);
    }
    None
}

fn base_info(binary: Option<&Path>, socket: &Path, version: Option<String>) -> ActDriverInfo {
    let mcp_command = binary.map(|p| p.display().to_string());
    let mcp_args = if binary.is_some() {
        vec![
            "mcp".into(),
            "--embedded".into(),
            "--socket".into(),
            socket.display().to_string(),
        ]
    } else {
        Vec::new()
    };
    let mcp_snippet = mcp_command
        .as_ref()
        .map(|cmd| mcp_toml_snippet(cmd, &mcp_args));
    ActDriverInfo {
        present: binary.is_some(),
        running: false,
        binary_path: binary.map(|p| p.display().to_string()),
        socket_path: Some(socket.display().to_string()),
        mcp_command,
        mcp_args,
        host_bundle_id: ACT_DRIVER_HOST_BUNDLE_ID.into(),
        embedded: true,
        version,
        error: None,
        mcp_server_name: MCP_SERVER_NAME.into(),
        policy_path: Some(
            socket
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("driver-policy.yaml")
                .display()
                .to_string(),
        ),
        mcp_snippet,
    }
}

fn mcp_toml_snippet(command: &str, args: &[String]) -> String {
    let escaped_cmd = command.replace('\\', "\\\\").replace('"', "\\\"");
    let toml_args = args
        .iter()
        .map(|a| format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[mcp_servers.{MCP_SERVER_NAME}]\ncommand = \"{escaped_cmd}\"\nargs = [{toml_args}]\n")
}

fn child_alive() -> bool {
    let mut slot = DRIVER.lock().unwrap_or_else(|e| e.into_inner());
    match slot.as_mut() {
        Some(driver) => match driver.child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => {
                *slot = None;
                false
            }
            Err(_) => false,
        },
        None => false,
    }
}

fn socket_live(path: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(path).is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn spawn(binary: &Path, paths: &CuaPaths, policy: &Path, managed: &Path) -> Result<()> {
    let socket = &paths.driver_socket;
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)?;
    }
    if socket.exists() {
        let _ = fs::remove_file(socket);
    }
    let mut cmd = Command::new(binary);
    cmd.arg("serve")
        .arg("--embedded")
        .arg("--socket")
        .arg(socket)
        .arg("--host-bundle-id")
        .arg(ACT_DRIVER_HOST_BUNDLE_ID)
        .env("CUA_DRIVER_EMBEDDED", "1")
        .env("CUA_DRIVER_HOST_BUNDLE_ID", ACT_DRIVER_HOST_BUNDLE_ID)
        .env("CUA_DRIVER_PERMISSION_MODE", "standard")
        .env("CUA_DRIVER_POLICY_FILE", policy)
        .env("CUA_DRIVER_MANAGED_POLICY_FILE", managed)
        .env("CUA_DRIVER_PARENT_LIVENESS_STDIN", "1")
        .env("CUA_DRIVER_RS_TELEMETRY_ENABLED", "false")
        .env("CUA_DRIVER_RS_UPDATE_CHECK", "false")
        .env("CUA_DRIVER_ENABLE_LEGACY_PAGE_MUTATIONS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn embedded cua-driver from {}", binary.display()))?;
    let stdin = child.stdin.take();
    // Give a crashed-immediate child a chance to write stderr.
    thread::sleep(Duration::from_millis(50));
    if let Ok(Some(status)) = child.try_wait() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut err);
        }
        bail!(
            "cua-driver serve exited immediately ({status}): {}",
            err.trim()
        );
    }
    drain_stderr(child.stderr.take());
    let mut slot = DRIVER.lock().unwrap_or_else(|e| e.into_inner());
    *slot = Some(DriverProcess {
        child,
        binary: binary.to_path_buf(),
        socket: socket.to_path_buf(),
        _stdin: stdin,
    });
    Ok(())
}

fn drain_stderr(stderr: Option<std::process::ChildStderr>) {
    let Some(mut stderr) = stderr else {
        return;
    };
    let _ = thread::Builder::new()
        .name("cua-driver-stderr".into())
        .spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        for line in text.lines() {
                            let line = line.trim();
                            if !line.is_empty() {
                                tracing::debug!(target: "cua_driver", "{line}");
                            }
                        }
                    }
                }
            }
        });
}

fn wait_for_socket(path: &Path, timeout: Duration) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if socket_live(path) {
            return Ok(());
        }
        if !child_alive() {
            let mut err = String::new();
            if let Some(mut driver) = DRIVER.lock().unwrap_or_else(|e| e.into_inner()).take() {
                if let Some(mut stderr) = driver.child.stderr.take() {
                    let _ = stderr.read_to_string(&mut err);
                }
            }
            bail!(
                "embedded cua-driver exited before its socket was ready: {}",
                err.trim()
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "embedded cua-driver socket did not appear at {} within {}s",
        path.display(),
        timeout.as_secs()
    );
}

fn probe_version(binary: &Path) -> Option<String> {
    let output = Command::new(binary).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ensure_token_file;

    #[test]
    fn missing_binary_is_a_soft_status_not_a_crash() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        ensure_token_file(&paths.token_file).unwrap();
        let info = status(&paths);
        assert!(!info.present);
        assert!(!info.running);
        assert!(info.embedded);
        assert_eq!(info.host_bundle_id, ACT_DRIVER_HOST_BUNDLE_ID);
        assert!(info.error.as_deref().unwrap().contains("not bundled"));
        assert!(info.mcp_command.is_none());
    }

    #[test]
    fn resolve_binary_from_app_helpers() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        let helper = paths.app.join("Contents/Helpers/cua-driver");
        std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
        std::fs::write(&helper, b"").unwrap();
        assert_eq!(resolve_binary(&paths).as_deref(), Some(helper.as_path()));
    }

    #[test]
    fn mcp_args_point_at_the_private_driver_socket() {
        let temp = tempfile::tempdir().unwrap();
        let fake = temp.path().join("cua-driver");
        std::fs::write(&fake, b"").unwrap();
        let info = base_info(Some(&fake), &temp.path().join("run/driver.sock"), None);
        assert_eq!(info.mcp_args[0], "mcp");
        assert!(info.mcp_args.contains(&"--embedded".into()));
        assert!(info.mcp_args.iter().any(|a| a.ends_with("driver.sock")));
        assert_eq!(info.mcp_server_name, MCP_SERVER_NAME);
        let snippet = info.mcp_snippet.expect("snippet");
        assert!(snippet.contains("[mcp_servers.computer-use]"));
        assert!(snippet.contains("\"mcp\""));
    }

    #[test]
    fn policy_paths_sit_next_to_the_driver_socket() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());
        assert!(policy_path(&paths)
            .to_string_lossy()
            .ends_with("driver-policy.yaml"));
        assert!(managed_policy_path(&paths)
            .to_string_lossy()
            .ends_with("driver-managed-policy.rego"));
    }
}
