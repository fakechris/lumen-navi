//! Session lock / secure desktop detection.
//!
//! `closed_eyes` and lock are hard capture gates, so this must fail closed:
//! when the state cannot be determined we report "locked" and skip capture
//! rather than risk writing a frame of the lock or UAC screen.

use async_trait::async_trait;
use lumen_platform::{PlatformError, ScreenLockProbe};

pub struct WinScreenLock;

#[async_trait]
impl ScreenLockProbe for WinScreenLock {
    async fn is_locked(&self) -> Result<bool, PlatformError> {
        Ok(is_screen_locked())
    }
}

/// True when the input desktop is not the interactive one — the machine is
/// locked, showing a UAC secure-desktop prompt, or the session is disconnected.
pub fn is_screen_locked() -> bool {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::StationsAndDesktops::{
            CloseDesktop, OpenInputDesktop, DESKTOP_SWITCHDESKTOP, DESKTOP_CONTROL_FLAGS,
        };

        unsafe {
            match OpenInputDesktop(
                DESKTOP_CONTROL_FLAGS(0),
                false,
                DESKTOP_SWITCHDESKTOP,
            ) {
                Ok(desktop) => {
                    let _ = CloseDesktop(desktop);
                    false
                }
                // The lock screen and the UAC secure desktop both run on a
                // desktop this process may not open.
                Err(_) => true,
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}
