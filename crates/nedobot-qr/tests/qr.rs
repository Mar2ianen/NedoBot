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
    let seed = property_seed();
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

fn property_seed() -> u64 {
    match std::env::var("NEDOBOT_PROPTEST_SEED") {
        Ok(value) => value.parse().expect("invalid property seed"),
        Err(std::env::VarError::NotPresent) => 0x14_01_02_03_04_05_06_07,
        Err(error) => panic!("invalid property seed: {error}"),
    }
}

// Generated independently with Python qrcode (version 2, ECC M, border 4).
// The expected payload is the encoder input, never the decoder's own output.
const KNOWN_QR: &str = "\
000000000000000000000000000000000\n\
000000000000000000000000000000000\n\
000000000000000000000000000000000\n\
000000000000000000000000000000000\n\
000011111110111111011011111110000\n\
000010000010011010101010000010000\n\
000010111010011010000010111010000\n\
000010111010101011101010111010000\n\
000010111010111001111010111010000\n\
000010000010101010000010000010000\n\
000011111110101010101011111110000\n\
000000000000101110100000000000000\n\
000010001011111100110111110010000\n\
000010101101111010110100110100000\n\
000011011010101111110111011000000\n\
000011101101110100110101001100000\n\
000011101111011100000111011110000\n\
000011110001100011011000100100000\n\
000000001011100110011101111000000\n\
000000010101000100110001101100000\n\
000011100011111010011111111000000\n\
000000000000110001001000100000000\n\
000011111110110001001010100000000\n\
000010000010011010001000111110000\n\
000010111010111101101111111110000\n\
000010111010001010100111001110000\n\
000010111010000111111110010100000\n\
000010000010001101000011111100000\n\
000011111110110011010110001110000\n\
000000000000000000000000000000000\n\
000000000000000000000000000000000\n\
000000000000000000000000000000000\n\
000000000000000000000000000000000";

#[test]
fn generated_qr_transforms_preserve_independently_encoded_payload() {
    let seed = property_seed();
    let mut runner = TestRunner::new(Config {
        cases: 64,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(
            &(4_u32..=8, 0_u8..4, 0_u8..=40, 210_u8..=255),
            |(scale, rotation, dark, light)| {
                let bits: Vec<_> = KNOWN_QR.lines().flat_map(str::bytes).collect();
                prop_assert_eq!(bits.len(), 33 * 33);
                let qr = image::GrayImage::from_fn(33, 33, |x, y| {
                    image::Luma([if bits[(y * 33 + x) as usize] == b'1' {
                        dark
                    } else {
                        light
                    }])
                });
                let qr =
                    imageops::resize(&qr, 33 * scale, 33 * scale, imageops::FilterType::Nearest);
                let qr = match rotation {
                    0 => qr,
                    1 => imageops::rotate90(&qr),
                    2 => imageops::rotate180(&qr),
                    _ => imageops::rotate270(&qr),
                };
                let payloads = scan_image(&encode_png(DynamicImage::ImageLuma8(qr))).unwrap();
                prop_assert_eq!(payloads, vec![b"https://example.org/qr".to_vec()]);
                Ok(())
            },
        )
        .unwrap_or_else(|error| panic!("case_id=qr-known-payload seed={seed}: {error}"));
}

fn encode_png(image: impl Into<DynamicImage>) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    image
        .into()
        .write_to(&mut output, ImageFormat::Png)
        .unwrap();
    output.into_inner()
}
