use nedobot_qr::{as_utf8, scan_image};

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
fn rejects_oversized_or_corrupt() {
    assert_eq!(
        scan_image(&vec![0; nedobot_qr::MAX_COMPRESSED_BYTES + 1]),
        Err(nedobot_qr::ScanError::TooLarge)
    );
    assert!(scan_image(b"broken").is_err());
}
