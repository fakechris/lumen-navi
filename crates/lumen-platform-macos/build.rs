fn main() {
    // `cfg!(target_os)` inside a build script describes the *host*, so a
    // macOS → Windows cross build would still try to compile the Objective-C
    // bridges. Cargo's target env var is the authoritative signal.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rerun-if-changed=src/ocr_bridge.m");
    println!("cargo:rerun-if-changed=src/asr_bridge.m");
    println!("cargo:rerun-if-changed=src/permission_bridge.m");
    cc::Build::new()
        .file("src/ocr_bridge.m")
        .flag("-fobjc-arc")
        .compile("lumen_ocr_bridge");
    cc::Build::new()
        .file("src/asr_bridge.m")
        .flag("-fobjc-arc")
        .compile("lumen_asr_bridge");
    cc::Build::new()
        .file("src/permission_bridge.m")
        .flag("-fobjc-arc")
        .compile("lumen_permission_bridge");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=Vision");
    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-lib=framework=ImageIO");
    println!("cargo:rustc-link-lib=framework=CoreGraphics");
    println!("cargo:rustc-link-lib=framework=Foundation");
}
