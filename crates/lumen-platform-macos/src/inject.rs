//! Result injection — write assistant output back into the app the text was
//! selected from (划词 popup's「写入原文」), and type user-provided text during
//! an Act replay. Explicit user action only; Observe never calls this.
//!
//! Two paths:
//! 1. AX direct write into the app's focused text control (preferred — no
//!    pasteboard clobbering, works without focus juggling).
//! 2. Pasteboard + synthetic ⌘V fallback for canvas/GPU-rendered inputs that
//!    expose no writable AX value. The user's pasteboard is saved and
//!    restored exactly as in the ⌘C selection fallback (`clipboard.rs`).

use crate::ax::{
    ax_string_attr, AxUIElementRef, ReleaseGuard, AXUIElementCopyAttributeValue,
    AXUIElementCreateApplication, AXUIElementSetAttributeValue,
};

/// How the injected text combines with existing field content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectMode {
    Replace,
    Append,
}

impl InjectMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "replace" => Some(Self::Replace),
            "append" => Some(Self::Append),
            _ => None,
        }
    }
}

/// Write `text` into app `pid`'s focused text control. AX first, pasteboard
/// fallback second. Errors explain why (password field, no target, …).
pub fn inject_text(pid: i32, text: &str, mode: InjectMode) -> Result<(), String> {
    if text.is_empty() {
        return Err("没有可写入的内容".into());
    }
    match inject_via_ax(pid, text, mode) {
        Ok(()) => Ok(()),
        Err(ax_err) => {
            tracing::debug!(error = %ax_err, "AX inject failed, trying pasteboard");
            inject_via_pasteboard(pid, text)
        }
    }
}

/// AX roles that accept text writes. Anything containing "secure" is refused
/// before this check (password fields must never be written).
const ALLOWED_ROLE_SUFFIXES: &[&str] = &["TextField", "TextArea", "SearchField", "WebArea", "ComboBox"];

#[cfg(target_os = "macos")]
fn inject_via_ax(pid: i32, text: &str, mode: InjectMode) -> Result<(), String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;

    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err("无法访问目标应用（可能已退出）".into());
        }
        let _app_guard = ReleaseGuard(app as *const std::ffi::c_void);

        let attr = CFString::new("AXFocusedUIElement");
        // The returned element is retained by us (copy rule) — guard it.
        let mut focused: core_foundation::base::CFTypeRef = std::ptr::null();
        if AXUIElementCopyAttributeValue(
            app,
            attr.as_concrete_TypeRef(),
            &mut focused as *mut _,
        ) != 0
            || focused.is_null()
        {
            return Err("目标应用没有可写入的文本控件".into());
        }
        let _focused_guard = ReleaseGuard(focused);
        let el = focused as AxUIElementRef;

        let role = ax_string_attr(el, "AXRole").unwrap_or_default();
        if role.to_lowercase().contains("secure") {
            return Err("目标看起来是密码字段，已拒绝写入".into());
        }
        if !ALLOWED_ROLE_SUFFIXES.iter().any(|s| role.ends_with(s)) {
            return Err(format!("该控件不接受文本写入（{role}）"));
        }

        let value = match mode {
            InjectMode::Replace => text.to_string(),
            InjectMode::Append => {
                let old = ax_string_attr(el, "AXValue").unwrap_or_default();
                format!("{old}{text}")
            }
        };

        // String attributes take a bare CFString (AXValue is only for
        // point/size/range payloads).
        let cf = CFString::new(&value);
        let err = AXUIElementSetAttributeValue(
            el,
            CFString::new("AXValue").as_concrete_TypeRef(),
            cf.as_concrete_TypeRef() as core_foundation::base::CFTypeRef,
        );
        if err != 0 {
            return Err(format!("AX 写入失败（错误码 {err}）"));
        }
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn inject_via_ax(_pid: i32, _text: &str, _mode: InjectMode) -> Result<(), String> {
    Err("AX 注入仅支持 macOS".into())
}

/// Pasteboard + ⌘V into `pid`. Replaces the current selection (the selection
/// that opened the popup is typically still active, so this reads as
/// "replace selection" semantics). The user's pasteboard is restored after.
#[cfg(target_os = "macos")]
fn inject_via_pasteboard(pid: i32, text: &str) -> Result<(), String> {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString,
        NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSData, NSString};

    activate_pid(pid)?;

    let pb = NSPasteboard::generalPasteboard();
    let before_count = pb.changeCount();
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

    pb.clearContents();
    let s = NSString::from_str(text);
    unsafe { pb.setString_forType(&s, &NSPasteboardTypeString) };

    std::thread::sleep(std::time::Duration::from_millis(120));
    crate::clipboard::post_cmd_v();
    // Give the target app time to consume the paste before restoring.
    std::thread::sleep(std::time::Duration::from_millis(350));

    // Restore the user's previous pasteboard (we clobbered it ourselves).
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
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn inject_via_pasteboard(_pid: i32, _text: &str) -> Result<(), String> {
    Err("粘贴注入仅支持 macOS".into())
}

/// Type `text` into whatever app currently has focus (Act replay `type`
/// steps — the caller has already focused the target window).
#[cfg(target_os = "macos")]
pub fn type_into_focused(text: &str) -> Result<(), String> {
    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardItem, NSPasteboardType, NSPasteboardTypeString,
        NSPasteboardWriting,
    };
    use objc2_foundation::{NSArray, NSData, NSString};

    if text.is_empty() {
        return Err("type 步没有用户提供文本".into());
    }
    let pb = NSPasteboard::generalPasteboard();
    let before_count = pb.changeCount();
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

    pb.clearContents();
    let s = NSString::from_str(text);
    unsafe { pb.setString_forType(&s, &NSPasteboardTypeString) };

    std::thread::sleep(std::time::Duration::from_millis(60));
    crate::clipboard::post_cmd_v();
    std::thread::sleep(std::time::Duration::from_millis(350));

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
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn type_into_focused(_text: &str) -> Result<(), String> {
    Err("type 注入仅支持 macOS".into())
}

/// Human-readable identity of a pid for popup target display
/// ("将写回: Ghostty").
#[cfg(target_os = "macos")]
pub fn app_identity_for_pid(pid: i32) -> Option<(String, Option<String>)> {
    use objc2_app_kit::NSRunningApplication;
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    let name = app
        .localizedName()
        .map(|s: objc2::rc::Retained<objc2_foundation::NSString>| s.to_string())
        .filter(|s| !s.is_empty());
    let bundle = app
        .bundleIdentifier()
        .map(|s: objc2::rc::Retained<objc2_foundation::NSString>| s.to_string())
        .filter(|s| !s.is_empty());
    Some((name.unwrap_or_else(|| format!("pid {pid}")), bundle))
}

#[cfg(not(target_os = "macos"))]
pub fn app_identity_for_pid(pid: i32) -> Option<(String, Option<String>)> {
    Some((format!("pid {pid}"), None))
}

#[cfg(target_os = "macos")]
fn activate_pid(pid: i32) -> Result<(), String> {
    use objc2_app_kit::NSRunningApplication;
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or_else(|| "目标应用已退出".to_string())?;
    let ok = app.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions(0));
    if !ok {
        return Err("无法激活目标应用".into());
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    Ok(())
}
