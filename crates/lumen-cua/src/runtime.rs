use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CuaPaths {
    /// Standalone runtime bundle. Keeping this outside any caller's `.app`
    /// bundle gives Lumen Cua its own TCC attribution.
    pub app: PathBuf,
    pub socket: PathBuf,
    pub token_file: PathBuf,
}

impl CuaPaths {
    /// Stable product-family location so Navi, ASR, and future Lumen apps can
    /// connect to the same signed helper without sharing an application DB.
    pub fn for_current_user() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        #[cfg(target_os = "macos")]
        let root = home.join("Library/Application Support/Lumen/Cua");
        #[cfg(not(target_os = "macos"))]
        let root = home.join(".lumen/cua");
        Self::under(root)
    }

    pub fn under(data_dir: impl AsRef<Path>) -> Self {
        let root = data_dir.as_ref();
        let run_dir = root.join("run");
        Self {
            app: root.join("Lumen Cua.app"),
            socket: run_dir.join("cua.sock"),
            token_file: run_dir.join("cua.token"),
        }
    }
}

pub fn ensure_token_file(path: &Path) -> Result<String> {
    if path.exists() {
        harden_token_permissions(path)?;
        return read_token_file(path);
    }

    let parent = path.parent().context("token file has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        options
    };
    #[cfg(not(unix))]
    let options = {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        options
    };

    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_token_file(path);
        }
        Err(error) => return Err(error).with_context(|| format!("create {}", path.display())),
    };
    file.write_all(token.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    file.sync_all().ok();
    harden_token_permissions(path)?;
    Ok(token)
}

fn harden_token_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, permissions)
            .with_context(|| format!("secure {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn read_token_file(path: &Path) -> Result<String> {
    for attempt in 0..10 {
        let mut file = OpenOptions::new()
            .read(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?;
        let mut token = String::new();
        file.read_to_string(&mut token)
            .with_context(|| format!("read {}", path.display()))?;
        let token = token.trim();
        if token.len() >= 32 && token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(token.to_owned());
        }
        if attempt < 9 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    bail!("invalid Lumen Cua token file: {}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_stable_and_private() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("run/cua.token");
        let first = ensure_token_file(&path).unwrap();
        let second = ensure_token_file(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn existing_token_permissions_are_repaired() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cua.token");
        let token = ensure_token_file(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        assert_eq!(ensure_token_file(&path).unwrap(), token);
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn shared_app_path_is_outside_the_ipc_run_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = CuaPaths::under(temp.path());

        assert_eq!(paths.app, temp.path().join("Lumen Cua.app"));
        assert_eq!(paths.socket, temp.path().join("run/cua.sock"));
    }
}
