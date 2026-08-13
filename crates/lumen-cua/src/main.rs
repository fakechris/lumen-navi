use std::ffi::OsString;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    initialize_macos_application()?;
    // Write logs to a file so they're visible (cua's stdout/stderr go to
    // /dev/null when launched via `open -n -g`).
    let log_dir = lumen_cua::CuaPaths::for_current_user()
        .socket
        .parent()
        .map(|p| p.to_path_buf());
    if let Some(dir) = &log_dir {
        let _ = std::fs::create_dir_all(dir);
        let log_path = dir.join("cua.log");
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            let filter = EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lumen_cua=debug,lumen_platform_macos=debug,warn".into());
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(file)
                .try_init();
        } else {
            let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen_cua=info,warn".into());
            let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
        }
    } else {
        let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen_cua=info,warn".into());
        let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
    }

    let args = launch_arguments();
    if lumen_cua::is_permission_host_request(&args) {
        activate_for_permission_prompt();
        return lumen_cua::run_permission_host(&args);
    }
    if is_serve_request(&args) {
        return run_serve();
    }

    eprintln!(
        "Lumen Cua is a background screen-capture helper for Lumen apps.\n\
         \n\
         It has no window. Start it through Lumen Navi, or run:\n\
           open -n -g \"/Applications/Lumen Cua.app\" --args serve\n\
         \n\
         Do not double-click the app for normal use — Navi installs and manages it."
    );
    bail!("usage: lumen-cua serve");
}

fn launch_arguments() -> Vec<OsString> {
    std::env::args_os()
        .skip(1)
        // Finder / LaunchServices injects -psn_… when the user opens the .app.
        .filter(|arg| {
            arg.to_str()
                .map(|value| !value.starts_with("-psn_"))
                .unwrap_or(true)
        })
        .collect()
}

fn is_serve_request(args: &[OsString]) -> bool {
    // Empty args (double-click / open without --args) default to serve so the
    // helper actually stays up instead of silently exiting.
    args.is_empty() || (args.len() == 1 && args[0] == "serve")
}

fn run_serve() -> Result<()> {
    let paths = lumen_cua::CuaPaths::for_current_user();
    lumen_cua::ensure_token_file(&paths.token_file)?;

    #[cfg(target_os = "macos")]
    {
        // Keep AppKit's main thread pumping so Activity Monitor does not mark
        // this accessory process as Not Responding while Tokio serves IPC and
        // capture work on background threads.
        let socket = paths.socket.clone();
        let token_file = paths.token_file.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("lumen-cua-serve".into())
            .spawn(move || {
                let result = (|| {
                    let runtime = tokio::runtime::Builder::new_multi_thread()
                        .enable_all()
                        .build()
                        .context("create Lumen Cua runtime")?;
                    runtime.block_on(lumen_cua::serve(&socket, &token_file))
                })();
                let _ = done_tx.send(result);
            })
            .context("spawn Lumen Cua serve thread")?;

        use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop};
        loop {
            match done_rx.try_recv() {
                Ok(result) => return result,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    bail!("Lumen Cua serve thread exited without a result")
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            CFRunLoop::run_in_mode(
                unsafe { kCFRunLoopDefaultMode },
                Duration::from_millis(100),
                true,
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("create Lumen Cua runtime")?;
        runtime.block_on(lumen_cua::serve(&paths.socket, &paths.token_file))
    }
}

#[cfg(target_os = "macos")]
fn initialize_macos_application() -> Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let mtm = MainThreadMarker::new().context("Lumen Cua must start on the macOS main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn initialize_macos_application() -> Result<()> {
    Ok(())
}

/// Briefly activate so TCC may present Screen Recording UI for this process.
/// The app remains LSUIElement/Accessory — activation does not create a dock icon.
#[cfg(target_os = "macos")]
fn activate_for_permission_prompt() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        app.activate();
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_for_permission_prompt() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_serve_args_start_the_helper() {
        assert!(is_serve_request(&[]));
        assert!(is_serve_request(&[OsString::from("serve")]));
        assert!(!is_serve_request(&[OsString::from("status")]));
        assert!(!is_serve_request(&[
            OsString::from("__permission-host"),
            OsString::from("--result-file"),
            OsString::from("/tmp/x.json"),
        ]));
    }
}
