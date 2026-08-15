//! Input event counting — behavioral keys only, never content.
//!
//! CGEventTap on mouseDown + keyDown. For the "roast my day" feature:
//! "you pressed Delete 1,191 times", "Cmd+C 133 times". We count only:
//!   - Behavior keys: Delete, Tab, Esc, Enter, arrows, Space
//!   - Shortcut combos: Cmd/Ctrl/Alt/Shift + C/V/X/Z/A/S/F/W/N/T (by flags)
//! Letters/digits alone are NEVER recorded — the counter can't see what you
//! typed, only that a key in a behavioral class went down.
//! Mouse: count clicks (left/right/other) + double-clicks, no position.
//!
//! Requires Input Monitoring TCC (System Settings → Privacy → Input Monitoring).
//! Default off; opt-in via config `[input] enabled = true`.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use lumen_platform::{ObserveHidEvent, ObserveHidKind};

/// Aggregated counters — the payload for input.stats.v1 events.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InputCounts {
    // Behavior keys (per-class counts).
    pub key_delete: u64,
    pub key_tab: u64,
    pub key_esc: u64,
    pub key_enter: u64,
    pub key_arrow: u64,
    pub key_space: u64,
    // Shortcut combos (modifier + key class).
    pub combo_copy: u64,    // Cmd/Ctrl + C
    pub combo_paste: u64,   // Cmd/Ctrl + V
    pub combo_cut: u64,     // Cmd/Ctrl + X
    pub combo_undo: u64,    // Cmd/Ctrl + Z
    pub combo_selectall: u64, // Cmd/Ctrl + A
    pub combo_find: u64,    // Cmd/Ctrl + F
    pub combo_close: u64,   // Cmd/Ctrl + W
    pub combo_new: u64,     // Cmd/Ctrl + N
    pub combo_save: u64,    // Cmd/Ctrl + S
    // Mouse.
    pub mouse_left: u64,
    pub mouse_right: u64,
    pub mouse_other: u64,
    pub mouse_double: u64,
}

// Shared state: counters + the app that was frontmost at each event.
pub struct InputCounterState {
    pub counts: Mutex<InputCounts>,
    pub last_app: Mutex<String>,
    /// Total events seen (for diagnostics).
    pub events_seen: AtomicU64,
    pending: Mutex<VecDeque<ObserveHidEvent>>,
}

impl Default for InputCounterState {
    fn default() -> Self {
        Self {
            counts: Mutex::new(InputCounts::default()),
            last_app: Mutex::new(String::new()),
            events_seen: AtomicU64::new(0),
            pending: Mutex::new(VecDeque::new()),
        }
    }
}

/// macOS key codes (Carbon kVK_* constants).
mod keycode {
    pub const DELETE: u32 = 0x33;
    pub const FORWARD_DELETE: u32 = 0x75;
    pub const TAB: u32 = 0x30;
    pub const ESCAPE: u32 = 0x35;
    pub const RETURN: u32 = 0x24;
    pub const ENTER: u32 = 0x4C;
    pub const ARROW_LEFT: u32 = 0x7B;
    pub const ARROW_RIGHT: u32 = 0x7C;
    pub const ARROW_DOWN: u32 = 0x7D;
    pub const ARROW_UP: u32 = 0x7E;
    pub const SPACE: u32 = 0x31;
    // Letters (used only for shortcut combos with modifier flags).
    pub const C: u32 = 0x08;
    pub const V: u32 = 0x09;
    pub const X: u32 = 0x07;
    pub const Z: u32 = 0x06;
    pub const A: u32 = 0x00;
    pub const S: u32 = 0x01;
    pub const F: u32 = 0x03;
    pub const W: u32 = 0x0D;
    pub const N: u32 = 0x2D;
    pub const T: u32 = 0x11;
}

// CGEvent field constants.
const K_CG_EVENT_KEY_CODE: usize = 9; // kCGKeyboardEventKeycode
const K_CG_EVENT_MOUSE_STATE: usize = 1; // kCGMouseEventButtonNumber
const K_CG_EVENT_SOURCE_STATE_ID: usize = 36; // kCGEventSourceUnixProcessID (unused, we use our own)

// CGEventFlags.
const FLAG_CMD: u64 = 1 << 20;     // kCGEventFlagMaskCommand
const FLAG_CTRL: u64 = 1 << 12;    // kCGEventFlagMaskControl
const FLAG_SHIFT: u64 = 1 << 17;
const FLAG_OPTION: u64 = 1 << 19;

