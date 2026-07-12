fn main() {
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rerun-if-changed=src/ocr_bridge.m");
        cc::Build::new()
            .file("src/ocr_bridge.m")
            .flag("-fobjc-arc")
            .compile("lumen_ocr_bridge");
        println!("cargo:rustc-link-lib=framework=Vision");
        println!("cargo:rustc-link-lib=framework=ImageIO");
        println!("cargo:rustc-link-lib=framework=CoreGraphics");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
