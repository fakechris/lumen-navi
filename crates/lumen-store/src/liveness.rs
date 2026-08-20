//! Overwrite-only Observe liveness frames.
//!
//! The 2-minute static-screen safety valve proves the capture loop is still
//! alive. It is not evidence: one JPEG per display, previous file replaced,
//! no `screenshot.v1`, no OCR/AX, no blob quota. Wipe discards it.

use std::fs;
use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::StoreError;

pub const LIVENESS_DIR: &str = "liveness";
const META_FILE: &str = "meta.json";
const PAYLOAD_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct LivenessFrameInput {
    pub display_id: u32,
    pub display_index: usize,
    pub is_main: bool,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LivenessDisplayMeta {
    pub display_id: u32,
    #[serde(default)]
    pub display_index: usize,
    #[serde(default)]
    pub is_main: bool,
    pub path: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub media_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LivenessMeta {
    pub payload_version: u32,
    pub captured_at: DateTime<Utc>,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub displays: Vec<LivenessDisplayMeta>,
}

pub fn put(
    data_dir: &Path,
    captured_at: DateTime<Utc>,
    app_name: Option<&str>,
    bundle_id: Option<&str>,
    frames: &[LivenessFrameInput],
) -> Result<LivenessMeta, StoreError> {
    let dir = data_dir.join(LIVENESS_DIR);
    let tmp = data_dir.join("tmp");
    fs::create_dir_all(&dir).map_err(StoreError::io)?;
    fs::create_dir_all(&tmp).map_err(StoreError::io)?;

    let mut displays = Vec::with_capacity(frames.len());
    let mut keep = vec![META_FILE.to_string()];
    for frame in frames {
        let name = display_filename(frame.display_id, &frame.media_type);
        let relative = format!("{LIVENESS_DIR}/{name}");
        atomic_write(&tmp, &dir.join(&name), &frame.bytes)?;
        keep.push(name);
        displays.push(LivenessDisplayMeta {
            display_id: frame.display_id,
            display_index: frame.display_index,
            is_main: frame.is_main,
            path: relative,
            bytes: frame.bytes.len() as u64,
            width: frame.width,
            height: frame.height,
            media_type: frame.media_type.clone(),
        });
    }

    let meta = LivenessMeta {
        payload_version: PAYLOAD_VERSION,
        captured_at,
        app_name: app_name.map(str::to_string),
        bundle_id: bundle_id.map(str::to_string),
        displays,
    };
    let json = serde_json::to_vec_pretty(&meta).map_err(StoreError::json)?;
    atomic_write(&tmp, &dir.join(META_FILE), &json)?;
    remove_orphans(&dir, &keep)?;
    Ok(meta)
}

pub fn read_meta(data_dir: &Path) -> Result<Option<LivenessMeta>, StoreError> {
    let path = data_dir.join(LIVENESS_DIR).join(META_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(StoreError::io)?;
    let meta = serde_json::from_slice(&bytes).map_err(StoreError::json)?;
    Ok(Some(meta))
}

pub fn wipe(data_dir: &Path) -> Result<(), StoreError> {
    let dir = data_dir.join(LIVENESS_DIR);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(StoreError::io)?;
    }
    Ok(())
}

fn display_filename(display_id: u32, media_type: &str) -> String {
    let ext = if media_type.contains("png") {
        "png"
    } else {
        "jpg"
    };
    format!("display-{display_id}.{ext}")
}

fn atomic_write(tmp_dir: &Path, dest: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let tmp_name = format!(
        "{}.{}.part",
        dest.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("liveness"),
        Uuid::new_v4()
    );
    let tmp_path = tmp_dir.join(tmp_name);
    {
        let mut f = fs::File::create(&tmp_path).map_err(StoreError::io)?;
        f.write_all(bytes).map_err(StoreError::io)?;
        f.sync_all().map_err(StoreError::io)?;
    }
    fs::rename(&tmp_path, dest).map_err(StoreError::io)?;
    Ok(())
}

fn remove_orphans(dir: &Path, keep: &[String]) -> Result<(), StoreError> {
    for entry in fs::read_dir(dir).map_err(StoreError::io)? {
        let entry = entry.map_err(StoreError::io)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if keep.iter().any(|k| k == name.as_ref()) {
            continue;
        }
        let path = entry.path();
        if path.is_file() {
            fs::remove_file(&path).map_err(StoreError::io)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn frame(id: u32, bytes: &[u8]) -> LivenessFrameInput {
        LivenessFrameInput {
            display_id: id,
            display_index: id.saturating_sub(1) as usize,
            is_main: id == 1,
            media_type: "image/jpeg".into(),
            width: 10,
            height: 10,
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn put_overwrites_same_display_and_drops_orphans() {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let first = put(
            data,
            Utc::now(),
            Some("Safari"),
            Some("com.apple.Safari"),
            &[frame(1, b"aaaa"), frame(2, b"bbbb")],
        )
        .unwrap();
        assert_eq!(first.displays.len(), 2);
        let p1 = data.join(&first.displays[0].path);
        assert_eq!(fs::read(&p1).unwrap(), b"aaaa");

        let second = put(data, Utc::now(), Some("Mail"), None, &[frame(1, b"cccc")]).unwrap();
        assert_eq!(second.displays.len(), 1);
        assert_eq!(second.app_name.as_deref(), Some("Mail"));
        assert_eq!(fs::read(&p1).unwrap(), b"cccc");
        assert!(!data.join("liveness/display-2.jpg").exists());
        let meta = read_meta(data).unwrap().unwrap();
        assert_eq!(meta.displays.len(), 1);
        assert_eq!(meta.displays[0].bytes, 4);
    }

    #[test]
    fn wipe_removes_dir() {
        let dir = tempdir().unwrap();
        put(dir.path(), Utc::now(), None, None, &[frame(1, b"x")]).unwrap();
        assert!(dir.path().join("liveness").exists());
        wipe(dir.path()).unwrap();
        assert!(!dir.path().join("liveness").exists());
        assert!(read_meta(dir.path()).unwrap().is_none());
    }

    #[test]
    fn read_missing_is_none() {
        let dir = tempdir().unwrap();
        assert!(read_meta(dir.path()).unwrap().is_none());
    }
}
