//! Resolve a macOS app icon PNG from a bundle id (cached under data_dir).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

/// 64×64 PNG as `data:image/png;base64,...`, or None if the app isn't installed.
pub fn data_url_for_bundle(cache_dir: &Path, bundle_id: &str) -> Option<String> {
    let id = bundle_id.trim();
    if id.is_empty() || id.contains('/') || id.contains('\0') {
        return None;
    }
    let cache = cache_dir.join(format!("{}.png", id.replace('/', "_")));
    if let Ok(bytes) = fs::read(&cache) {
        if !bytes.is_empty() {
            return Some(to_data_url(&bytes));
        }
    }
    let app = resolve_app_path(id)?;
    let icns = find_icns(&app)?;
    let _ = fs::create_dir_all(cache_dir);
    let status = Command::new("sips")
        .args([
            "-s",
            "format",
            "png",
            "-z",
            "64",
            "64",
            icns.to_str()?,
            "--out",
            cache.to_str()?,
        ])
        .output()
        .ok()?;
    if !status.status.success() {
        let _ = fs::remove_file(&cache);
        return None;
    }
    let bytes = fs::read(&cache).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(to_data_url(&bytes))
}

fn to_data_url(bytes: &[u8]) -> String {
    format!("data:image/png;base64,{}", B64.encode(bytes))
}

fn resolve_app_path(bundle_id: &str) -> Option<PathBuf> {
    let out = Command::new("mdfind")
        .arg(format!("kMDItemCFBundleIdentifier == '{bundle_id}'"))
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut apps: Vec<PathBuf> = text
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
        .collect();
    apps.sort_by_key(|p| {
        let s = p.to_string_lossy();
        if s.starts_with("/Applications/") {
            0u8
        } else if s.starts_with("/System/Applications/") {
            1
        } else if s.starts_with("/System/Library/") {
            2
        } else if s.contains("/target/") {
            9
        } else {
            5
        }
    });
    apps.into_iter().next()
}

fn find_icns(app: &Path) -> Option<PathBuf> {
    let resources = app.join("Contents/Resources");
    let named = Command::new("defaults")
        .args([
            "read",
            &app.join("Contents/Info").to_string_lossy(),
            "CFBundleIconFile",
        ])
        .output()
        .ok()
        .and_then(|o| {
            let name = String::from_utf8(o.stdout).ok()?.trim().to_string();
            if name.is_empty() {
                return None;
            }
            Some(name)
        });
    if let Some(name) = named {
        let stem = name.trim_end_matches(".icns");
        for cand in [resources.join(format!("{stem}.icns")), resources.join(name)] {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    fs::read_dir(resources)
        .ok()?
        .filter_map(|e| e.ok())
        .find_map(|e| {
            let p = e.path();
            (p.extension().and_then(|x| x.to_str()) == Some("icns")).then_some(p)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_pathy_ids() {
        let dir = std::env::temp_dir();
        assert!(data_url_for_bundle(&dir, "").is_none());
        assert!(data_url_for_bundle(&dir, "../etc").is_none());
    }

    #[test]
    fn resolves_ghostty_when_installed() {
        if !PathBuf::from("/Applications/Ghostty.app").is_dir() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let url = data_url_for_bundle(dir.path(), "com.mitchellh.ghostty");
        assert!(
            url.as_deref()
                .is_some_and(|u| u.starts_with("data:image/png;base64,")),
            "expected a png data url, got {url:?}"
        );
    }
}
