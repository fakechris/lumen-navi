//! Resolve offline ASR model directories (SenseVoice / Whisper).
//!
//! Pattern mirrors lumen-asr: env → app models dir → known local caches.

use std::path::{Path, PathBuf};

/// Product Application Support models root for Navi.
pub fn app_models_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    #[cfg(target_os = "macos")]
    {
        PathBuf::from(home).join("Library/Application Support/LumenNavi/models")
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::from(home).join(".lumen-navi/models")
    }
}

/// Prefer env, then Navi app dir, then shared LumenAsr / coli caches (dev machines).
pub fn default_sensevoice_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LUMEN_NAVI_SENSEVOICE_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("LUMEN_SENSEVOICE_DIR") {
        return PathBuf::from(p);
    }
    let app = app_models_dir().join("sensevoice");
    if sensevoice_ready(&app) {
        return app;
    }
    // Reuse lumen-asr install if present
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let candidates = [
        format!("{home}/Library/Application Support/LumenAsr/models/sensevoice"),
        format!("{home}/.coli/models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"),
        format!("{home}/.coli/models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17"),
    ];
    for c in candidates {
        let p = PathBuf::from(&c);
        if sensevoice_ready(&p) {
            return p;
        }
    }
    app
}

pub fn default_whisper_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LUMEN_NAVI_WHISPER_DIR") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("LUMEN_WHISPER_DIR") {
        return PathBuf::from(p);
    }
    let app = app_models_dir().join("whisper");
    if whisper_ready(&app) {
        return app;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let candidates = [
        format!("{home}/Library/Application Support/LumenAsr/models/whisper"),
        format!("{home}/.coli/models/sherpa-onnx-whisper-tiny.en"),
        format!("{home}/.coli/models/sherpa-onnx-whisper-base.en"),
    ];
    for c in candidates {
        let p = PathBuf::from(&c);
        if whisper_ready(&p) {
            return p;
        }
    }
    app
}

pub fn sensevoice_ready(dir: &Path) -> bool {
    sensevoice_model_path(dir).is_some() && sensevoice_tokens_path(dir).is_some()
}

pub fn whisper_ready(dir: &Path) -> bool {
    whisper_encoder_path(dir).is_some()
        && whisper_decoder_path(dir).is_some()
        && whisper_tokens_path(dir).is_some()
}

/// Scan known locations for ready (or placeholder app) model dirs.
pub fn scan_model_candidates() -> Vec<ModelCandidate> {
    let mut out = Vec::new();
    let mut push = |engine: &str, path: PathBuf, source: &str| {
        if !path.exists() && source != "app" {
            return;
        }
        let ready = match engine {
            "sensevoice" => sensevoice_ready(&path),
            "whisper" => whisper_ready(&path),
            _ => false,
        };
        if !ready && source != "app" {
            return;
        }
        // Ensure parent exists listing for app target
        if source == "app" && !path.exists() {
            // still list planned install path
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
            format!("{engine} ({source}) — 下载目标")
        };
        out.push(ModelCandidate {
            engine: engine.into(),
            path: path.display().to_string(),
            label,
            ready,
            source: source.into(),
        });
    };

    let app_sv = app_models_dir().join("sensevoice");
    push("sensevoice", app_sv, "app");
    if let Ok(p) = std::env::var("LUMEN_NAVI_SENSEVOICE_DIR") {
        push("sensevoice", PathBuf::from(p), "env");
    }
    if let Ok(p) = std::env::var("LUMEN_SENSEVOICE_DIR") {
        push("sensevoice", PathBuf::from(p), "env");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let h = PathBuf::from(&home);
    push(
        "sensevoice",
        h.join("Library/Application Support/LumenAsr/models/sensevoice"),
        "lumen-asr",
    );
    for name in [
        "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17",
        "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17",
    ] {
        push(
            "sensevoice",
            h.join(".coli/models").join(name),
            "coli-cache",
        );
    }

    let app_wh = app_models_dir().join("whisper");
    push("whisper", app_wh, "app");
    if let Ok(p) = std::env::var("LUMEN_NAVI_WHISPER_DIR") {
        push("whisper", PathBuf::from(p), "env");
    }
    if let Ok(p) = std::env::var("LUMEN_WHISPER_DIR") {
        push("whisper", PathBuf::from(p), "env");
    }
    push(
        "whisper",
        h.join("Library/Application Support/LumenAsr/models/whisper"),
        "lumen-asr",
    );
    for name in [
        "sherpa-onnx-whisper-tiny.en",
        "sherpa-onnx-whisper-base.en",
    ] {
        push("whisper", h.join(".coli/models").join(name), "coli-cache");
    }

    let mut seen = std::collections::HashSet::new();
    out.retain(|c| seen.insert(c.path.clone()));
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
}
