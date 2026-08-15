//! Media intake — product Observe capture orchestrator.
//!
//! See `docs/OBSERVE_CAPTURE.md` (screen) and `docs/AUDIO_PRODUCT.md` (mic).
//! OCR is intentionally out of scope here.

mod activity;
mod audio;
mod interaction;
mod orchestrator;
mod session;

pub use activity::{ActivityAccumulator, ActivitySample, ActivityTick};
pub use audio::{
    synthetic_silence_chunk, synthetic_tone_chunk, AudioOrchestrator, AudioStats, CapturedAudio,
};
pub use interaction::{InteractionCoalescer, InteractionContext};
pub use orchestrator::{ActivityPoll, CaptureOrchestrator, CaptureStats, CapturedBatch};
pub use session::{
    drain_transition, session_matches_frontmost, SessionManager, SessionTransition,
    SharedSessionBinder,
};
