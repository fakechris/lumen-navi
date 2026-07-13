#![cfg(target_os = "macos")]

use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};
use lumen_context::{HelperVisionOcr, OcrEngine};

fn blank_png() -> Vec<u8> {
    let image = ImageBuffer::from_pixel(64, 64, Rgba([255_u8, 255, 255, 255]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut encoded, ImageFormat::Png)
        .unwrap();
    encoded.into_inner()
}

#[tokio::test]
async fn vision_helper_round_trips_a_real_request() {
    let engine = HelperVisionOcr::new(
        PathBuf::from(env!("CARGO_BIN_EXE_lumen-context-ocr-helper")),
        Duration::from_secs(10),
        1024 * 1024,
    );

    let result = engine
        .recognize_boxes(&blank_png(), &["en-US".to_owned()])
        .await
        .unwrap();

    assert_eq!(result.mode, "accurate_layout");
}

#[tokio::test]
async fn helper_failure_is_reported_without_terminating_the_caller() {
    let engine = HelperVisionOcr::new(
        PathBuf::from("/usr/bin/false"),
        Duration::from_secs(2),
        1024 * 1024,
    );

    let error = engine
        .recognize_text(&blank_png(), &["en-US".to_owned()])
        .await
        .unwrap_err();

    assert!(error.to_string().contains("OCR helper"));
}

#[tokio::test]
async fn hung_helper_is_killed_at_its_deadline() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("hung-helper");
    std::fs::write(&executable, b"#!/bin/sh\nexec /bin/sleep 30\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let engine = HelperVisionOcr::new(executable, Duration::from_millis(50), 1024 * 1024);
    let started = Instant::now();

    let error = engine
        .recognize_text(&blank_png(), &["en-US".to_owned()])
        .await
        .unwrap_err();

    assert!(error.to_string().contains("timed out"));
    assert!(started.elapsed() < Duration::from_secs(2));
}
