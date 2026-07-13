fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=src/macos/vision_bridge.m");
        println!("cargo:rerun-if-changed=src/macos/ax_bridge.m");
        println!("cargo:rerun-if-changed=src/macos/screen_bridge.m");
        println!("cargo:rerun-if-changed=src/macos/keychain_bridge.m");
        cc::Build::new()
            .files([
                "src/macos/vision_bridge.m",
                "src/macos/ax_bridge.m",
                "src/macos/screen_bridge.m",
                "src/macos/keychain_bridge.m",
            ])
            .flag("-fobjc-arc")
            .compile("lumen_context_macos_bridges");
        println!("cargo:rustc-link-lib=framework=Vision");
        println!("cargo:rustc-link-lib=framework=ImageIO");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
        println!("cargo:rustc-link-lib=framework=ScreenCaptureKit");
        println!("cargo:rustc-link-lib=framework=Security");
    }
}
