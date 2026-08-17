//! Recursive AX-tree walker for deep accessibility-text capture.
//!
//! Walks the focused window's AX tree, extracts text from meaningful nodes
//! (buttons, text fields, static text, cells, headings, …), prunes decorative
//! roles (scroll bars, images, toolbars), and resets depth at `AXWebArea` so
//! Electron/Chromium shell layers don't consume the depth budget before
//! reaching actual app content.
//!
//! Algorithm adapted from screenpipe's `MacosTreeWalker::walk_focused_window`
//! (`crates/screenpipe-a11y/src/tree/macos.rs`), ported to Lumen Navi's raw
//! `core-foundation` FFI (no `cidre` dependency).
//!
//! Output: a flat text blob (for FTS search) + metadata. Structured node
//! storage is iteration 2.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use lumen_platform::{AxHit, AxTreeSnapshot, AxTreeWalkConfig, PlatformError};

use crate::ax::{
    ax_point_attr, ax_size_attr, ax_string_attr, ensure_enhanced_ax_for_pid, AxError,
    AxUIElementRef, ReleaseGuard, AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
    AXUIElementSetMessagingTimeout, _AXUIElementGetWindow,
};

/// kAXErrorSuccess
const K_AX_SUCCESS: AxError = 0;

/// Roles whose subtrees contain no useful text — skip them entirely.
/// Ported from screenpipe `should_skip_role` (`tree/macos.rs:1318`).
const SKIP_ROLES: &[&str] = &[
    "AXScrollBar",
    "AXImage",
    "AXSplitter",
    "AXGrowArea",
    "AXMenu",
    "AXMenuBar",
    "AXMenuBarItem",
    "AXToolbar",
    "AXUnknown",
    "AXSlider",
    "AXProgressIndicator",
    "AXBusyIndicator",
    "AXHandle",
    "AXHelpTag",
    "AXOutline",
    "AXColumn",
    "AXStaticTextMount",
];

/// Roles worth extracting text from (via AXTitle or AXValue). Ported from
/// screenpipe `should_extract_text` (`tree/macos.rs:1338`).
const TEXT_ROLES: &[&str] = &[
    "AXStaticText",
    "AXTextField",
    "AXTextArea",
    "AXButton",
    "AXMenuItem",
    "AXCell",
    "AXHeading",
    "AXLink",
    "AXPopUpButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXTab",
    "AXMenuItemCheckBox",
    "AXMenuItemRadio",
    "AXComboBox",
    "AXSearchField",
    "AXList",
    "AXRow",
    "AXWindow",
    "AXWebArea",
];

/// macOS implementation of the `AxTreeWalker` platform trait.
pub struct MacAxTreeWalker;

#[async_trait::async_trait]
impl lumen_platform::AxTreeWalker for MacAxTreeWalker {
    async fn walk(
        &self,
        pid: i32,
        window_id: Option<u64>,
        config: AxTreeWalkConfig,
    ) -> Result<AxTreeSnapshot, PlatformError> {
        let config = config.clone();
        let result = tokio::task::spawn_blocking(move || walk_window(pid, window_id, &config))
            .await
            .map_err(|e| PlatformError::Message(format!("AX walk join: {e}")))?;
        result
    }

    fn is_supported(&self) -> bool {
        cfg!(target_os = "macos")
    }
}

/// Walk the focused window of `pid`. Prefer [`walk_window`] when a capture-time
/// `window_id` is available.
pub fn walk_focused_window(pid: i32, config: &AxTreeWalkConfig) -> Result<AxTreeSnapshot, PlatformError> {
    walk_window(pid, None, config)
}

