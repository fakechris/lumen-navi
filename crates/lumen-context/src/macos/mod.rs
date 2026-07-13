mod ax;
mod capture;
mod frontmost;
mod lock;
mod screen_source;
mod vision;
mod vision_source;

pub(crate) use ax::default_ax_sources;
pub use capture::{MacDisplays, MacScreenCapturer};
pub use frontmost::{frontmost_app, MacFrontmost};
pub use lock::{is_screen_locked, MacScreenLock};
pub(crate) use screen_source::default_screen_sources;
pub use vision::{default_ocr_languages, MacVisionOcr};
pub(crate) use vision_source::default_vision_sources;
