//! Process DPI awareness.
//!
//! Without this, Win32 reports virtualized (scaled) monitor rects and GDI
//! blits a stretched, blurry copy of the desktop on any display above 100%
//! scaling. Per-monitor v2 gives physical pixels on every monitor.

use std::sync::Once;

static INIT: Once = Once::new();

/// Idempotently opt the process into per-monitor-v2 DPI awareness.
///
/// Must run before the first display enumeration or capture. A failure means
/// awareness was already set (by a manifest, or by Tauri/WebView2), which is
/// fine — it is not an error worth surfacing.
pub fn ensure_process_dpi_aware() {
    INIT.call_once(|| {
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::UI::HiDpi::{
                SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            };
            unsafe {
                if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                    .is_err()
                {
                    tracing::debug!("per-monitor-v2 DPI awareness already set");
                }
            }
        }
    });
}
