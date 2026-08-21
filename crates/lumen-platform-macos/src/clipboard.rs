//! ⌘C-based selection grab with full pasteboard save/restore.
//!
//! Last-resort fallback for apps that expose no Accessibility text at all
//! (canvas-rendered editors like DingTalk Docs, GPU terminals like Warp):
//! simulate ⌘C so the *app itself* copies its own selection, read the
//! string, then restore the user's pasteboard contents exactly as before.
//!
//! Privacy: previous pasteboard contents are held only in memory during the
//! grab and written back immediately — never persisted or transmitted. The
//! grabbed selection is treated exactly like an AX selection (sent to the
//! LLM only on explicit user action).

/// Grab the current selection by simulating ⌘C and reading the pasteboard,
/// restoring prior contents afterwards. Returns None when nothing was
/// copied (no selection, app refused, or timeout).
pub fn clipboard_grab_selection() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        unsafe { grab_native() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
unsafe fn grab_native() -> Option<String> {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString,
        NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSData};

    let pb = NSPasteboard::generalPasteboard();
    let before_count = pb.changeCount();

    // Save the full pasteboard (all items, all data types) for restore.
    let saved: Vec<Vec<(Retained<NSPasteboardType>, Retained<NSData>)>> = pb
        .pasteboardItems()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.types()
                        .iter()
                        .filter_map(|t| item.dataForType(&t).map(|d| (t.clone(), d)))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();

    post_cmd_c();

    // The target app copies asynchronously; poll for the change.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
    let mut grabbed: Option<String> = None;
    while std::time::Instant::now() < deadline {
        if pb.changeCount() != before_count {
            if let Some(s) = pb.stringForType(NSPasteboardTypeString) {
                let s = s.to_string();
                if !s.trim().is_empty() {
                    grabbed = Some(s);
                }
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // Restore previous contents exactly (only if ⌘C actually clobbered them).
    if pb.changeCount() != before_count {
        pb.clearContents();
        if !saved.is_empty() {
            let mut restored: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> =
                Vec::with_capacity(saved.len());
            for item_types in &saved {
                let item = NSPasteboardItem::new();
                for (t, d) in item_types {
                    item.setData_forType(d, t);
                }
                restored.push(ProtocolObject::from_retained(item));
            }
            let array = NSArray::from_retained_slice(&restored);
            pb.writeObjects(&array);
        }
    }

    if let Some(t) = &grabbed {
        tracing::debug!(chars = t.chars().count(), "selection via ⌘C fallback");
    }
    grabbed
}

/// Post ⌘C (key down + up with Command flag) to the HID event tap.
#[cfg(target_os = "macos")]
fn post_cmd_c() {
    post_cmd_key(8); // kVK_ANSI_C
}

/// Post ⌘V — used by result injection (`inject.rs`) to paste into the target.
#[cfg(target_os = "macos")]
pub fn post_cmd_v() {
    post_cmd_key(9); // kVK_ANSI_V
}

#[cfg(target_os = "macos")]
fn post_cmd_key(keycode: u16) {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let Ok(source) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) else {
        return;
    };
    if let Ok(down) = CGEvent::new_keyboard_event(source.clone(), keycode, true) {
        down.set_flags(CGEventFlags::CGEventFlagCommand);
        down.post(CGEventTapLocation::HID);
    }
    if let Ok(up) = CGEvent::new_keyboard_event(source, keycode, false) {
        up.set_flags(CGEventFlags::CGEventFlagCommand);
        up.post(CGEventTapLocation::HID);
    }
}