/// Walk `pid`'s AX tree. If `window_id` is `Some` and that CG window is gone,
/// returns [`PlatformError::WindowGone`]. Title mismatch is not a failure.
pub fn walk_window(
    pid: i32,
    window_id: Option<u64>,
    config: &AxTreeWalkConfig,
) -> Result<AxTreeSnapshot, PlatformError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, window_id, config);
        return Err(PlatformError::Message("AX tree walk requires macOS".into()));
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(id) = window_id {
            if !crate::frontmost::cg_window_exists(id) {
                return Err(PlatformError::WindowGone(id));
            }
        }
        // Force-enable Electron/Chromium AX (cached; only pokes once per 60s).
        if ensure_enhanced_ax_for_pid(pid) {
            std::thread::sleep(Duration::from_millis(150));
        }

        let mut best = objc2::rc::autoreleasepool(|_pool| unsafe {
            walk_window_inner(pid, window_id, config)
        });
        const MIN_NODES: usize = 15;
        const MAX_RETRIES: u32 = 3;
        const RETRY_SLEEP: Duration = Duration::from_millis(50);

        for _ in 0..MAX_RETRIES {
            if matches!(&best, Err(PlatformError::WindowGone(_))) {
                break;
            }
            let nodes = best.as_ref().map(|s| s.node_count).unwrap_or(0);
            if nodes >= MIN_NODES {
                break;
            }
            std::thread::sleep(RETRY_SLEEP);
            let attempt = objc2::rc::autoreleasepool(|_pool| unsafe {
                walk_window_inner(pid, window_id, config)
            });
            if let Ok(snap) = &attempt {
                if snap.node_count > nodes {
                    best = attempt;
                }
            }
        }
        best
    }
}

#[cfg(target_os = "macos")]
unsafe fn find_window_by_cg_id(app: AxUIElementRef, want: u32) -> Option<AxUIElementRef> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let wins_attr = CFString::new("AXWindows");
    let mut wins: core_foundation::base::CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(app, wins_attr.as_concrete_TypeRef(), &mut wins) != K_AX_SUCCESS
        || wins.is_null()
    {
        return None;
    }
    let arr = wins as core_foundation_sys::array::CFArrayRef;
    let count = core_foundation_sys::array::CFArrayGetCount(arr);
    for i in 0..count {
        let v = core_foundation_sys::array::CFArrayGetValueAtIndex(arr, i);
        if v.is_null() {
            continue;
        }
        let mut cgid: u32 = 0;
        if _AXUIElementGetWindow(v as AxUIElementRef, &mut cgid) == K_AX_SUCCESS && cgid == want {
            core_foundation_sys::base::CFRetain(v);
            core_foundation_sys::base::CFRelease(wins);
            return Some(v as AxUIElementRef);
        }
    }
    core_foundation_sys::base::CFRelease(wins);
    None
}

#[cfg(target_os = "macos")]
unsafe fn resolve_focused_window(app: AxUIElementRef) -> Option<AxUIElementRef> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let attr = CFString::new("AXFocusedWindow");
    let mut value: core_foundation::base::CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value) == K_AX_SUCCESS
        && !value.is_null()
    {
        return Some(value as AxUIElementRef);
    }
    let wins_attr = CFString::new("AXWindows");
    let mut wins: core_foundation::base::CFTypeRef = std::ptr::null();
    if AXUIElementCopyAttributeValue(app, wins_attr.as_concrete_TypeRef(), &mut wins) != K_AX_SUCCESS
        || wins.is_null()
    {
        return None;
    }
    let arr = wins as core_foundation_sys::array::CFArrayRef;
    let count = core_foundation_sys::array::CFArrayGetCount(arr);
    if count == 0 {
        core_foundation_sys::base::CFRelease(wins);
        return None;
    }
    let v = core_foundation_sys::array::CFArrayGetValueAtIndex(arr, 0);
    if v.is_null() {
        core_foundation_sys::base::CFRelease(wins);
        return None;
    }
    core_foundation_sys::base::CFRetain(v);
    core_foundation_sys::base::CFRelease(wins);
    Some(v as AxUIElementRef)
}

