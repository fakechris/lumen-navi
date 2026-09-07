//! CGWindowList helpers for Act targeting. Not used by Observe capture.

#[derive(Debug, Clone)]
pub struct WindowRecord {
    pub window_id: u64,
    pub pid: i32,
    pub owner: String,
    /// `kCGWindowName`. Empty when Screen Recording is denied.
    pub title: String,
    pub on_screen: bool,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub layer: i32,
}

pub fn list_windows(on_screen_only: bool) -> Vec<WindowRecord> {
    #[cfg(target_os = "macos")]
    {
        list_windows_macos(on_screen_only)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = on_screen_only;
        Vec::new()
    }
}

pub fn window_on_current_space(window_id: u64) -> bool {
    list_windows(true).iter().any(|w| w.window_id == window_id)
}

pub fn top_pid_at(x: f64, y: f64) -> Option<i32> {
    list_windows(true)
        .into_iter()
        .filter(|w| w.layer == 0 && w.w > 0.0 && w.h > 0.0)
        .find(|w| x >= w.x && x <= w.x + w.w && y >= w.y && y <= w.y + w.h)
        .map(|w| w.pid)
}

pub fn windows_for_pid(pid: i32) -> Vec<WindowRecord> {
    list_windows(false)
        .into_iter()
        .filter(|w| w.pid == pid && w.layer == 0 && w.w > 1.0 && w.h > 1.0)
        .collect()
}

/// Pick a window by `kCGWindowName`, never by owner (app) name.
pub fn choose_window<'a>(
    windows: &'a [WindowRecord],
    title: Option<&str>,
) -> Option<&'a WindowRecord> {
    let want = title.unwrap_or("");
    if windows.is_empty() {
        return None;
    }
    if want.is_empty() {
        return windows.first();
    }
    windows
        .iter()
        .find(|w| title_matches(&w.title, want))
        .or(windows.first())
}

fn title_matches(title: &str, want: &str) -> bool {
    !title.is_empty() && (title.contains(want) || want.contains(title))
}

#[cfg(target_os = "macos")]
fn list_windows_macos(on_screen_only: bool) -> Vec<WindowRecord> {
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex};
    use core_foundation_sys::base::CFRelease;

    const OPTION_ALL: u32 = 0;
    const OPTION_ONSCREEN_EXCL_DESKTOP: u32 = 0x11;
    let option = if on_screen_only {
        OPTION_ONSCREEN_EXCL_DESKTOP
    } else {
        OPTION_ALL
    };

    unsafe {
        let raw = CGWindowListCopyWindowInfo(option, 0);
        if raw.is_null() {
            return Vec::new();
        }
        let array = raw as core_foundation_sys::array::CFArrayRef;
        let count = CFArrayGetCount(array);
        let mut out = Vec::new();
        for i in 0..count {
            let dict = CFArrayGetValueAtIndex(array, i)
                as core_foundation_sys::dictionary::CFDictionaryRef;
            if dict.is_null() {
                continue;
            }
            let Some(window_id) = cf_dict_number(dict, "kCGWindowNumber").map(|n| n as u64) else {
                continue;
            };
            let pid = cf_dict_number(dict, "kCGWindowOwnerPID").unwrap_or(0);
            if pid <= 0 {
                continue;
            }
            let owner = cf_dict_string(dict, "kCGWindowOwnerName").unwrap_or_default();
            let title = cf_dict_string(dict, "kCGWindowName").unwrap_or_default();
            let layer = cf_dict_number(dict, "kCGWindowLayer").unwrap_or(-1);
            let (x, y, w, h) = cf_dict_bounds(dict).unwrap_or((0.0, 0.0, 0.0, 0.0));
            out.push(WindowRecord {
                window_id,
                pid,
                owner,
                title,
                on_screen: on_screen_only,
                x,
                y,
                w,
                h,
                layer,
            });
        }
        CFRelease(raw as *const _);
        out
    }
}

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
unsafe fn cf_dict_bounds(
    dict: core_foundation_sys::dictionary::CFDictionaryRef,
) -> Option<(f64, f64, f64, f64)> {
    use core_foundation::base::TCFType;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation_sys::dictionary::CFDictionaryGetValue;
    use core_foundation_sys::number::CFNumberRef;
    let k = CFString::new("kCGWindowBounds");
    let val = CFDictionaryGetValue(dict, k.as_concrete_TypeRef() as *const _);
    if val.is_null() {
        return None;
    }
    let bounds = val as core_foundation_sys::dictionary::CFDictionaryRef;
    let num = |name: &str| -> Option<f64> {
        let nk = CFString::new(name);
        let v = CFDictionaryGetValue(bounds, nk.as_concrete_TypeRef() as *const _);
        if v.is_null() {
            return None;
        }
        CFNumber::wrap_under_get_rule(v as CFNumberRef).to_f64()
    };
    Some((num("X")?, num("Y")?, num("Width")?, num("Height")?))
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const std::ffi::c_void;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: u64, owner: &str, title: &str) -> WindowRecord {
        WindowRecord {
            window_id: id,
            pid: 1,
            owner: owner.into(),
            title: title.into(),
            on_screen: true,
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
            layer: 0,
        }
    }

    #[test]
    fn choose_window_matches_title_not_owner() {
        let windows = [
            rec(1, "Lumen Navi", "Inbox — Gmail"),
            rec(2, "Safari", "Lumen Navi"),
        ];
        let chosen = choose_window(&windows, Some("Lumen Navi")).unwrap();
        assert_eq!(chosen.window_id, 2);
        assert_eq!(chosen.title, "Lumen Navi");
    }

    #[test]
    fn choose_window_empty_title_takes_first() {
        let windows = [rec(1, "Safari", "A"), rec(2, "Safari", "B")];
        assert_eq!(choose_window(&windows, None).unwrap().window_id, 1);
        assert!(choose_window(&[], Some("x")).is_none());
    }
}
