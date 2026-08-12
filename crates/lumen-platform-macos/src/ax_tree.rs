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

use lumen_platform::{AxTreeSnapshot, AxTreeWalkConfig, PlatformError};

use crate::ax::{
    ensure_enhanced_ax_for_pid, ax_string_attr, AxError, AxUIElementRef, ReleaseGuard,
    AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementSetMessagingTimeout,
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
    async fn walk(&self, pid: i32, config: AxTreeWalkConfig) -> Result<AxTreeSnapshot, PlatformError> {
        let config = config.clone();
        let result = tokio::task::spawn_blocking(move || walk_focused_window(pid, &config))
            .await
            .map_err(|e| PlatformError::Message(format!("AX walk join: {e}")))?;
        result
    }

    fn is_supported(&self) -> bool {
        cfg!(target_os = "macos")
    }
}

/// Walk the focused window of `pid`'s application. Returns a flat text blob
/// of all extractable text in the tree, plus metadata.
pub fn walk_focused_window(pid: i32, config: &AxTreeWalkConfig) -> Result<AxTreeSnapshot, PlatformError> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, config);
        return Err(PlatformError::Message("AX tree walk requires macOS".into()));
    }
    #[cfg(target_os = "macos")]
    {
        // Force-enable Electron/Chromium AX (cached; only pokes once per 60s).
        if ensure_enhanced_ax_for_pid(pid) {
            // First poke for this pid — the tree materializes asynchronously.
            std::thread::sleep(Duration::from_millis(150));
        }

        // Wakeup retry: AX trees materialize lazily (especially web content).
        // If the first walk yields few nodes, retry up to 3 times with short
        // sleeps — mirrors yansu's poll-retry approach. Most of the cost is
        // the first walk (AX IPC); subsequent walks on the same materialized
        // tree are very fast.
        let mut best = objc2::rc::autoreleasepool(|_pool| unsafe {
            walk_focused_window_inner(pid, config)
        });
        const MIN_NODES: usize = 15;
        const MAX_RETRIES: u32 = 3;
        const RETRY_SLEEP: Duration = Duration::from_millis(50);

        for _ in 0..MAX_RETRIES {
            let nodes = best.as_ref().map(|s| s.node_count).unwrap_or(0);
            if nodes >= MIN_NODES {
                break;
            }
            std::thread::sleep(RETRY_SLEEP);
            let attempt = objc2::rc::autoreleasepool(|_pool| unsafe {
                walk_focused_window_inner(pid, config)
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
unsafe fn walk_focused_window_inner(
    pid: i32,
    config: &AxTreeWalkConfig,
) -> Result<AxTreeSnapshot, PlatformError> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    let start = Instant::now();
    let element_timeout = (config.element_timeout_ms as f64) / 1000.0;

    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Err(PlatformError::Message(format!(
            "AXUIElementCreateApplication({pid}) returned null"
        )));
    }
    AXUIElementSetMessagingTimeout(app, 0.5);
    let _app_guard = ReleaseGuard(app as *const c_void);

<<<<<<< HEAD
    tracing::debug!(pid, "walk_focused_window_inner: starting");

    // Use AXFocusedWindow directly (screenpipe's approach). Fall back to
    // AXWindows[0] if null. No child-count probing — some AX providers hang
    // on read_children.
=======
<<<<<<< HEAD
    tracing::debug!(pid, "walk_focused_window_inner: starting");

=======
>>>>>>> origin/main
    // Resolve the focused window with a 4-tier fallback (mirrors screenpipe's
    // resolve_focused_window). AXFocusedWindow can return a stale/ghost window
    // with only AXMenuBar as child (the real content window is a different
    // AXUIElement). If a candidate has ≤2 children, it's likely the wrong
    // window — try the next candidate.
<<<<<<< HEAD
    tracing::debug!(pid, "walk_inner: resolving window");
    // Use AXFocusedWindow directly — the 4-tier resolve_window with child-count
    // probing calls read_children on each candidate, and some apps' AX
    // providers hang on that call even with messaging timeout. If
    // AXFocusedWindow is null, fall back to a simple AXWindows[0].
>>>>>>> origin/main
    let window = {
        let attr = CFString::new("AXFocusedWindow");
        let mut value: core_foundation::base::CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value) != K_AX_SUCCESS || value.is_null() {
<<<<<<< HEAD
=======
            // Fallback: AXWindows[0]
>>>>>>> origin/main
            let wins_attr = CFString::new("AXWindows");
            let mut wins: core_foundation::base::CFTypeRef = std::ptr::null();
            if AXUIElementCopyAttributeValue(app, wins_attr.as_concrete_TypeRef(), &mut wins) == K_AX_SUCCESS && !wins.is_null() {
                let arr = wins as core_foundation_sys::array::CFArrayRef;
                let count = core_foundation_sys::array::CFArrayGetCount(arr);
                if count > 0 {
                    let v = core_foundation_sys::array::CFArrayGetValueAtIndex(arr, 0);
                    core_foundation_sys::base::CFRelease(wins);
                    v as AxUIElementRef
                } else {
                    core_foundation_sys::base::CFRelease(wins);
                    return Ok(AxTreeSnapshot {
                        text_content: String::new(), node_count: 0,
                        content_hash: String::new(), walk_duration_ms: start.elapsed().as_millis() as u64,
                        truncated: false, app_name: None, window_title: None,
                        document_path: None, browser_url: None,
                    });
                }
            } else {
                return Ok(AxTreeSnapshot {
                    text_content: String::new(), node_count: 0,
                    content_hash: String::new(), walk_duration_ms: start.elapsed().as_millis() as u64,
                    truncated: false, app_name: None, window_title: None,
                    document_path: None, browser_url: None,
                });
            }
        } else {
            value as AxUIElementRef
        }
    };
<<<<<<< HEAD
    let _win_guard = ReleaseGuard(window as *const c_void);
=======
=======
    let window = resolve_window(app, element_timeout)?;
>>>>>>> origin/main
    let _win_guard = ReleaseGuard(window as *const c_void);
    tracing::debug!(pid, "walk_inner: window resolved");
>>>>>>> origin/main

    // Read window-level metadata (cheap, no recursion).
    let window_title = ax_string_attr(window, "AXTitle");
    let app_name = ax_string_attr(app, "AXTitle");
    tracing::debug!(pid, title = ?window_title, "walk_inner: got metadata");
    let document_path = ax_string_attr(window, "AXDocument")
        .filter(|s| !s.is_empty())
        .map(decode_file_url);

    let mut walker = Walker {
        config,
        start,
        node_count: 0usize,
        truncated: false,
        text: String::with_capacity(8192),
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

        let role = ax_string_attr(element, "AXRole").unwrap_or_default();

        // Prune decorative subtrees entirely.
        if should_skip_role(&role) {
            return;
        }

        // Extract text from this node if its role warrants it.
        if should_extract_text(&role) {
            // Try AXTitle first, then AXValue, then AXDescription.
            for attr in &["AXTitle", "AXValue", "AXDescription"] {
                if let Some(t) = ax_string_attr(element, attr) {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        self.push_text(trimmed);
                        break;
                    }
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

/// Should we skip this role's subtree entirely? Decorative / non-text roles.
fn should_skip_role(role: &str) -> bool {
    SKIP_ROLES.iter().any(|r| r == &role)
}

/// Should we try to extract text from this role?
fn should_extract_text(role: &str) -> bool {
    TEXT_ROLES.iter().any(|r| r == &role) || role.is_empty()
}

#[cfg(target_os = "macos")]
/// Resolve the best window element for tree walking. Tries AXFocusedWindow
/// first, then AXMainWindow, then AXWindows[0], then the first AXWindow in
/// the app's AXChildren. If a candidate has ≤2 children (likely just
/// AXMenuBar — the "ghost window" problem on Safari), it falls through to
/// the next candidate. This mirrors screenpipe's `resolve_focused_window`.
#[cfg(target_os = "macos")]
unsafe fn resolve_window(app: AxUIElementRef, timeout: f64) -> Result<AxUIElementRef, PlatformError> {
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::CFString;

    // Collect candidates in priority order.
    let mut candidates: Vec<AxUIElementRef> = Vec::new();

    for attr_name in &["AXFocusedWindow", "AXMainWindow"] {
        let attr = CFString::new(attr_name);
        let mut value: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value) == K_AX_SUCCESS
            && !value.is_null()
        {
            candidates.push(value as AxUIElementRef);
        }
    }

    // AXWindows array — take first element.
    {
        let attr = CFString::new("AXWindows");
        let mut value: CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(app, attr.as_concrete_TypeRef(), &mut value) == K_AX_SUCCESS
            && !value.is_null()
        {
            if let Some(wins) = cf_array_to_vec(value as core_foundation_sys::array::CFArrayRef) {
                if let Some(first) = wins.into_iter().next() {
                    candidates.push(first);
                }
            }
            core_foundation_sys::base::CFRelease(value);
        }
    }

    // App's AXChildren — find first AXWindow.
    if let Some(app_children) = read_children(app) {
        for child in app_children {
            let role = ax_string_attr(child, "AXRole").unwrap_or_default();
            if role == "AXWindow" {
                candidates.push(child);
                break;
            }
        }
    }

    // Pick the first candidate with >2 children (the "ghost window" has only
    // AXMenuBar). Fall back to the first candidate if none qualify.
    let mut best = candidates.first().copied();
    for &cand in &candidates {
        AXUIElementSetMessagingTimeout(cand, timeout);
        if let Some(kids) = read_children(cand) {
            if kids.len() > 2 {
                best = Some(cand);
                break;
            }
        }
    }

    best.ok_or_else(|| PlatformError::Message("no resolvable window".into()))
}

/// Convert a CFArrayRef of AXUIElementRefs into a Vec. Does NOT release the
/// array (caller manages the array's lifetime).
#[cfg(target_os = "macos")]
unsafe fn cf_array_to_vec(arr: core_foundation_sys::array::CFArrayRef) -> Option<Vec<AxUIElementRef>> {
    if arr.is_null() {
        return None;
    }
    let count = core_foundation_sys::array::CFArrayGetCount(arr) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let v = core_foundation_sys::array::CFArrayGetValueAtIndex(arr, i as isize);
        if !v.is_null() {
            out.push(v as AxUIElementRef);
        }
    }
    Some(out)
}

/// Read the `AXChildren` attribute as a vector of AXUIElementRefs. Each child
/// is a retained reference the caller must release — but since we hold the
/// autorelease pool from `walk_focused_window`, the pool drains them.
unsafe fn read_children(element: AxUIElementRef) -> Option<Vec<AxUIElementRef>> {
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType};
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
    let children: Vec<AxUIElementRef> = array
        .iter()
        .map(|p| *p as AxUIElementRef)
        .filter(|p| !p.is_null())
        .collect();
    // array dropped here (create-rule Drop releases the CFArray — no manual
    // CFRelease needed, and a manual one would be a double-free).

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
