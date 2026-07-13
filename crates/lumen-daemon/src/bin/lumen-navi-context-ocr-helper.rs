#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() != Some("--stdio") {
        std::process::exit(64);
    }
    if let Err(error) = lumen_context::run_vision_ocr_helper_stdio().await {
        eprintln!("Vision OCR helper failed: {error}");
        std::process::exit(1);
    }
}