#[cfg(target_os = "macos")]
unsafe fn walk_window_inner(
    pid: i32,
    window_id: Option<u64>,
    config: &AxTreeWalkConfig,
) -> Result<AxTreeSnapshot, PlatformError> {
    let start = Instant::now();

    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(PlatformError::Message(format!(
            "AXUIElementCreateApplication({pid}) returned null"
        )));
    }
    AXUIElementSetMessagingTimeout(app, 0.5);
    let _app_guard = ReleaseGuard(app as *const c_void);

    // Prefer the capture-time CG window. If AX can't map it but the window
    // still exists, fall back to focused — do not treat title drift as gone.
    let window = if let Some(id) = window_id {
        if let Some(w) = find_window_by_cg_id(app, id as u32) {
            w
        } else {
            resolve_focused_window(app).unwrap_or(std::ptr::null())
        }
    } else {
        resolve_focused_window(app).unwrap_or(std::ptr::null())
    };
    if window.is_null() {
        return Ok(AxTreeSnapshot {
            text_content: String::new(),
            node_count: 0,
            content_hash: String::new(),
            walk_duration_ms: start.elapsed().as_millis() as u64,
            truncated: false,
            app_name: None,
            window_title: None,
            document_path: None,
            browser_url: None,
            hits: Vec::new(),
            window_bounds: None,
        });
    }
    let _win_guard = ReleaseGuard(window as *const c_void);

    // Read window-level metadata (cheap, no recursion).
    let window_title = ax_string_attr(window, "AXTitle");
    let app_name = ax_string_attr(app, "AXTitle");
    tracing::debug!(pid, title = ?window_title, "walk_inner: got metadata");
    let document_path = ax_string_attr(window, "AXDocument")
        .filter(|s| !s.is_empty())
        .map(decode_file_url);

    let window_bounds = ax_frame(window);
    let mut walker = Walker {
        config,
        start,
        node_count: 0usize,
        truncated: false,
        text: String::with_capacity(8192),
        hits: Vec::new(),
    };

    tracing::debug!(pid, "walk_inner: starting walk_element");
    walker.walk_element(window, 0);
    tracing::debug!(pid, nodes = walker.node_count, text_len = walker.text.len(), "walk_inner: walk_element done");

    let walk_duration = start.elapsed();
    let content_hash = blake3_hash(&walker.text);

    Ok(AxTreeSnapshot {
        // Trim to max_text_length.
        text_content: trim_text(walker.text, config.max_text_length),
        node_count: walker.node_count,
        content_hash,
        walk_duration_ms: walk_duration.as_millis().max(1) as u64,
        truncated: walker.truncated,
        app_name,
        window_title,
        document_path,
        browser_url: None, // iteration 2: AXWebArea→AXURL
        hits: walker.hits,
        window_bounds,
    })
}

/// The recursive walker state.
#[cfg(target_os = "macos")]
struct Walker<'a> {
    config: &'a AxTreeWalkConfig,
    start: Instant,
    node_count: usize,
    truncated: bool,
    text: String,
    hits: Vec<AxHit>,
}

