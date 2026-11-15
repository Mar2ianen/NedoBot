//! Локальная проверка: cargo run --example scan_links -- ./tests/fixtures/screenshot.png
//! Не выводите реальные секреты и приватные URL в общие логи.
use nedobot_qr::{as_utf8, scan_image};
use std::{env, fs};
use teloxide_url::{Kind, find, parse_telegram_link};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args().nth(1).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: scan_links <image.png>",
        )
    })?;
    let bytes = fs::read(path)?;
    let decoded =
        scan_image(&bytes).map_err(|e| std::io::Error::other(format!("QR scan error: {e:?}")))?;
    println!("{} unique QR payloads", decoded.len());
    for (index, qr) in decoded.iter().enumerate() {
        let Some(text) = as_utf8(qr) else {
            println!("code {index}: not UTF-8");
            continue;
        };
        let links = find(text);
        for link in links {
            if link.kind != Kind::Url {
                continue;
            }
            // Выводим только тип ссылки: URL может содержать секреты.
            println!(
                "code {index}: {}",
                if parse_telegram_link(&link.value).is_some() {
                    "telegram-link"
                } else {
                    "web-link"
                }
            );
        }
    }
    Ok(())
}
