//! Resolve offline ASR model directories (SenseVoice / Whisper).
//!
//! **Shared cluster root** — all Lumen apps (navi, asr, future) install and
//! discover models under one place so users do not re-download per product:
//!
//! - macOS: `~/Library/Application Support/Lumen/models/`
//! - other: `~/.lumen/models/`
//! - override: env `LUMEN_MODELS_DIR` or config `asr.models_root`
//!
//! Per-app legacy paths (`LumenAsr/models`, `LumenNavi/models`, coli caches)
//! are still **discovered** and selectable; new downloads go to the shared root.

use std::path::{Path, PathBuf};

/// Env var for the shared Lumen models root (cluster-wide).
pub const ENV_LUMEN_MODELS_DIR: &str = "LUMEN_MODELS_DIR";

pub fn user_home_dir() -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Some(path) = nonempty_env_path(key) {
            return path;
        }
    }
    match (std::env::var_os("HOMEDRIVE"), std::env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) if !drive.is_empty() && !path.is_empty() => {
            let mut home = PathBuf::from(drive);
            home.push(path);
            home
        }
        _ => std::env::temp_dir(),
    }
}

fn nonempty_env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn legacy_model_roots(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Application Support/LumenAsr/models"),
        home.join("Library/Application Support/LumenNavi/models"),
        home.join(".lumen-asr/models"),
        home.join(".lumen-navi/models"),
    ]
}

fn legacy_source(root: &Path) -> &'static str {
    let path = root.to_string_lossy();
    if path.contains("LumenAsr") || path.contains(".lumen-asr") {
        "legacy-lumen-asr"
    } else {
        "legacy-lumen-navi"
    }
}

/// Shared models root for the Lumen app cluster.
///
/// Priority: `override_root` → `LUMEN_MODELS_DIR` → platform default
/// (`…/Application Support/Lumen/models` on macOS).
pub fn lumen_models_dir() -> PathBuf {
    lumen_models_dir_with_override(None)
}

pub fn lumen_models_dir_with_override(override_root: Option<&Path>) -> PathBuf {
    if let Some(p) = override_root {
        if !p.as_os_str().is_empty() {
            return p.to_path_buf();
        }
    }
    if let Some(root) = nonempty_env_path(ENV_LUMEN_MODELS_DIR) {
        return root;
    }
    let home = user_home_dir();
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Lumen/models")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".lumen/models")
    }
}

/// Canonical install / default lookup dir for SenseVoice under the cluster root.
pub fn shared_sensevoice_dir(models_root: Option<&Path>) -> PathBuf {
    lumen_models_dir_with_override(models_root).join("sensevoice")
}

/// Canonical install / default lookup dir for Whisper under the cluster root.
pub fn shared_whisper_dir(models_root: Option<&Path>) -> PathBuf {
    lumen_models_dir_with_override(models_root).join("whisper")
}

/// @deprecated Prefer [`lumen_models_dir`]. Alias kept for call sites.
pub fn app_models_dir() -> PathBuf {
    lumen_models_dir()
}

/// Prefer env → shared cluster → ready legacy locations → shared (install target).
pub fn default_sensevoice_dir() -> PathBuf {
    default_sensevoice_dir_with_root(None)
}

pub fn default_sensevoice_dir_with_root(models_root: Option<&Path>) -> PathBuf {
    if let Ok(p) = std::env::var("LUMEN_SENSEVOICE_DIR") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(p) = std::env::var("LUMEN_NAVI_SENSEVOICE_DIR") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }

    let shared = shared_sensevoice_dir(models_root);
    if sensevoice_ready(&shared) {
        return shared;
    }

    for (path, _) in sensevoice_discovery_paths(models_root) {
        if path != shared && sensevoice_ready(&path) {
            return path;
        }
    }
    // Empty shared path = default download / config target (one place for all apps).
    shared
}

pub fn default_whisper_dir() -> PathBuf {
    default_whisper_dir_with_root(None)
}

