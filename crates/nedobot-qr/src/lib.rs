//! Декодирование QR отдельно от `teloxide-url` и модерации.
//! Вызывающая сторона скачивает медиа с собственными лимитами времени и сети.
use std::{collections::HashSet, io::Cursor};

use image::{ImageReader, Limits};

pub const MAX_COMPRESSED_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_SIDE: u32 = 4096;
pub const MAX_PIXELS: u64 = 16 * 1024 * 1024;
pub const MAX_CODES: usize = 16;
pub const MAX_PAYLOAD_BYTES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    TooLarge,
    Image(String),
}

/// Возвращает payload QR в виде байтов; это не обязательно UTF-8.
/// Повторный инвертированный проход помогает для QR с инверсными цветами.
/// Результаты обоих проходов дедуплицируются, все коды сохраняются.
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
    let width = gray.width() as usize;
    let height = gray.height() as usize;
    let mut pixels = gray.into_raw();
    for inverted in [false, true] {
        if inverted {
            for pixel in &mut pixels {
                *pixel = 255 - *pixel;
            }
        }
        for code in scanner.identify(width, height, &pixels) {
            if result.len() >= MAX_CODES {
                return Ok(result);
            }
            let Ok(code) = code else {
                continue;
            };
            let Ok(decoded) = code.decode() else {
                continue;
            };
            if decoded.payload.len() > MAX_PAYLOAD_BYTES {
                continue;
            }
            if unique.insert(decoded.payload.clone()) {
                result.push(decoded.payload);
            }
        }
    }
    Ok(result)
}

/// Вызывающая сторона решает, как обрабатывать не-UTF-8 данные. Lossy URL запрещены.
pub fn as_utf8(payload: &[u8]) -> Option<&str> {
    std::str::from_utf8(payload).ok()
}
