#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if input.len() > 16 * 1024 {
        return;
    }
    if let Ok(text) = std::str::from_utf8(input) {
        let _ = teloxide_url::find_urls(text);
    }
});
