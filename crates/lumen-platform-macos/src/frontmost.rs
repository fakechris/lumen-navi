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
    frontmost_native()
        .or_else(frontmost_osascript)
        .map(|mut f| {
            // For browsers, enrich with the active tab URL (per-website time
            // tracking). Done here so every construction path benefits. Only
            // spawned when the bundle looks like a scriptable browser; cheap
            // for non-browsers (early return).
            if let Some(ref bid) = f.bundle_id {
                if is_scriptable_browser(bid) {
                    f.tab_url = browser_tab_url(bid);
                }
            }
            f
        })
}

/// Bundle ids of browsers that expose the active tab URL via AppleScript.
/// Safari uses the WebKit sdef (`current tab`); Chromium-family browsers
/// (Chrome, Edge, Brave, Arc, Comet, Vivaldi, Opera) share one sdef
/// (`active tab`). Firefox is NOT scriptable for URLs and intentionally
/// absent — it would need the browser extension or fall back to title-only.
fn is_scriptable_browser(bundle_id: &str) -> bool {
    const KNOWN: &[&str] = &[
        "com.apple.Safari",
        "com.google.Chrome",
        "com.microsoft.edgemac",
        "com.brave.Browser",
        "company.thebrowser.Browser", // Arc
        "ai.perplexity.comet",
        "com.vivaldi.Vivaldi",
        "com.operasoftware.Opera",
    ];
    // Prefix match covers edition variants (e.g. Safari Technology Preview).
    KNOWN.iter().any(|k| bundle_id.starts_with(k))
}

/// Read the active tab URL of a scriptable browser via AppleScript. Returns
/// None on any error or empty result (browser not running, Automation
/// permission not granted, private window where the browser refuses, etc.).
///
/// Safari speaks the WebKit sdef; Chromium-family browsers share the Chrome
/// sdef. We target by bundle id (`tell application id <bid>`) so display-name
/// differences (e.g. localized names) don't matter. First use per target app
/// triggers a native TCC "Automation" prompt; subsequent calls are free.
fn browser_tab_url(bundle_id: &str) -> Option<String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle_id;
        None
    }
    #[cfg(target_os = "macos")]
    {
        let script = if bundle_id.starts_with("com.apple.Safari") {
            // WebKit sdef: "current tab of front window".
            format!(
                r#"tell application id "{}" to get URL of current tab of front window"#,
                bundle_id
            )
        } else {
            // Chromium-family sdef (Chrome, Edge, Brave, Arc, Comet, Vivaldi, …):
            // "active tab of front window".
            format!(
                r#"tell application id "{}" to get URL of active tab of front window"#,
                bundle_id
            )
        };

        // osascript can block if the target app is hung; bound it. The probe
        // already runs off the async hot path inside the orchestrator's poll,
        // but the subprocess timeout is a belt-and-suspenders guard.
        let output = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .ok()?;
        if !output.status.success() {
            // Most common: Automation permission denied, or browser not running.
            return None;
        }
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Browsers sometimes return "" or a missing-value placeholder.
        if url.is_empty() || url.eq_ignore_ascii_case("missing value") {
            return None;
        }
        // Only persist real http(s) URLs — drop about:, file://, chrome://, etc.
        // (those are internal and not useful for per-site tracking).
        if url.starts_with("http://") || url.starts_with("https://") {
            Some(url)
        } else {
            None
        }
    }
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
        // System owners that appear at layer 0 but aren't real user-facing apps.
        const SYSTEM_OWNERS: &[&str] = &[
            "Window Server", "Dock", "SystemUIServer", "ControlCenter",
            "Notification Center", "Spotlight", "loginwindow",
        ];
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
            let Some(name) = owner else { continue };
            if name.is_empty() || pid <= 0 {
                continue;
            }
            // Skip system processes that pollute layer 0.
            if SYSTEM_OWNERS.contains(&name.as_str()) {
                continue;
            }
            // Require a non-zero window bounds — real app windows have them;
            // invisible/system overlays often don't.
            let has_bounds = cf_dict_bounds_present(dict);
            if !has_bounds {
                continue;
            }
            CFRelease(raw as *const _);
            return Some((name, pid));
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

/// Whether the window has a real (non-zero) bounds dict — filters out
/// invisible overlays and system pseudo-windows.
#[cfg(target_os = "macos")]
unsafe fn cf_dict_bounds_present(
    dict: core_foundation_sys::dictionary::CFDictionaryRef,
) -> bool {
    use core_foundation::base::TCFType;
    use core_foundation_sys::dictionary::CFDictionaryGetValue;
    let k = core_foundation::string::CFString::new("kCGWindowBounds");
    let val = CFDictionaryGetValue(dict, k.as_concrete_TypeRef() as *const _);
    !val.is_null()
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
    if let Some((owner_name, pid)) = frontmost_via_windowlist() {
        let meta = running_app_meta(pid);
        let app_name = meta
            .localized_name
            .filter(|s| !s.is_empty())
            .unwrap_or(owner_name);
        let window_title = crate::ax::focused_window_title(pid);
        return Some(FrontmostApp {
            app_name,
            bundle_id: meta.bundle_id,
            window_title,
            ls_category_type: meta.ls_category_type,
            tab_url: None,
            pid: Some(pid),
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
    let ls_category_type = if pid > 0 {
        running_app_meta(pid).ls_category_type
    } else {
        None
    };

    Some(FrontmostApp {
        app_name,
        bundle_id,
        window_title,
        ls_category_type,
        tab_url: None,
        pid: if pid > 0 { Some(pid) } else { None },
    })
}

#[cfg(target_os = "macos")]
struct RunningAppMeta {
    localized_name: Option<String>,
    bundle_id: Option<String>,
    ls_category_type: Option<String>,
}

/// Resolve display name, bundle id, and Info.plist category for a pid.
#[cfg(target_os = "macos")]
fn running_app_meta(pid: i32) -> RunningAppMeta {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;

    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return RunningAppMeta {
            localized_name: None,
            bundle_id: None,
            ls_category_type: None,
        };
    };
    let localized_name = app
        .localizedName()
        .map(|s: objc2::rc::Retained<NSString>| s.to_string())
        .filter(|s| !s.is_empty());
    let bundle_id = app
        .bundleIdentifier()
        .map(|s: objc2::rc::Retained<NSString>| s.to_string())
        .filter(|s| !s.is_empty());
    let ls_category_type = app.bundleURL().and_then(|url| {
        let path = url.path()?.to_string();
        ls_application_category_type(&path)
    });
    RunningAppMeta {
        localized_name,
        bundle_id,
        ls_category_type,
    }
}

/// Read `LSApplicationCategoryType` from an `.app` bundle path.
#[cfg(target_os = "macos")]
fn ls_application_category_type(app_path: &str) -> Option<String> {
    let plist = std::path::Path::new(app_path).join("Contents/Info.plist");
    if !plist.is_file() {
        return None;
    }
    let output = std::process::Command::new("/usr/libexec/PlistBuddy")
        .arg("-c")
        .arg("Print :LSApplicationCategoryType")
        .arg(&plist)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() || value.starts_with("Print:") || value.contains("Does Not Exist") {
        None
    } else {
        Some(value)
    }
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
        let ls_category_type = bundle
            .as_ref()
            .and_then(|b| {
                // Best-effort: locate app via mdfind (slow path; rare fallback).
                let _ = b;
                None
            });
        Some(FrontmostApp {
            app_name: name.to_string(),
            bundle_id: bundle,
            window_title: None,
            ls_category_type,
            tab_url: None,
            pid: None,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