pub fn default_whisper_dir_with_root(models_root: Option<&Path>) -> PathBuf {
    if let Ok(p) = std::env::var("LUMEN_WHISPER_DIR") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }
    if let Ok(p) = std::env::var("LUMEN_NAVI_WHISPER_DIR") {
        let t = p.trim();
        if !t.is_empty() {
            return PathBuf::from(t);
        }
    }

    let shared = shared_whisper_dir(models_root);
    if whisper_ready(&shared) {
        return shared;
    }
    for (path, _) in whisper_discovery_paths(models_root) {
        if path != shared && whisper_ready(&path) {
            return path;
        }
    }
    shared
}

pub fn sensevoice_ready(dir: &Path) -> bool {
    sensevoice_model_path(dir).is_some() && sensevoice_tokens_path(dir).is_some()
}

pub fn whisper_ready(dir: &Path) -> bool {
    whisper_encoder_path(dir).is_some()
        && whisper_decoder_path(dir).is_some()
        && whisper_tokens_path(dir).is_some()
}

/// (path, source label) for SenseVoice discovery — shared first, then legacy.
fn sensevoice_discovery_paths(models_root: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    let shared_root = lumen_models_dir_with_override(models_root);
    out.push((shared_root.join("sensevoice"), "lumen-shared"));

    // Any ready subdir under shared models root (user-managed layouts)
    if let Ok(rd) = std::fs::read_dir(&shared_root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if name == "sensevoice" || name.contains("extract") {
                    continue;
                }
                if sensevoice_ready(&p) {
                    out.push((p, "lumen-shared"));
                }
            }
        }
    }

    let home = user_home_dir();
    for root in legacy_model_roots(&home) {
        out.push((root.join("sensevoice"), legacy_source(&root)));
    }
    for name in [
        "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17",
        "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    ] {
        out.push((home.join(".coli/models").join(name), "coli-cache"));
    }
    out
}

fn whisper_discovery_paths(models_root: Option<&Path>) -> Vec<(PathBuf, &'static str)> {
    let mut out = Vec::new();
    let shared_root = lumen_models_dir_with_override(models_root);
    out.push((shared_root.join("whisper"), "lumen-shared"));

    if let Ok(rd) = std::fs::read_dir(&shared_root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = e.file_name().to_string_lossy().to_string();
                if name == "whisper" || name.contains("extract") {
                    continue;
                }
                if whisper_ready(&p) {
                    out.push((p, "lumen-shared"));
                }
            }
        }
    }

    let home = user_home_dir();
    for root in legacy_model_roots(&home) {
        out.push((root.join("whisper"), legacy_source(&root)));
    }
    for name in ["sherpa-onnx-whisper-tiny.en", "sherpa-onnx-whisper-base.en"] {
        out.push((home.join(".coli/models").join(name), "coli-cache"));
    }
    out
}

/// Scan known locations for ready (or placeholder shared) model dirs.
///
/// Users can pick any ready path; new downloads land under the shared root.
pub fn scan_model_candidates() -> Vec<ModelCandidate> {
    scan_model_candidates_with_root(None)
}

