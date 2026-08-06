use anyhow::{bail, Context, Result};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    initialize_macos_application()?;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "lumen_cua=info,warn".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 1 || args[0] != "serve" {
        bail!("usage: lumen-cua serve");
    }
    let paths = lumen_cua::CuaPaths::for_current_user();
    lumen_cua::ensure_token_file(&paths.token_file)?;
    lumen_cua::serve(&paths.socket, &paths.token_file).await
}

#[cfg(target_os = "macos")]
fn initialize_macos_application() -> Result<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    let mtm = MainThreadMarker::new().context("Lumen Cua must start on the macOS main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn initialize_macos_application() -> Result<()> {
    Ok(())
}
