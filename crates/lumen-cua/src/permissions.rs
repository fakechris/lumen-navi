use std::time::Duration;

use lumen_platform::PermissionState;
#[cfg(target_os = "macos")]
use lumen_platform_macos::{request_screen_recording, screen_recording_access_granted};

use crate::{CuaStatus, DirectCaptureError, DirectCaptureStatus};

// Permission-host UX must not look hung. 90s covers a slow SCK callback while
// still failing closed if the probe never returns.
const DIRECT_CAPTURE_PROBE_TIMEOUT: Duration = Duration::from_secs(90);

/// After CGRequest, keep the main run loop alive long enough for:
/// - TCC to present a prompt (when allowed)
/// - TCC to register the process in the Screen Recording list
/// - session caches / XPC notifications to settle
const POST_REQUEST_SETTLE: Duration = Duration::from_secs(3);

#[cfg(not(target_os = "macos"))]
fn screen_recording_access_granted() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn request_screen_recording() -> bool {
    false
}

pub(crate) fn read_only_status() -> CuaStatus {
    status_from_observations(screen_recording_access_granted(), None)
}

/// Request the base Screen Recording grant on the caller's main thread, then
/// perform an explicit ScreenCaptureKit capability probe. This function is
/// only called by the short-lived LaunchServices permission host.
///
/// Also requests the Accessibility TCC grant — needed for AX tree walking.
pub(crate) fn request_and_probe_screen_capture() -> CuaStatus {
    promote_for_permission_prompt();

    // Request Accessibility TCC (for AX tree walking). This surfaces the
    // "Lumen Cua wants to control [App]" prompt or registers cua in the
    // Accessibility list for manual enable.
    #[cfg(target_os = "macos")]
    {
        let _ = lumen_platform_macos::accessibility_trusted(true);
        pump_main_run_loop(POST_REQUEST_SETTLE);
    }

    // Always call the request API first so macOS has a chance to register this
    // process identity in Screen Recording (even when it refuses to prompt).
    let requested = request_screen_recording();
    pump_main_run_loop(POST_REQUEST_SETTLE);

    // Second call after settle — some macOS builds only surface the prompt /
    // list entry after the process has stayed alive with a run loop briefly.
    if !screen_recording_access_granted() {
        let _ = request_screen_recording();
        pump_main_run_loop(Duration::from_secs(1));
    }

    let granted = requested || screen_recording_access_granted();
    if !granted {
        // Kick ScreenCaptureKit once even when preflight is false. On some
        // builds this is what causes the app to appear under Screen Recording
        // for manual enable, even though capture itself stays blocked.
        let _ = kick_shareable_content_registration();
        pump_main_run_loop(Duration::from_secs(1));
        let granted_after_kick = screen_recording_access_granted();
        if !granted_after_kick {
            return status_from_observations(false, Some(Ok(false)));
        }
    }

    status_from_observations(true, Some(bounded_direct_capture_probe()))
}

fn status_from_observations(
    screen_recording_granted: bool,
    direct_probe: Option<Result<bool, DirectCaptureError>>,
) -> CuaStatus {
    let screen_recording = if screen_recording_granted {
        PermissionState::Granted
    } else {
        PermissionState::NotDetermined
    };
    if !screen_recording_granted && direct_probe.is_some() {
        return CuaStatus {
            screen_recording,
            screen_recording_capturable: None,
            direct_capture_status: DirectCaptureStatus::BlockedByScreenRecording,
            direct_capture_error: None,
        };
    }

    match direct_probe {
        None => CuaStatus {
            screen_recording,
            screen_recording_capturable: None,
            direct_capture_status: DirectCaptureStatus::NotChecked,
            direct_capture_error: None,
        },
        Some(Ok(true)) => CuaStatus {
            screen_recording,
            screen_recording_capturable: Some(true),
            direct_capture_status: DirectCaptureStatus::Ready,
            direct_capture_error: None,
        },
        Some(Ok(false)) => CuaStatus {
            screen_recording,
            screen_recording_capturable: Some(false),
            direct_capture_status: DirectCaptureStatus::Unavailable,
            direct_capture_error: None,
        },
        Some(Err(error)) => {
            let direct_capture_status = match error.code.as_str() {
                "direct_capture_probe_timed_out" => DirectCaptureStatus::TimedOut,
                "direct_capture_unavailable" => DirectCaptureStatus::Unavailable,
                _ => DirectCaptureStatus::ProbeFailed,
            };
            CuaStatus {
                screen_recording,
                screen_recording_capturable: None,
                direct_capture_status,
                direct_capture_error: Some(error),
            }
        }
    }
}

