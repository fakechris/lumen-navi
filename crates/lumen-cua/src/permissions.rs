use lumen_platform::PermissionState;
use lumen_platform_macos::{request_screen_recording, screen_recording_access_granted};

use crate::{CuaStatus, DirectCaptureError, DirectCaptureStatus};

const DIRECT_CAPTURE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub(crate) fn read_only_status() -> CuaStatus {
    status_from_observations(screen_recording_access_granted(), None)
}

/// Request the base Screen Recording grant on the caller's main thread, then
/// perform an explicit ScreenCaptureKit capability probe. This function is
/// only called by the short-lived LaunchServices permission host.
pub(crate) fn request_and_probe_screen_capture() -> CuaStatus {
    let requested = request_screen_recording();
    let granted = requested || screen_recording_access_granted();
    if !granted {
        return status_from_observations(false, Some(Ok(false)));
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
                message: "ScreenCaptureKit capability probe did not complete within 5 minutes"
                    .into(),
            });
        }

        // The permission host runs on the app's main thread. Pump its run loop
        // while awaiting ScreenCaptureKit so macOS can present consent UI and
        // deliver callbacks without deadlocking that thread.
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            std::time::Duration::from_millis(50),
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
            message: "ScreenCaptureKit capability probe did not complete within 5 minutes".into(),
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
