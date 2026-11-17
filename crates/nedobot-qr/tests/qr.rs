use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb, RgbImage, imageops};
use nedobot_qr::{as_utf8, scan_image};
use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed, TestRunner},
};
use std::io::Cursor;

#[test]
fn decodes_simple_qr_and_screenshot() {
    for name in ["normal.png", "screenshot.png"] {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(path).unwrap();
        let payloads = scan_image(&bytes).unwrap();
        assert!(!payloads.is_empty(), "missing QR: {name}");
        assert!(
            payloads.iter().any(|p| as_utf8(p).is_some()),
            "invalid QR text in {name}"
        );
    }
}

#[test]
fn qr_payloads_can_be_parsed_by_the_shared_url_crate() {
    for name in ["normal.png", "screenshot.png", "multiple.png"] {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(path).unwrap();
        let payloads = scan_image(&bytes).unwrap();
        assert!(
            payloads
                .iter()
                .filter_map(|payload| as_utf8(payload))
                .any(|text| !teloxide_url::find_urls(text).is_empty()),
            "no URL parsed from {name}"
        );
    }
}

#[test]
fn inverted_qr_can_be_decoded() {
    let bytes = include_bytes!("fixtures/inverted.png");
    assert!(!scan_image(bytes).unwrap().is_empty());
}

#[test]
fn finds_multiple_qrs() {
    let bytes = include_bytes!("fixtures/multiple.png");
    assert!(scan_image(bytes).unwrap().len() >= 2);
}

#[test]
fn finds_small_qr_in_screenshot_corner_after_bounded_multiscale_scan() {
    let qr = image::load_from_memory(include_bytes!("fixtures/normal.png"))
        .unwrap()
        .to_rgb8();
    let small = imageops::resize(&qr, 96, 96, imageops::FilterType::Nearest);
    let mut canvas: RgbImage = ImageBuffer::from_pixel(2400, 1600, Rgb([255, 255, 255]));
    imageops::overlay(&mut canvas, &small, 2260, 1460);
    let bytes = encode_png(canvas);
    assert!(!scan_image(&bytes).unwrap().is_empty());
}

#[test]
fn repeated_qr_payload_in_separate_regions_is_deduplicated() {
    let normal = image::load_from_memory(include_bytes!("fixtures/normal.png"))
        .unwrap()
        .to_rgb8();
    let inverted = image::load_from_memory(include_bytes!("fixtures/inverted.png"))
        .unwrap()
        .to_rgb8();
    let mut canvas: RgbImage = ImageBuffer::from_pixel(1600, 1000, Rgb([255, 255, 255]));
    imageops::overlay(&mut canvas, &normal, 10, 10);
    imageops::overlay(&mut canvas, &normal, 1370, 10);
    imageops::overlay(&mut canvas, &inverted, 1370, 770);
    let payloads = scan_image(&encode_png(canvas)).unwrap();
    assert_eq!(payloads.len(), 1, "same QR payload must be deduplicated");
}

#[test]
fn rejects_oversized_or_corrupt() {
    assert_eq!(
        scan_image(&vec![0; nedobot_qr::MAX_COMPRESSED_BYTES + 1]),
        Err(nedobot_qr::ScanError::TooLarge)
    );
    assert!(scan_image(b"broken").is_err());
}

#[test]
fn generated_valid_pngs_keep_qr_scan_output_bounded_and_deterministic() {
    let seed = std::env::var("NEDOBOT_PROPTEST_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0x14_01_02_03_04_05_06_07);
    let dimensions = (1_u32..=128, 1_u32..=128);
    let strategy = dimensions.prop_flat_map(|(width, height)| {
        prop::collection::vec(any::<u8>(), (width * height) as usize)
            .prop_map(move |pixels| (width, height, pixels))
    });
    let mut runner = TestRunner::new(Config {
        cases: 128,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    });

    runner
        .run(&strategy, |(width, height, pixels)| {
            let image = image::GrayImage::from_raw(width, height, pixels)
                .expect("generated pixel count matches dimensions");
            let bytes = encode_png(DynamicImage::ImageLuma8(image));
            let first = scan_image(&bytes).expect("bounded schema-valid PNG must decode safely");
            let replay = scan_image(&bytes).expect("same PNG must remain decodable");

            prop_assert_eq!(&first, &replay);
            prop_assert!(first.len() <= nedobot_qr::MAX_CODES);
            prop_assert!(
                first
                    .iter()
                    .all(|payload| payload.len() <= nedobot_qr::MAX_PAYLOAD_BYTES)
            );
            Ok(())
        })
        .unwrap_or_else(|error| {
            panic!(
                "case_id=qr-decoder-resource seed={seed} sha={} replay=sh scripts/replay_quality_case.sh qr-decoder-resource {seed}: {error}",
                std::env::var("NEDOBOT_TEST_SHA")
                    .unwrap_or_else(|_| "local-unpinned".to_owned())
            )
        });
}

fn encode_png(image: impl Into<DynamicImage>) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    image
        .into()
        .write_to(&mut output, ImageFormat::Png)
        .unwrap();
    output.into_inner()
}
