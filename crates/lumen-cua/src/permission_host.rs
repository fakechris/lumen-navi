use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

pub const PERMISSION_HOST_ARG: &str = "__permission-host";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PermissionHostRequest {
    result_file: PathBuf,
    result_device: u64,
    result_inode: u64,
}

impl PermissionHostRequest {
    fn parse(args: &[OsString]) -> Result<Self> {
        if args.first().map(OsString::as_os_str) != Some(OsStr::new(PERMISSION_HOST_ARG)) {
            bail!("not a Lumen Cua permission-host request");
        }
        let result_file = args
            .windows(2)
            .find(|pair| pair[0] == "--result-file")
            .map(|pair| PathBuf::from(&pair[1]))
            .context("permission host omitted --result-file")?;
        let result_device = parse_u64_arg(args, "--result-device")?;
        let result_inode = parse_u64_arg(args, "--result-inode")?;
        Ok(Self {
            result_file,
            result_device,
            result_inode,
        })
    }
}

fn parse_u64_arg(args: &[OsString], name: &str) -> Result<u64> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .and_then(|pair| pair[1].to_str())
        .with_context(|| format!("permission host omitted {name}"))?
        .parse()
        .with_context(|| format!("permission host received invalid {name}"))
}

pub fn is_permission_host_request(args: &[OsString]) -> bool {
    args.first().map(OsString::as_os_str) == Some(OsStr::new(PERMISSION_HOST_ARG))
}

pub fn run(args: &[OsString]) -> Result<()> {
    ensure_running_inside_lumen_cua_app()?;
    let request = PermissionHostRequest::parse(args)?;
    let mut result_file = open_result_file(&request, std::env::temp_dir())?;
    let status = crate::permissions::request_and_probe_screen_capture();
    write_result(&mut result_file, &status)
}

fn open_result_file(
    request: &PermissionHostRequest,
    trusted_temp_root: PathBuf,
) -> Result<std::fs::File> {
    let path = &request.result_file;
    let name = path
        .file_name()
        .and_then(OsStr::to_str)
        .context("permission result file has no valid name")?;
    if !name.starts_with("lumen-cua-permission-") || !name.ends_with(".json") {
        bail!("permission result file is outside the private temporary namespace");
    }
    let root = std::fs::canonicalize(&trusted_temp_root).with_context(|| {
        format!(
            "resolve private temporary directory {}",
            trusted_temp_root.display()
        )
    })?;
    let parent = path
        .parent()
        .context("permission result file has no parent")?;
    let parent = std::fs::canonicalize(parent)
        .with_context(|| format!("resolve permission result directory {}", parent.display()))?;
    if !parent.starts_with(&root) {
        bail!("permission result file is outside the private temporary directory");
    }

    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open private permission result file {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect open permission result file {}", path.display()))?;
    if !metadata.is_file() {
        bail!("permission result path is not a regular file");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("permission result file is not owned by the current user");
        }
        if metadata.mode() & 0o777 != 0o600 {
            bail!("permission result file must have mode 0600");
        }
        if metadata.nlink() != 1 {
            bail!("permission result file must not have hard links");
        }
        if metadata.dev() != request.result_device || metadata.ino() != request.result_inode {
            bail!("permission result file identity changed before the host opened it");
        }
    }
    Ok(file)
}

fn write_result(file: &mut std::fs::File, status: &crate::CuaStatus) -> Result<()> {
    use std::io::{Seek, Write};

    let payload = serde_json::to_vec(status).context("encode permission result")?;
    file.set_len(0).context("truncate permission result file")?;
    file.rewind().context("rewind permission result file")?;
    file.write_all(&payload)
        .context("write permission result file")?;
    file.sync_all().ok();
    Ok(())
}

fn ensure_running_inside_lumen_cua_app() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let executable = std::env::current_exe().context("resolve Lumen Cua executable")?;
        let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
        let inside_bundle = executable
            .to_string_lossy()
            .contains("/Lumen Cua.app/Contents/MacOS/");
        if !inside_bundle {
            bail!("permission host must run from the installed Lumen Cua.app bundle");
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::Path;

    fn request_for(path: &Path) -> PermissionHostRequest {
        let metadata = std::fs::metadata(path).unwrap();
        PermissionHostRequest {
            result_file: path.to_owned(),
            result_device: metadata.dev(),
            result_inode: metadata.ino(),
        }
    }

    #[test]
    fn permission_host_requires_a_precreated_private_result_file() {
        let temp = tempfile::tempdir().unwrap();
        let result = temp.path().join("lumen-cua-permission-test.json");
        std::fs::write(&result, []).unwrap();
        std::fs::set_permissions(&result, std::fs::Permissions::from_mode(0o600)).unwrap();

        let metadata = std::fs::metadata(&result).unwrap();
        let request = PermissionHostRequest::parse(&[
            OsString::from(PERMISSION_HOST_ARG),
            OsString::from("--result-file"),
            result.as_os_str().to_owned(),
            OsString::from("--result-device"),
            OsString::from(metadata.dev().to_string()),
            OsString::from("--result-inode"),
            OsString::from(metadata.ino().to_string()),
        ])
        .unwrap();

        assert_eq!(request.result_file, result);
        open_result_file(&request, temp.path().to_owned()).unwrap();
    }

    #[test]
    fn permission_host_rejects_result_files_outside_its_private_directory() {
        let trusted = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let result = outside.path().join("lumen-cua-permission-outside.json");
        std::fs::write(&result, []).unwrap();

        let error = open_result_file(&request_for(&result), trusted.path().to_owned()).unwrap_err();
        assert!(error.to_string().contains("private temporary directory"));
    }

    #[test]
    fn permission_host_rejects_symlink_results() {
        let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let target = temp.path().join("lumen-cua-permission-target.json");
        let link = temp.path().join("lumen-cua-permission-link.json");
        std::fs::write(&target, []).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut request = request_for(&target);
        request.result_file = link;
        let error = open_result_file(&request, std::env::temp_dir()).unwrap_err();
        assert!(error
            .to_string()
            .contains("open private permission result file"));
    }

    #[test]
    fn permission_host_requires_exactly_mode_0600() {
        let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let result = temp.path().join("lumen-cua-permission-mode.json");
        std::fs::write(&result, []).unwrap();
        std::fs::set_permissions(&result, std::fs::Permissions::from_mode(0o644)).unwrap();

        let error = open_result_file(&request_for(&result), std::env::temp_dir()).unwrap_err();
        assert!(error.to_string().contains("mode 0600"));
    }

    #[test]
    fn permission_host_rejects_a_replaced_result_inode() {
        let temp = tempfile::tempdir_in(std::env::temp_dir()).unwrap();
        let result = temp.path().join("lumen-cua-permission-replaced.json");
        std::fs::write(&result, []).unwrap();
        std::fs::set_permissions(&result, std::fs::Permissions::from_mode(0o600)).unwrap();
        let request = request_for(&result);
        std::fs::remove_file(&result).unwrap();
        std::fs::write(&result, []).unwrap();
        std::fs::set_permissions(&result, std::fs::Permissions::from_mode(0o600)).unwrap();

        let error = open_result_file(&request, std::env::temp_dir()).unwrap_err();
        assert!(error.to_string().contains("identity changed"));
    }
}
