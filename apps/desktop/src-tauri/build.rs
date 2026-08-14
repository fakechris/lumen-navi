fn main() {
    // Inject git short SHA at compile time for build version display.
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=LUMEN_BUILD_SHA={sha}");

    tauri_build::build()
}
