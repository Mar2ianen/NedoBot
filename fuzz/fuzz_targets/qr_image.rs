#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if input.len() > 256 * 1024 {
        return;
    }
    if let Ok(payloads) = nedobot_qr::scan_image(input) {
        assert!(payloads.len() <= nedobot_qr::MAX_CODES);
        assert!(payloads.iter().all(|p| p.len() <= nedobot_qr::MAX_PAYLOAD_BYTES));
    }
});
