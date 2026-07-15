//! Local ASR model discovery + SenseVoice package download (onboarding).
//!
//! Models install under the **shared Lumen cluster** path so navi / asr /
//! future apps share one download:
//!   ~/Library/Application Support/Lumen/models/
//!
//! Users may still pick any ready path (legacy LumenAsr/Navi, coli, custom).
//! Selection is persisted in `navi.toml` (`asr.engine` / `asr.model_dir` /
//! optional `asr.models_root`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use lumen_asr_engine::{
    default_sensevoice_dir_with_root, default_whisper_dir_with_root, download_sensevoice_package,
    lumen_models_dir_with_override, scan_model_candidates_with_root, sensevoice_ready,
    whisper_ready, SENSEVOICE_ARCHIVE_URL,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::AppState;

static DOWNLOAD_CANCEL: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub struct AsrModelCandidate {
    pub engine: String,
    pub path: String,
    pub label: String,
    pub ready: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsrModelStatus {
    pub sensevoice_ready: bool,
    pub sensevoice_dir: String,
    pub whisper_ready: bool,
    pub whisper_dir: String,
    /// Shared cluster root used for download + default scan.
    pub models_root: String,
    pub active_engine: String,
    pub active_model_dir: String,
    pub candidates: Vec<AsrModelCandidate>,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsrDownloadProgress {
    pub phase: String,
    pub message: String,
    pub bytes: u64,
    pub total: Option<u64>,
    pub percent: Option<f32>,
}

fn models_root_from_cfg(cfg: &lumen_config::Config) -> Option<PathBuf> {
    cfg.asr.models_root_path()
}

fn candidates_for(cfg: &lumen_config::Config) -> Vec<AsrModelCandidate> {
    let root = models_root_from_cfg(cfg);
    scan_model_candidates_with_root(root.as_deref())
        .into_iter()
        .map(|c| AsrModelCandidate {
            engine: c.engine,
            path: c.path,
            label: c.label,
            ready: c.ready,
            source: c.source,
        })
        .collect()
}

fn status_from_config(state: &AppState) -> Result<AsrModelStatus, String> {
    let cfg = state.load_config().map_err(|e| e.to_string())?;
    let root = models_root_from_cfg(&cfg);
    let root_path = lumen_models_dir_with_override(root.as_deref());

    let sv = if !cfg.asr.model_dir.is_empty()
        && (cfg.asr.engine.contains("sensevoice") || cfg.asr.engine.is_empty())
    {
        PathBuf::from(&cfg.asr.model_dir)
    } else {
        default_sensevoice_dir_with_root(root.as_deref())
    };
    let wh = if !cfg.asr.model_dir.is_empty() && cfg.asr.engine.contains("whisper") {
        PathBuf::from(&cfg.asr.model_dir)
    } else {
        default_whisper_dir_with_root(root.as_deref())
    };

    let sensevoice_dir = if sensevoice_ready(&sv) {
        sv
    } else {
        default_sensevoice_dir_with_root(root.as_deref())
    };
    let whisper_dir = if whisper_ready(&wh) {
        wh
    } else {
        default_whisper_dir_with_root(root.as_deref())
    };

    Ok(AsrModelStatus {
        sensevoice_ready: sensevoice_ready(&sensevoice_dir),
        sensevoice_dir: sensevoice_dir.display().to_string(),
        whisper_ready: whisper_ready(&whisper_dir),
        whisper_dir: whisper_dir.display().to_string(),
        models_root: root_path.display().to_string(),
        active_engine: cfg.asr.engine.clone(),
        active_model_dir: cfg.asr.model_dir.clone(),
        candidates: candidates_for(&cfg),
        download_url: SENSEVOICE_ARCHIVE_URL.into(),
    })
}

#[tauri::command]
pub fn check_asr_model_status(state: State<'_, AppState>) -> Result<AsrModelStatus, String> {
    status_from_config(&state)
}

#[tauri::command]
pub fn list_local_asr_models(state: State<'_, AppState>) -> Result<Vec<AsrModelCandidate>, String> {
    let cfg = state.load_config().map_err(|e| e.to_string())?;
    Ok(candidates_for(&cfg))
}

#[derive(Debug, Deserialize)]
pub struct UseAsrModelInput {
    pub path: String,
    pub engine: Option<String>,
}

/// Point daemon config at an existing model directory (any path the user chooses).
#[tauri::command]
pub fn use_existing_asr_model(
    state: State<'_, AppState>,
    input: UseAsrModelInput,
) -> Result<AsrModelStatus, String> {
    let path = PathBuf::from(input.path.trim());
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()));
    }
    let engine = input
        .engine
        .unwrap_or_else(|| "sensevoice".into())
        .trim()
        .to_ascii_lowercase();

    match engine.as_str() {
        "whisper" => {
            if !whisper_ready(&path) {
                return Err(
                    "folder is not a valid Whisper (sherpa) model dir (encoder/decoder/tokens)"
                        .into(),
                );
            }
        }
        "sensevoice" | "sensevoice_sherpa" | "sherpa" => {
            if !sensevoice_ready(&path) {
                return Err(
                    "folder is not a valid SenseVoice model dir (need model*.onnx + tokens.txt)"
                        .into(),
                );
            }
        }
        other => {
            return Err(format!(
                "use_existing_asr_model only supports sensevoice|whisper (got {other})"
            ));
        }
    }

    let mut cfg = state.load_config().map_err(|e| e.to_string())?;
    cfg.asr.engine = if engine.starts_with("sense") {
        "sensevoice".into()
    } else {
        "whisper".into()
    };
    cfg.asr.model_dir = path.display().to_string();
    cfg.asr.enabled = true;
    state.save_config(&cfg).map_err(|e| e.to_string())?;
    tracing::info!(
        path = %path.display(),
        engine = %cfg.asr.engine,
        "ASR model path selected for Navi (user choice)"
    );
    status_from_config(&state)
}