#[cfg(target_os = "macos")]
impl<'a> Walker<'a> {
    /// Process one element: extract its text (if its role warrants it), then
    /// recurse into its children — unless the role is pruned or we've hit a
    /// budget cap.
    unsafe fn walk_element(&mut self, element: AxUIElementRef, depth: usize) {
        // Budget checks first.
        if self.node_count >= self.config.max_nodes as usize {
            self.truncated = true;
            return;
        }
        if self.start.elapsed() > Duration::from_millis(self.config.walk_timeout_ms) {
            self.truncated = true;
            return;
        }
        if depth > self.config.max_depth as usize {
            return;
        }
        self.node_count += 1;

        // Set messaging timeout on EVERY element — AXUIElementSetMessagingTimeout
        // is per-element and does NOT inherit to children. Without this, deep
        // nodes (Safari's AXSplitGroup → AXWebArea chain) can hang indefinitely
        // waiting for the target app's main thread to respond.
        AXUIElementSetMessagingTimeout(element, 0.3);

        let role = ax_string_attr(element, "AXRole").unwrap_or_default();

        // Prune decorative subtrees entirely.
        if should_skip_role(&role) {
            return;
        }

        // Extract text from this node if its role warrants it.
        if should_extract_text(&role) {
            // Try AXTitle first, then AXValue, then AXDescription.
            let mut title = None;
            for attr in &["AXTitle", "AXValue", "AXDescription"] {
                if let Some(t) = ax_string_attr(element, attr) {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        self.push_text(trimmed);
                        title = Some(trimmed.to_string());
                        break;
                    }
                }
            }
            if let (Some(title), Some(frame)) = (title, ax_frame(element)) {
                if self.hits.len() < 80 && frame.w >= 2.0 && frame.h >= 2.0 {
                    self.hits.push(AxHit {
                        role: role.clone(),
                        title: title.chars().take(80).collect(),
                        x: frame.x,
                        y: frame.y,
                        w: frame.w,
                        h: frame.h,
                    });
                }
            }
        }

        // Recurse into children.
        if let Some(children) = read_children(element) {
            // AXWebArea depth reset: Electron/Chromium shell layers above the
            // web area consume depth budget without contributing content.
            // Reset to 0 so the DOM tree underneath gets the full budget.
            let child_depth = if role == "AXWebArea" { 0 } else { depth + 1 };
            for child in &children {
                self.walk_element(*child, child_depth);
            }
            // read_children retained each element (+1); release them now that
            // every subtree has been walked.
            for child in children {
                core_foundation_sys::base::CFRelease(child);
            }
        }
    }

    fn push_text(&mut self, s: &str) {
        if !self.text.is_empty() {
            self.text.push(' ');
        }
        // Guard against pathological single-node values.
        let max_node = self.config.max_text_length.min(2000) as usize;
        if s.len() > max_node {
            self.text.push_str(&s[..max_node]);
        } else {
            self.text.push_str(s);
        }
    }
}

fn ax_frame(element: AxUIElementRef) -> Option<AxHit> {
    unsafe {
        let (x, y) = ax_point_attr(element, "AXPosition")?;
        let (w, h) = ax_size_attr(element, "AXSize")?;
        Some(AxHit {
            role: String::new(),
            title: String::new(),
            x,
            y,
            w,
            h,
        })
    }
}

/// Should we skip this role's subtree entirely? Decorative / non-text roles.
fn should_skip_role(role: &str) -> bool {
    SKIP_ROLES.iter().any(|r| r == &role)
}

/// Should we try to extract text from this role?
fn should_extract_text(role: &str) -> bool {
    TEXT_ROLES.iter().any(|r| r == &role) || role.is_empty()
}

/// Read the `AXChildren` attribute as a vector of AXUIElementRefs. Each
/// returned element is **retained (+1)** — the caller must `CFRelease` every
/// element when done with it.
///
/// Ownership note: `CFArray::iter()` yields Get-rule borrows. Dropping the
/// array (create-rule Drop) releases its children, so returning the bare
/// pointers without retaining them leaves the caller with dangling
/// AXUIElementRefs — this was the production SIGSEGV in `walk_element`.
/// (An earlier comment here claimed "the autorelease pool drains them";
/// AXUIElementRefs from AXUIElementCopyAttributeValue are NOT autoreleased,
/// they are +1 owned by the array.)
unsafe fn read_children(element: AxUIElementRef) -> Option<Vec<AxUIElementRef>> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType};
    use core_foundation::string::CFString;

    let attr = CFString::new("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != K_AX_SUCCESS || value.is_null() {
        tracing::trace!(err, "read_children: AXChildren copy failed or null");
        return None;
    }

    // Type-check: must be a CFArray.
    let arr_type_id = core_foundation_sys::array::CFArrayGetTypeID();
    if CFGetTypeID(value) != arr_type_id {
        CFRelease(value);
        return None;
    }

    let array = CFArray::<*const c_void>::wrap_under_create_rule(value as CFArrayRef);
    // Retain each child while the array is still alive; the array's Drop
    // then only balances our retains, leaving every returned pointer valid.
    let children: Vec<AxUIElementRef> = array
        .iter()
        .map(|p| {
            let elem = *p as AxUIElementRef;
            if !elem.is_null() {
                // Retain each child BEFORE the array is dropped. The CFArray
                // holds the only reference; without this retain, CFRelease(array)
                // would free the children → dangling pointers → SIGSEGV.
                CFRetain(elem as *const c_void);
            }
            elem
        })
        .filter(|p| !p.is_null())
        .map(|p| {
            core_foundation_sys::base::CFRetain(p);
            p
        })
        .collect();
    // array dropped here — CFRelease(array) is safe because each child has +1.

    if children.is_empty() {
        tracing::trace!("read_children: AXChildren array was empty");
        None
    } else {
        tracing::trace!(count = children.len(), "read_children: got children");
        Some(children)
    }
}

