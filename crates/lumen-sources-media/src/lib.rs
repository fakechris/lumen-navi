//! Media intake — product Observe capture orchestrator.
//!
//! See `docs/OBSERVE_CAPTURE.md` (screen) and `docs/AUDIO_PRODUCT.md` (mic).
//! OCR is intentionally out of scope here.

mod audio;
mod orchestrator;
mod session;

pub use audio::{
    synthetic_silence_chunk, synthetic_tone_chunk, AudioOrchestrator, AudioStats, CapturedAudio,
};
pub use orchestrator::{CaptureOrchestrator, CaptureStats, CapturedBatch};
pub use session::SessionManager;