/// Select engine without requiring a local model path (e.g. speech).
#[tauri::command]
pub fn set_asr_engine_preference(
    state: State<'_, AppState>,
    engine: String,
) -> Result<AsrModelStatus, String> {
    let eng = engine.trim().to_ascii_lowercase();
    let allowed = [
        "sensevoice",
        "whisper",
        "speech",
        "openai_audio",
        "qwen",
    ];
    if !allowed.iter().any(|a| *a == eng.as_str()) {
        return Err(format!(
            "unsupported engine '{eng}' (want sensevoice|whisper|speech|openai_audio|qwen)"
        ));
    }
    let mut cfg = state.load_config().map_err(|e| e.to_string())?;
    cfg.asr.engine = eng;
    cfg.asr.enabled = true;
    state.save_config(&cfg).map_err(|e| e.to_string())?;
    status_from_config(&state)
}

/// Override the shared cluster models root (empty = platform default Lumen/models).
#[tauri::command]
pub fn set_asr_models_root(
    state: State<'_, AppState>,
    models_root: String,
) -> Result<AsrModelStatus, String> {
    let mut cfg = state.load_config().map_err(|e| e.to_string())?;
    let t = models_root.trim();
    if !t.is_empty() {
        let p = PathBuf::from(t);
        if p.exists() && !p.is_dir() {
            return Err(format!("models_root is not a directory: {t}"));
        }
        // Create if missing so download can use it.
        std::fs::create_dir_all(&p).map_err(|e| format!("create models_root: {e}"))?;
        cfg.asr.models_root = p.display().to_string();
    } else {
        cfg.asr.models_root.clear();
    }
    state.save_config(&cfg).map_err(|e| e.to_string())?;
    status_from_config(&state)
}

#[tauri::command]
pub fn cancel_asr_model_download() -> Result<(), String> {
    DOWNLOAD_CANCEL.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn start_asr_model_download(app: AppHandle) -> Result<AsrModelStatus, String> {
    if DOWNLOAD_RUNNING.swap(true, Ordering::SeqCst) {
        return Err("download already running".into());
    }
    DOWNLOAD_CANCEL.store(false, Ordering::SeqCst);

    let state = app.state::<AppState>();
    let cfg = state.load_config().map_err(|e| e.to_string())?;
    let root = lumen_models_dir_with_override(cfg.asr.models_root_path().as_deref());

    let app_for_dl = app.clone();
    let root_for_dl = root.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        download_sensevoice_package(&root_for_dl, &DOWNLOAD_CANCEL, |p| {
            let percent = p.total.map(|t| {
                if t == 0 {
                    0.0
                } else {
                    (p.bytes as f32 / t as f32) * 100.0
                }
            });
            let _ = app_for_dl.emit(
                "asr-download-progress",
                AsrDownloadProgress {
                    phase: p.phase,
                    message: p.message,
                    bytes: p.bytes,
                    total: p.total,
                    percent,
                },
            );
        })
    })
    .await;
    DOWNLOAD_RUNNING.store(false, Ordering::SeqCst);

    match result {
        Ok(Ok(dir)) => {
            let state = app.state::<AppState>();
            let mut cfg = state.load_config().map_err(|e| e.to_string())?;
            cfg.asr.engine = "sensevoice".into();
            cfg.asr.model_dir = dir.display().to_string();
            // Persist resolved shared root if user had none (makes multi-app sharing explicit).
            if cfg.asr.models_root.trim().is_empty() {
                cfg.asr.models_root = root.display().to_string();
            }
            cfg.asr.enabled = true;
            state.save_config(&cfg).map_err(|e| e.to_string())?;
            tracing::info!(
                dir = %dir.display(),
                models_root = %root.display(),
                "SenseVoice installed under shared Lumen models root"
            );
            status_from_config(&state)
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(format!("download task failed: {e}")),
    }
}