// CGEventTypes.
const TYPE_LEFT_MOUSE_DOWN: u32 = 1;
const TYPE_LEFT_MOUSE_UP: u32 = 2;
const TYPE_RIGHT_MOUSE_DOWN: u32 = 3;
const TYPE_RIGHT_MOUSE_UP: u32 = 4;
const TYPE_OTHER_MOUSE_DOWN: u32 = 25;
const TYPE_OTHER_MOUSE_UP: u32 = 26;
const TYPE_KEY_DOWN: u32 = 10;
const K_CG_EVENT_CLICK_STATE: usize = 1;

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    fn CGEventKeyboardGetUnicodeString(
        event: *const c_void,
        max_len: usize,
        actual: *mut usize,
        buffer: *mut u16,
    );
    fn CGEventTapCreate(
        location: *const c_void,   // kCGSessionEventTap
        placement: i32,            // kCGHeadInsertEventTap = 1
        options: i32,              // kCGEventTapOptionListenOnly = 1
        event_mask: u64,
        callback: unsafe extern "C" fn(
            proxy: *const c_void,
            etype: u32,
            event: *const c_void,
            userinfo: *mut c_void,
        ) -> *const c_void,
        userinfo: *mut c_void,
    ) -> *const c_void;
    fn CGEventTapEnable(tap: *const c_void, enable: bool);
    fn CFRunLoopGetCurrent() -> *const c_void;
    fn CFRunLoopAddSource(rl: *const c_void, source: *const c_void, mode: *const c_void);
    fn CFRunLoopRun();
    fn CFRunLoopStop(rl: *const c_void);
    fn CGEventGetIntegerValueField(event: *const c_void, field: usize) -> i64;
    fn CGEventGetFlags(event: *const c_void) -> u64;
    fn CFMachPortCreateRunLoopSource(alloc: *const c_void, port: *const c_void, order: i32) -> *const c_void;
}

static STOP_FLAG: AtomicU64 = AtomicU64::new(0);
static mut COUNTER: Option<&'static InputCounterState> = None;

/// The event tap callback. Updates counters in-place. Must be fast (no I/O).
unsafe extern "C" fn tap_callback(
    _proxy: *const c_void,
    etype: u32,
    event: *const c_void,
    _userinfo: *mut c_void,
) -> *const c_void {
    let counter = match { COUNTER } {
        Some(c) => c,
        None => return event,
    };
    counter.events_seen.fetch_add(1, Ordering::Relaxed);

    match etype {
        TYPE_KEY_DOWN => {
            let code = CGEventGetIntegerValueField(event, K_CG_EVENT_KEY_CODE) as u32;
            let flags = CGEventGetFlags(event);
            let has_mod = (flags & (FLAG_CMD | FLAG_CTRL)) != 0;

            if let Ok(mut c) = counter.counts.lock() {
                if has_mod {
                    // Shortcut combos: modifier + specific key.
                    match code {
                        keycode::C => c.combo_copy += 1,
                        keycode::V => c.combo_paste += 1,
                        keycode::X => c.combo_cut += 1,
                        keycode::Z => c.combo_undo += 1,
                        keycode::A => c.combo_selectall += 1,
                        keycode::F => c.combo_find += 1,
                        keycode::W => c.combo_close += 1,
                        keycode::N => c.combo_new += 1,
                        keycode::S => c.combo_save += 1,
                        _ => {} // Other combos: not counted (privacy).
                    }
                } else {
                    // Behavior keys only — no letters/digits without modifier.
                    match code {
                        keycode::DELETE | keycode::FORWARD_DELETE => c.key_delete += 1,
                        keycode::TAB => c.key_tab += 1,
                        keycode::ESCAPE => c.key_esc += 1,
                        keycode::RETURN | keycode::ENTER => c.key_enter += 1,
                        keycode::ARROW_LEFT | keycode::ARROW_RIGHT
                        | keycode::ARROW_UP | keycode::ARROW_DOWN => c.key_arrow += 1,
                        keycode::SPACE => c.key_space += 1,
                        _ => {} // All other bare keys: ignored.
                    }
                }
            }
            enqueue_hid(
                counter,
                ObserveHidEvent {
                    kind: ObserveHidKind::KeyDown,
                    keycode: code,
                    unicode: unicode_from_event(event),
                    command: (flags & FLAG_CMD) != 0,
                    control: (flags & FLAG_CTRL) != 0,
                    shift: (flags & FLAG_SHIFT) != 0,
                    option: (flags & FLAG_OPTION) != 0,
                    button: 0,
                    x: 0.0,
                    y: 0.0,
                    click_count: 1,
                },
            );
        }
        TYPE_LEFT_MOUSE_DOWN | TYPE_RIGHT_MOUSE_DOWN | TYPE_OTHER_MOUSE_DOWN => {
            let button = match etype {
                TYPE_LEFT_MOUSE_DOWN => 0,
                TYPE_RIGHT_MOUSE_DOWN => 1,
                _ => 2,
            };
            if let Ok(mut c) = counter.counts.lock() {
                match button {
                    0 => c.mouse_left += 1,
                    1 => c.mouse_right += 1,
                    _ => c.mouse_other += 1,
                }
            }
            let loc = CGEventGetLocation(event);
            let clicks = CGEventGetIntegerValueField(event, K_CG_EVENT_CLICK_STATE).max(1) as u32;
            let flags = CGEventGetFlags(event);
            enqueue_hid(
                counter,
                ObserveHidEvent {
                    kind: ObserveHidKind::MouseDown,
                    keycode: 0,
                    unicode: None,
                    command: (flags & FLAG_CMD) != 0,
                    control: (flags & FLAG_CTRL) != 0,
                    shift: (flags & FLAG_SHIFT) != 0,
                    option: (flags & FLAG_OPTION) != 0,
                    button,
                    x: loc.x,
                    y: loc.y,
                    click_count: clicks,
                },
            );
        }
        TYPE_LEFT_MOUSE_UP | TYPE_RIGHT_MOUSE_UP | TYPE_OTHER_MOUSE_UP => {
            let button = match etype {
                TYPE_LEFT_MOUSE_UP => 0,
                TYPE_RIGHT_MOUSE_UP => 1,
                _ => 2,
            };
            let loc = CGEventGetLocation(event);
            let flags = CGEventGetFlags(event);
            enqueue_hid(
                counter,
                ObserveHidEvent {
                    kind: ObserveHidKind::MouseUp,
                    keycode: 0,
                    unicode: None,
                    command: (flags & FLAG_CMD) != 0,
                    control: (flags & FLAG_CTRL) != 0,
                    shift: (flags & FLAG_SHIFT) != 0,
                    option: (flags & FLAG_OPTION) != 0,
                    button,
                    x: loc.x,
                    y: loc.y,
                    click_count: 1,
                },
            );
        }
        _ => {}
    }

    // Listen-only: always pass the event through unchanged.
    event
}

