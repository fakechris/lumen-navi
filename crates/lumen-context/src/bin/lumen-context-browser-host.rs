#![cfg(unix)]

fn main() {
    if let Err(error) = lumen_context::run_native_browser_host() {
        eprintln!("native browser host failed: {error}");
        std::process::exit(1);
    }
}
