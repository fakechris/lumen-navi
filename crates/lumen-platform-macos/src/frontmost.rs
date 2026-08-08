//! Frontmost application probe (cheap signal for screenshot.v1 payload).

use async_trait::async_trait;
use lumen_platform::{FrontmostApp, FrontmostAppProbe, PlatformError};

pub struct MacFrontmost;

#[async_trait]
impl FrontmostAppProbe for MacFrontmost {
    async fn frontmost(&self) -> Result<Option<FrontmostApp>, PlatformError> {
        Ok(frontmost_app())
    }
}

pub fn frontmost_app() -> Option<FrontmostApp> {
    frontmost_native().or_else(frontmost_osascript)
}

/// Resolve the true frontmost app via `CGWindowListCopyWindowInfo` (layer-0
/// windows sorted by z-order). This is the correct API for background
/// processes — `NSWorkspace.frontmostApplication()` reports the caller's own
/// bundle from a daemon, not the user's actual focused window. Returns the
/// owner app name + pid so we can scope the AX title query.
#[cfg(target_os = "macos")]
fn frontmost_via_windowlist() -> Option<(String, i32)> {
    use core_foundation_sys::base::CFRelease;

    // CGWindowListOption: onScreenOnly (1<<0) | excludeDesktopElements (1<<4) = 0x11
    const OPTION_ONSCREEN_EXCL_DESKTOP: u32 = 0x11;

    unsafe {
        let raw = CGWindowListCopyWindowInfo(OPTION_ONSCREEN_EXCL_DESKTOP, 0);
        if raw.is_null() {
            return None;
        }
        let array = raw as core_foundation_sys::array::CFArrayRef;
        let count = core_foundation_sys::array::CFArrayGetCount(array);
        for i in 0..count {
            let dict = core_foundation_sys::array::CFArrayGetValueAtIndex(array, i)
                as core_foundation_sys::dictionary::CFDictionaryRef;
            if dict.is_null() {
                continue;
            }
            // Skip non-window-layer entries (menu bar, dock, etc. live at layer > 0).
            let layer = cf_dict_number(dict, "kCGWindowLayer").unwrap_or(-1);
            if layer != 0 {
                continue;
            }
            let owner = cf_dict_string(dict, "kCGWindowOwnerName");
            let pid = cf_dict_number(dict, "kCGWindowOwnerPID").unwrap_or(0);
            if let Some(name) = owner {
                if !name.is_empty() && pid > 0 {
                    CFRelease(raw as *const _);
                    return Some((name, pid));
                }
            }
        }
        CFRelease(raw as *const _);
        None
    }
}

/// Read a CFString value from a CGWindowList dictionary by key.
#[cfg(target_os = "macos")]
unsafe fn cf_dict_string(
    dict: core_foundation_sys::dictionary::CFDictionaryRef,
    key: &str,
) -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::string::CFString;
    use core_foundation_sys::dictionary::CFDictionaryGetValue;
    use core_foundation_sys::string::CFStringRef;
    let k = CFString::new(key);
    let val = CFDictionaryGetValue(dict, k.as_concrete_TypeRef() as *const _);
    if val.is_null() {
        return None;
    }
    let s = CFString::wrap_under_get_rule(val as CFStringRef);
    Some(s.to_string())
}

/// Read a numeric (CFNumber) value from a CGWindowList dictionary by key.
#[cfg(target_os = "macos")]
unsafe fn cf_dict_number(
    dict: core_foundation_sys::dictionary::CFDictionaryRef,
    key: &str,
) -> Option<i32> {
    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;
    use core_foundation_sys::dictionary::CFDictionaryGetValue;
    use core_foundation_sys::number::CFNumberRef;
    let k = core_foundation::string::CFString::new(key);
    let val = CFDictionaryGetValue(dict, k.as_concrete_TypeRef() as *const _);
    if val.is_null() {
        return None;
    }
    CFNumber::wrap_under_get_rule(val as CFNumberRef).to_i32()
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const std::ffi::c_void;
}

#[cfg(target_os = "macos")]
fn frontmost_native() -> Option<FrontmostApp> {
    // CGWindowList is the source of truth for background processes (daemons).
    // NSWorkspace.frontmostApplication() returns the caller's own bundle from a
    // child process, not the user's actual focused window. Try the window list
    // first; resolve bundle id from the pid via NSRunningApplication.
    if let Some((app_name, pid)) = frontmost_via_windowlist() {
        let bundle_id = bundle_id_for_pid(pid);
        let window_title = crate::ax::focused_window_title(pid);
        return Some(FrontmostApp {
            app_name,
            bundle_id,
            window_title,
        });
    }

    // Fallback: NSWorkspace (correct when the caller is itself the frontmost app,
    // e.g. the selection popup path).
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    let ws = NSWorkspace::sharedWorkspace();
    let app = ws.frontmostApplication()?;
    let app_name = app
        .localizedName()
        .map(|s: objc2::rc::Retained<NSString>| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let bundle_id = app
        .bundleIdentifier()
        .map(|s: objc2::rc::Retained<NSString>| s.to_string())
        .filter(|s| !s.is_empty());

    let pid = app.processIdentifier();
    let window_title = if pid > 0 {
        crate::ax::focused_window_title(pid)
    } else {
        None
    };

    Some(FrontmostApp {
        app_name,
        bundle_id,
        window_title,
    })
}

/// Look up the bundle id for a pid via NSRunningApplication.
#[cfg(target_os = "macos")]
fn bundle_id_for_pid(pid: i32) -> Option<String> {
    use objc2_app_kit::NSRunningApplication;
    let app = unsafe { NSRunningApplication::runningApplicationWithProcessIdentifier(pid) };
    app.and_then(|a| {
        a.bundleIdentifier()
            .map(|s: objc2::rc::Retained<objc2_foundation::NSString>| s.to_string())
            .filter(|s| !s.is_empty())
    })
}

#[cfg(not(target_os = "macos"))]
fn frontmost_native() -> Option<FrontmostApp> {
    None
}

fn frontmost_osascript() -> Option<FrontmostApp> {
    #[cfg(target_os = "macos")]
    {
        let script = r#"
tell application "System Events"
  set p to first application process whose frontmost is true
  set n to name of p
  set b to ""
  try
    set b to bundle identifier of p
  end try
  return n & linefeed & b
end tell
"#;
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&output.stdout);
        let mut lines = s.lines();
        let name = lines.next().map(str::trim).filter(|x| !x.is_empty())?;
        let bundle = lines
            .next()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(|x| x.to_string());
        Some(FrontmostApp {
            app_name: name.to_string(),
            bundle_id: bundle,
            window_title: None,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