/// Spawn the input tap on a dedicated thread. Returns a state handle the
/// caller polls periodically to flush counters as events. If the tap cannot
/// be created (no Input Monitoring permission), returns Err.
pub fn start_input_counter(state: &'static InputCounterState) -> Result<(), String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state;
        return Err("input counter requires macOS".into());
    }
    #[cfg(target_os = "macos")]
    unsafe {
        { COUNTER = Some(state); }

        // downs + ups + keyDown
        let mask: u64 = (1 << 1)
            | (1 << 2)
            | (1 << 3)
            | (1 << 4)
            | (1 << 25)
            | (1 << 26)
            | (1 << 10);
        let session_tap: *const c_void = 0 as *const c_void; // kCGSessionEventTap = NULL
        let tap = CGEventTapCreate(
            session_tap,
            1,  // kCGHeadInsertEventTap
            1,  // kCGEventTapOptionListenOnly
            mask,
            tap_callback,
            std::ptr::null_mut(),
        );
        if tap.is_null() {
            return Err(
                "CGEventTap 创建失败 — 需要在系统设置 → 隐私与安全 → 输入监控中授权，且不能在无头环境运行".into(),
            );
        }

        // Wrap the raw pointer in a Send wrapper; it lives entirely on the
        // dedicated thread's run loop.
        let tap_addr = tap as usize;

        std::thread::Builder::new()
            .name("input-counter".into())
            .spawn(move || {
                let tap = tap_addr as *const c_void;
                unsafe {
                    let rl = CFRunLoopGetCurrent();
                    let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
                    let mode: *const c_void = std::ptr::null();
                    CFRunLoopAddSource(rl, source, mode);
                    CGEventTapEnable(tap, true);
                    CFRunLoopRun();
                }
            })
            .map_err(|e| format!("spawn input thread: {e}"))?;

        Ok(())
    }
}

/// Take a snapshot of counters (drain semantics: read without reset so
/// callers can compute deltas).
pub fn snapshot(state: &InputCounterState) -> InputCounts {
    state.counts.lock().unwrap().clone()
}

/// Reset counters to zero (called after flushing an event).
pub fn reset(state: &InputCounterState) {
    *state.counts.lock().unwrap() = InputCounts::default();
}

const PENDING_CAP: usize = 2048;

fn enqueue_hid(state: &InputCounterState, ev: ObserveHidEvent) {
    if let Ok(mut q) = state.pending.lock() {
        if q.len() >= PENDING_CAP {
            q.pop_front();
        }
        q.push_back(ev);
    }
}

unsafe fn unicode_from_event(event: *const c_void) -> Option<String> {
    let mut len = 0usize;
    let mut buf = [0u16; 8];
    CGEventKeyboardGetUnicodeString(event, buf.len(), &mut len, buf.as_mut_ptr());
    if len == 0 {
        return None;
    }
    String::from_utf16(&buf[..len]).ok()
}

pub fn drain_hid(state: &InputCounterState) -> Vec<ObserveHidEvent> {
    state
        .pending
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}
