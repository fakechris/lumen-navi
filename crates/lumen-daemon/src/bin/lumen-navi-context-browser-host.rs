fn main() {
    let config = lumen_config::default_browser_host_config_path();
    if let Err(error) = lumen_context::run_native_browser_host_with_config(Some(config)) {
        eprintln!("native browser host failed: {error}");
        std::process::exit(1);
    }
}