fn bounded_direct_capture_probe() -> Result<bool, DirectCaptureError> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    start_live_direct_capture_probe(tx)?;

    receive_direct_capture_probe(rx)
}

#[cfg(target_os = "macos")]
fn promote_for_permission_prompt() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        // Regular (not Accessory) is more likely to be allowed to present TCC
        // UI. The process is still short-lived and LSUIElement=1, so no dock icon.
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        app.activate();
    }
}

#[cfg(not(target_os = "macos"))]
fn promote_for_permission_prompt() {}

#[cfg(target_os = "macos")]
fn pump_main_run_loop(duration: Duration) {
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};

    let started = std::time::Instant::now();
    while started.elapsed() < duration {
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            Duration::from_millis(50),
            true,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn pump_main_run_loop(duration: Duration) {
    std::thread::sleep(duration);
}

/// Best-effort TCC registration nudge via ScreenCaptureKit content enumeration.
#[cfg(target_os = "macos")]
fn kick_shareable_content_registration() -> Result<(), DirectCaptureError> {
    use block2::RcBlock;
    use objc2_foundation::NSError;
    use objc2_screen_capture_kit::SCShareableContent;

    if objc2::runtime::AnyClass::get(c"SCShareableContent").is_none() {
        return Ok(());
    }

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let completion = RcBlock::new(move |_content: *mut SCShareableContent, _error: *mut NSError| {
        let _ = tx.send(());
    });
    unsafe {
        SCShareableContent::getShareableContentWithCompletionHandler(&completion);
    }

    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if rx.try_recv().is_ok() {
            break;
        }
        pump_main_run_loop(Duration::from_millis(50));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn kick_shareable_content_registration() -> Result<(), DirectCaptureError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn receive_direct_capture_probe(
    rx: std::sync::mpsc::Receiver<Result<bool, DirectCaptureError>>,
) -> Result<bool, DirectCaptureError> {
    use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};

    let started = std::time::Instant::now();
    loop {
        match rx.try_recv() {
            Ok(result) => return result,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(DirectCaptureError {
                    code: "direct_capture_probe_failed".into(),
                    message: "ScreenCaptureKit capability probe exited without a result".into(),
                });
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if started.elapsed() >= DIRECT_CAPTURE_PROBE_TIMEOUT {
            return Err(DirectCaptureError {
                code: "direct_capture_probe_timed_out".into(),
                message: "ScreenCaptureKit capability probe did not complete within 90 seconds"
                    .into(),
            });
        }

        // The permission host runs on the app's main thread. Pump its run loop
        // while awaiting ScreenCaptureKit so macOS can present consent UI and
        // deliver callbacks without deadlocking that thread.
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            Duration::from_millis(50),
            true,
        );
    }
}

#[cfg(not(target_os = "macos"))]
fn receive_direct_capture_probe(
    rx: std::sync::mpsc::Receiver<Result<bool, DirectCaptureError>>,
) -> Result<bool, DirectCaptureError> {
    match rx.recv_timeout(DIRECT_CAPTURE_PROBE_TIMEOUT) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(DirectCaptureError {
            code: "direct_capture_probe_timed_out".into(),
            message: "ScreenCaptureKit capability probe did not complete within 90 seconds".into(),
        }),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(DirectCaptureError {
            code: "direct_capture_probe_failed".into(),
            message: "ScreenCaptureKit capability probe exited without a result".into(),
        }),
    }
}