pub fn scan_model_candidates_with_root(models_root: Option<&Path>) -> Vec<ModelCandidate> {
    let mut out = Vec::new();
    let mut push = |engine: &str, path: PathBuf, source: &str| {
        let ready = match engine {
            "sensevoice" => sensevoice_ready(&path),
            "whisper" => whisper_ready(&path),
            _ => false,
        };
        let is_shared_target =
            source == "lumen-shared" && (path.ends_with("sensevoice") || path.ends_with("whisper"));
        if !ready && !is_shared_target {
            return;
        }
        if !ready && is_shared_target {
            // List planned install path even if missing.
        } else if !path.exists() {
            return;
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let label = if ready {
            format!("{name} · {source}")
        } else {
            format!("{engine} · {source} — 下载目标（全 Lumen 应用共享）")
        };
        out.push(ModelCandidate {
            engine: engine.into(),
            path: path.display().to_string(),
            label,
            ready,
            source: source.into(),
        });
    };

    if let Ok(p) = std::env::var("LUMEN_SENSEVOICE_DIR") {
        if !p.trim().is_empty() {
            push("sensevoice", PathBuf::from(p.trim()), "env");
        }
    }
    if let Ok(p) = std::env::var("LUMEN_WHISPER_DIR") {
        if !p.trim().is_empty() {
            push("whisper", PathBuf::from(p.trim()), "env");
        }
    }

    for (path, source) in sensevoice_discovery_paths(models_root) {
        push("sensevoice", path, source);
    }
    for (path, source) in whisper_discovery_paths(models_root) {
        push("whisper", path, source);
    }

    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert((c.engine.clone(), c.path.clone())));
    // Prefer ready shared first in UI: sort ready+lumen-shared first
    out.sort_by(|a, b| {
        let score = |c: &ModelCandidate| {
            let mut s = 0i32;
            if c.ready {
                s += 10;
            }
            if c.source == "lumen-shared" {
                s += 5;
            }
            if c.source == "env" {
                s += 8;
            }
            -s // descending via reverse compare
        };
        score(a).cmp(&score(b)).then_with(|| a.path.cmp(&b.path))
    });
    out
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelCandidate {
    pub engine: String,
    pub path: String,
    pub label: String,
    pub ready: bool,
    pub source: String,
}

pub fn sensevoice_model_path(dir: &Path) -> Option<PathBuf> {
    for name in ["model.int8.onnx", "model.onnx", "sensevoice.onnx"] {
        let p = dir.join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn sensevoice_tokens_path(dir: &Path) -> Option<PathBuf> {
    let p = dir.join("tokens.txt");
    p.is_file().then_some(p)
}

pub fn whisper_encoder_path(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains("encoder") && name.ends_with(".onnx") {
            return Some(e.path());
        }
    }
    None
}

pub fn whisper_decoder_path(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains("decoder") && name.ends_with(".onnx") {
            return Some(e.path());
        }
    }
    None
}

pub fn whisper_tokens_path(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.contains("tokens") && name.ends_with(".txt") {
            return Some(e.path());
        }
    }
    let p = dir.join("tokens.txt");
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_ready_empty_dir() {
        let dir = std::env::temp_dir().join("lumen-navi-asr-empty-test");
        let _ = std::fs::create_dir_all(&dir);
        assert!(!sensevoice_ready(&dir));
        assert!(!whisper_ready(&dir));
    }

    #[test]
    fn shared_root_ends_with_models() {
        let d = lumen_models_dir();
        assert!(d.ends_with("models") || d.to_string_lossy().contains("models"));
    }

    #[test]
    fn override_models_root_install_path() {
        let root =
            std::env::temp_dir().join(format!("lumen-test-models-root-{}", std::process::id()));
        assert_eq!(lumen_models_dir_with_override(Some(&root)), root);
        assert_eq!(shared_sensevoice_dir(Some(&root)), root.join("sensevoice"));
        // Download target is always the shared subdir under models_root,
        // even when legacy caches exist on the machine.
        assert_eq!(shared_whisper_dir(Some(&root)), root.join("whisper"));
    }

    #[test]
    fn legacy_roots_cover_macos_and_dot_directory_layouts() {
        let home = Path::new("/home/alice");

        assert_eq!(
            legacy_model_roots(home),
            vec![
                home.join("Library/Application Support/LumenAsr/models"),
                home.join("Library/Application Support/LumenNavi/models"),
                home.join(".lumen-asr/models"),
                home.join(".lumen-navi/models"),
            ]
        );
    }

    #[test]
    fn shared_root_discovers_ready_model_in_custom_subdir() {
        let root =
            std::env::temp_dir().join(format!("lumen-navi-shared-custom-{}", std::process::id()));
        let custom = root.join("sherpa-sensevoice-custom");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&custom).unwrap();
        std::fs::write(custom.join("model.int8.onnx"), b"model").unwrap();
        std::fs::write(custom.join("tokens.txt"), b"tokens").unwrap();

        let candidates = scan_model_candidates_with_root(Some(&root));

        assert!(candidates.iter().any(|candidate| {
            candidate.engine == "sensevoice"
                && candidate.path == custom.display().to_string()
                && candidate.source == "lumen-shared"
                && candidate.ready
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_shared_targets_are_still_listed_for_installation() {
        let root = std::env::temp_dir().join(format!(
            "lumen-navi-shared-placeholder-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let candidates = scan_model_candidates_with_root(Some(&root));

        assert!(candidates.iter().any(|candidate| {
            candidate.engine == "sensevoice"
                && candidate.path == root.join("sensevoice").display().to_string()
                && !candidate.ready
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.engine == "whisper"
                && candidate.path == root.join("whisper").display().to_string()
                && !candidate.ready
        }));
    }
}
