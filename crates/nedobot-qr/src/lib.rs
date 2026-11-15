//! Декодирование QR отдельно от `teloxide-url` и модерации.
//! Вызывающая сторона скачивает медиа с собственными лимитами времени и сети.
use std::{collections::HashSet, io::Cursor};

use image::{GrayImage, ImageReader, Limits, imageops};

pub const MAX_COMPRESSED_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_SIDE: u32 = 4096;
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
pub const MAX_CODES: usize = 16;
pub const MAX_PAYLOAD_BYTES: usize = 4096;
/// Hard CPU budget across full-frame, inverted, tiled, and upscaled scans.
pub const MAX_TOTAL_SCAN_PIXELS: u64 = 104 * 1024 * 1024;
const MAX_CANDIDATES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    TooLarge,
    Image(String),
}

/// Возвращает payload QR в виде байтов; это не обязательно UTF-8.
/// The original, inverted image, bounded overlapping crops, and upscaled corner
/// crops are scanned under a total pixel budget. Results are deduplicated.
pub fn scan_image(bytes: &[u8]) -> Result<Vec<Vec<u8>>, ScanError> {
    if bytes.len() > MAX_COMPRESSED_BYTES {
        return Err(ScanError::TooLarge);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ScanError::Image(e.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| ScanError::Image(e.to_string()))?;
    if u64::from(img.width()) * u64::from(img.height()) > MAX_PIXELS {
        return Err(ScanError::TooLarge);
    }
    let gray = img.into_luma8();
    let mut scanner = quircs::Quirc::default();
    let mut unique = HashSet::<Vec<u8>>::new();
    let mut result = Vec::new();
    let mut scanned_pixels = 0_u64;
    let mut candidate_count = 0_usize;

    scan_pass(
        &gray,
        false,
        &mut scanner,
        &mut unique,
        &mut result,
        &mut scanned_pixels,
        &mut candidate_count,
    );
    scan_pass(
        &gray,
        true,
        &mut scanner,
        &mut unique,
        &mut result,
        &mut scanned_pixels,
        &mut candidate_count,
    );

    let tile_width = (gray.width() * 2 / 5).max(1);
    let tile_height = (gray.height() * 2 / 5).max(1);
    let max_x = gray.width().saturating_sub(tile_width);
    let max_y = gray.height().saturating_sub(tile_height);
    let tiles = [
        (0, 0),
        (max_x, 0),
        (0, max_y),
        (max_x, max_y),
        (max_x / 2, max_y / 2),
    ];
    let mut seen_tiles = HashSet::new();
    for (tile_index, (x, y)) in tiles.into_iter().enumerate() {
        if !seen_tiles.insert((x, y)) || result.len() >= MAX_CODES {
            continue;
        }
        let tile = imageops::crop_imm(&gray, x, y, tile_width, tile_height).to_image();
        scan_pass(
            &tile,
            false,
            &mut scanner,
            &mut unique,
            &mut result,
            &mut scanned_pixels,
            &mut candidate_count,
        );
        scan_pass(
            &tile,
            true,
            &mut scanner,
            &mut unique,
            &mut result,
            &mut scanned_pixels,
            &mut candidate_count,
        );

        // Enlarging just the four bounded corners lets the decoder resolve
        // small QR modules without multiplying work across every crop.
        if tile_index < 4 && result.len() < MAX_CODES {
            let scaled = imageops::resize(
                &tile,
                tile_width.saturating_mul(2),
                tile_height.saturating_mul(2),
                imageops::FilterType::Nearest,
            );
            scan_pass(
                &scaled,
                false,
                &mut scanner,
                &mut unique,
                &mut result,
                &mut scanned_pixels,
                &mut candidate_count,
            );
        }
    }
    Ok(result)
}

fn scan_pass(
    image: &GrayImage,
    inverted: bool,
    scanner: &mut quircs::Quirc,
    unique: &mut HashSet<Vec<u8>>,
    result: &mut Vec<Vec<u8>>,
    scanned_pixels: &mut u64,
    candidate_count: &mut usize,
) {
    if result.len() >= MAX_CODES || *candidate_count >= MAX_CANDIDATES {
        return;
    }
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if scanned_pixels.saturating_add(pixels) > MAX_TOTAL_SCAN_PIXELS {
        return;
    }
    *scanned_pixels += pixels;
    let inverted_pixels;
    let pixels = if inverted {
        inverted_pixels = image
            .as_raw()
            .iter()
            .map(|pixel| 255 - pixel)
            .collect::<Vec<_>>();
        inverted_pixels.as_slice()
    } else {
        image.as_raw().as_slice()
    };
    for candidate in scanner.identify(image.width() as usize, image.height() as usize, pixels) {
        if *candidate_count >= MAX_CANDIDATES || result.len() >= MAX_CODES {
            break;
        }
        *candidate_count += 1;
        let Ok(candidate) = candidate else {
            continue;
        };
        let Ok(decoded) = candidate.decode() else {
            continue;
        };
        if decoded.payload.len() <= MAX_PAYLOAD_BYTES && unique.insert(decoded.payload.clone()) {
            result.push(decoded.payload);
        }
    }
}

/// Вызывающая сторона решает, как обрабатывать не-UTF-8 данные. Lossy URL запрещены.
pub fn as_utf8(payload: &[u8]) -> Option<&str> {
    std::str::from_utf8(payload).ok()
}