#[cfg(target_os = "macos")]
fn start_live_direct_capture_probe(
    sender: std::sync::mpsc::SyncSender<Result<bool, DirectCaptureError>>,
) -> Result<(), DirectCaptureError> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2_core_graphics::CGImage;
    use objc2_foundation::{NSArray, NSError};
    use objc2_screen_capture_kit::{
        SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
    };

    if objc2::runtime::AnyClass::get(c"SCShareableContent").is_none()
        || objc2::runtime::AnyClass::get(c"SCScreenshotManager").is_none()
    {
        return Err(DirectCaptureError {
            code: "direct_capture_unavailable".into(),
            message: "This macOS version does not provide ScreenCaptureKit screenshot probing"
                .into(),
        });
    }

    let completion = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            if !error.is_null() {
                let _ = sender.send(Err(DirectCaptureError {
                    code: "direct_capture_probe_failed".into(),
                    message: "ScreenCaptureKit returned an error while listing shareable content"
                        .into(),
                }));
                return;
            }
            if content.is_null() {
                let _ = sender.send(Err(DirectCaptureError {
                    code: "direct_capture_probe_failed".into(),
                    message: "ScreenCaptureKit returned no shareable content".into(),
                }));
                return;
            }

            let displays = unsafe { (&*content).displays() };
            if displays.count() == 0 {
                let _ = sender.send(Ok(false));
                return;
            }

            let display = displays.objectAtIndex(0);
            let excluded = NSArray::<SCWindow>::new();
            let filter = unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &excluded,
                )
            };
            let configuration = unsafe { SCStreamConfiguration::new() };
            unsafe {
                configuration.setWidth(64);
                configuration.setHeight(64);
            }

            let screenshot_sender = sender.clone();
            let screenshot = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
                let result = if !error.is_null() {
                    Err(DirectCaptureError {
                        code: "direct_capture_probe_failed".into(),
                        message: "ScreenCaptureKit returned an error while capturing a probe frame"
                            .into(),
                    })
                } else {
                    Ok(!image.is_null())
                };
                let _ = screenshot_sender.send(result);
            });
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &configuration,
                    Some(&screenshot),
                );
            }
        },
    );
    unsafe {
        SCShareableContent::getShareableContentWithCompletionHandler(&completion);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn start_live_direct_capture_probe(
    sender: std::sync::mpsc::SyncSender<Result<bool, DirectCaptureError>>,
) -> Result<(), DirectCaptureError> {
    let _ = sender.send(Ok(false));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_status_never_claims_direct_capture_was_checked() {
        let status = status_from_observations(false, None);

        assert_eq!(status.screen_recording, PermissionState::NotDetermined);
        assert_eq!(status.screen_recording_capturable, None);
        assert_eq!(
            status.direct_capture_status,
            DirectCaptureStatus::NotChecked
        );
        assert_eq!(status.direct_capture_error, None);
    }

    #[test]
    fn live_probe_is_blocked_until_screen_recording_is_granted() {
        let status = status_from_observations(false, Some(Ok(true)));

        assert_eq!(
            status.direct_capture_status,
            DirectCaptureStatus::BlockedByScreenRecording
        );
        assert_eq!(status.screen_recording_capturable, None);
    }

    #[test]
    fn live_probe_failure_is_structured() {
        let status = status_from_observations(
            true,
            Some(Err(DirectCaptureError {
                code: "direct_capture_probe_timed_out".into(),
                message: "probe timed out".into(),
            })),
        );

        assert_eq!(status.direct_capture_status, DirectCaptureStatus::TimedOut);
        assert_eq!(status.screen_recording_capturable, None);
        assert_eq!(
            status
                .direct_capture_error
                .as_ref()
                .map(|error| error.code.as_str()),
            Some("direct_capture_probe_timed_out")
        );
    }
}