/// Decode a `file://` URL into a POSIX path. Ported from screenpipe's
/// `extract_document_path` (`tree/macos.rs:176`).
fn decode_file_url(url: String) -> String {
    let stripped = url
        .strip_prefix("file://")
        .or_else(|| url.strip_prefix("file:"))
        .unwrap_or(&url);
    // Percent-decode (%20 → space, etc.).
    let decoded = percent_decode(stripped);
    // Strip a leading host segment if present (file://localhost/Users → /Users).
    // A POSIX path starts with '/'; if there's a host, the first segment before
    // '/' is the hostname. Simple heuristic: if the path doesn't start with '/',
    // drop everything up to and including the first '/'.
    if let Some(pos) = decoded.find('/') {
        if pos > 0 {
            return decoded[pos..].to_string();
        }
    }
    decoded
}

/// Minimal percent-decoding for file URLs (handles %20 etc.).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// blake3 hash of the text content, hex-encoded.
fn blake3_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

/// Trim text to `max_chars` (Unicode-safe), appending an ellipsis if truncated.
fn trim_text(mut text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    text.truncate(text.char_indices().nth(max_chars).map(|(i, _)| i).unwrap_or(text.len()));
    text.push_str("…[truncated]");
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_roles_are_correct() {
        assert!(should_skip_role("AXScrollBar"));
        assert!(should_skip_role("AXToolbar"));
        assert!(!should_skip_role("AXButton"));
        assert!(!should_skip_role("AXStaticText"));
    }

    #[test]
    fn extract_roles_are_correct() {
        assert!(should_extract_text("AXButton"));
        assert!(should_extract_text("AXStaticText"));
        assert!(!should_extract_text("AXScrollBar"));
    }

    #[test]
    fn decode_file_url_strips_scheme_and_host() {
        assert_eq!(
            decode_file_url("file:///Users/chris/src/main.rs".into()),
            "/Users/chris/src/main.rs"
        );
        assert_eq!(
            decode_file_url("file://localhost/Users/chris/x.md".into()),
            "/Users/chris/x.md"
        );
    }

    #[test]
    fn percent_decode_handles_common_cases() {
        assert_eq!(percent_decode("/Users/chris/my%20docs/x.txt"), "/Users/chris/my docs/x.txt");
        assert_eq!(percent_decode("/plain/path"), "/plain/path");
    }

    #[test]
    fn trim_text_is_unicode_safe() {
        let s = "你好世界test"; // 4 CJK + 4 ASCII = 8 chars
        let trimmed = trim_text(s.to_string(), 5);
        // 5 chars kept + "…[truncated]" suffix (12 chars) = 17 total.
        assert_eq!(trimmed.chars().count(), 5 + "…[truncated]".chars().count());
        assert!(trimmed.starts_with("你好世界t")); // first 5 chars = 4 CJK + 1 ASCII
    }
}
