//! Download + install SenseVoice sherpa package (onboarding).
//!
//! Uses system `curl` + `tar` (macOS-friendly, same approach as lumen-asr).

use crate::paths::{lumen_models_dir, sensevoice_ready};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

/// Official int8 SenseVoice package (zh/en/ja/ko/yue).
pub const SENSEVOICE_ARCHIVE_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";
pub const SENSEVOICE_ARCHIVE_NAME: &str =
    "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";

#[derive(Debug, Clone)]
pub struct DownloadProgress {
    pub phase: String,
    pub message: String,
    pub bytes: u64,
    pub total: Option<u64>,
}

/// Install SenseVoice under `models_root/sensevoice`.
///
/// Default `models_root` is the **shared Lumen cluster** path
/// (`…/Application Support/Lumen/models`) so navi / asr / future apps share one download.
pub fn download_sensevoice_package(
    models_root: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(models_root).map_err(|e| e.to_string())?;
    let archive_path = models_root.join(SENSEVOICE_ARCHIVE_NAME);
    let extract_tmp = models_root.join("sensevoice-extract-tmp");
    let final_dir = models_root.join("sensevoice");

    if sensevoice_ready(&final_dir) {
        on_progress(DownloadProgress {
            phase: "done".into(),
            message: "SenseVoice already installed".into(),
            bytes: 0,
            total: None,
        });
        return Ok(final_dir);
    }

    if cancel.load(Ordering::SeqCst) {
        return Err("download cancelled".into());
    }

    on_progress(DownloadProgress {
        phase: "downloading".into(),
        message: "Downloading SenseVoice model…".into(),
        bytes: 0,
        total: None,
    });

    let archive_str = archive_path
        .to_str()
        .ok_or_else(|| "bad archive path".to_string())?;
    let status = Command::new("curl")
        .args(["-fL", "--progress-bar", "-o", archive_str, SENSEVOICE_ARCHIVE_URL])
        .status()
        .map_err(|e| format!("curl failed to start: {e}"))?;

    if cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&archive_path);
        return Err("download cancelled".into());
    }
    if !status.success() {
        return Err(format!(
            "download failed (curl exit {:?}). Check network or place model under {}",
            status.code(),
            final_dir.display()
        ));
    }

    let bytes = std::fs::metadata(&archive_path)
        .map(|m| m.len())
        .unwrap_or(0);
    on_progress(DownloadProgress {
        phase: "extracting".into(),
        message: "Extracting archive…".into(),
        bytes,
        total: Some(bytes),
    });

    let _ = std::fs::remove_dir_all(&extract_tmp);
    std::fs::create_dir_all(&extract_tmp).map_err(|e| e.to_string())?;

    let extract_str = extract_tmp
        .to_str()
        .ok_or_else(|| "bad extract path".to_string())?;
    let tar_status = Command::new("tar")
        .args(["-xjf", archive_str, "-C", extract_str])
        .status()
        .map_err(|e| format!("tar failed: {e}"))?;
    if !tar_status.success() {
        return Err("failed to extract model archive".into());
    }

    let found = find_sensevoice_dir(&extract_tmp).ok_or_else(|| {
        "extracted archive but could not find model*.onnx + tokens.txt".to_string()
    })?;

    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }
    std::fs::rename(&found, &final_dir).or_else(|_| {
        copy_dir_recursive(&found, &final_dir)?;
        let _ = std::fs::remove_dir_all(&found);
        Ok::<(), String>(())
    })?;

    let _ = std::fs::remove_dir_all(&extract_tmp);
    let _ = std::fs::remove_file(&archive_path);

    if !sensevoice_ready(&final_dir) {
        return Err("model installed but validation failed".into());
    }

    on_progress(DownloadProgress {
        phase: "done".into(),
        message: "SenseVoice ready".into(),
        bytes,
        total: Some(bytes),
    });
    Ok(final_dir)
}

/// Default install root: shared `…/Lumen/models` (cluster-wide).
pub fn default_models_root() -> PathBuf {
    lumen_models_dir()
}

fn find_sensevoice_dir(root: &Path) -> Option<PathBuf> {
    if sensevoice_ready(root) {
        return Some(root.to_path_buf());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if sensevoice_ready(&dir) {
            return Some(dir);
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    stack.push(e.path());
                }
            }
        }
    }
    None
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let e = e.map_err(|e| e.to_string())?;
        let to = dst.join(e.file_name());
        if e.path().is_dir() {
            copy_dir_recursive(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_url_is_https() {
        assert!(SENSEVOICE_ARCHIVE_URL.starts_with("https://"));
        assert!(SENSEVOICE_ARCHIVE_NAME.ends_with(".tar.bz2"));
    }
}
