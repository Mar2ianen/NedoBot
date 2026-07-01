use std::path::{Path, PathBuf};

use teloxide::net::Download;
use teloxide::prelude::*;
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

use crate::config::Config;
use crate::features::voice::types::{VoiceMedia, VoiceMediaKind};

pub struct DownloadedVoice {
    pub path: PathBuf,
    pub filename: String,
    pub mime_type: Option<String>,
    pub size: u64,
    _temp_path: TempPath,
}

#[derive(Debug, Eq, PartialEq)]
pub enum VoiceDownloadSkip {
    TooLong { duration_sec: u32, max_sec: u32 },
    TooLarge { size: u64, max_size: u64 },
    UnsupportedVideoNote,
}

impl VoiceDownloadSkip {
    pub fn user_message(&self) -> String {
        match self {
            Self::TooLong {
                duration_sec,
                max_sec,
            } => format!(
                "Голосовое слишком длинное: {}. Сейчас лимит {}.",
                format_duration(*duration_sec),
                format_duration(*max_sec)
            ),
            Self::TooLarge { size, max_size } => format!(
                "Файл слишком большой: {} MB. Сейчас лимит {} MB.",
                bytes_to_mb(*size),
                bytes_to_mb(*max_size)
            ),
            Self::UnsupportedVideoNote => {
                "Кружки пока не расшифровываю: для них нужен отдельный аудио-extract.".to_string()
            }
        }
    }
}

pub fn validate_media(media: &VoiceMedia, config: &Config) -> Result<(), VoiceDownloadSkip> {
    if media.kind == VoiceMediaKind::VideoNote {
        return Err(VoiceDownloadSkip::UnsupportedVideoNote);
    }

    if let Some(duration_sec) = media.duration_sec {
        if duration_sec > config.voice_max_duration_sec {
            return Err(VoiceDownloadSkip::TooLong {
                duration_sec,
                max_sec: config.voice_max_duration_sec,
            });
        }
    }

    if let Some(file_size) = media.file_size {
        let max_size = config.voice_max_file_mb as u64 * 1024 * 1024;
        if file_size > max_size {
            return Err(VoiceDownloadSkip::TooLarge {
                size: file_size,
                max_size,
            });
        }
    }

    Ok(())
}

pub async fn download_voice_file(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    media: &VoiceMedia,
) -> anyhow::Result<DownloadedVoice> {
    let file = bot.get_file(media.file_id.clone()).await?;
    let suffix = file_suffix(media);
    let named = tempfile::Builder::new()
        .prefix("tg-voice-")
        .suffix(&suffix)
        .tempfile()?;
    let temp_path = named.into_temp_path();
    let path = temp_path.to_path_buf();

    let mut dst = tokio::fs::File::create(&path).await?;
    bot.download_file(&file.path, &mut dst).await?;
    dst.flush().await?;

    let size = tokio::fs::metadata(&path).await?.len();
    Ok(DownloadedVoice {
        path,
        filename: format!("voice{}", file_suffix(media)),
        mime_type: media.mime_type.clone(),
        size,
        _temp_path: temp_path,
    })
}

fn file_suffix(media: &VoiceMedia) -> String {
    let Some(mime_type) = media.mime_type.as_deref() else {
        return match media.kind {
            VoiceMediaKind::Voice => ".ogg".to_string(),
            VoiceMediaKind::Audio => ".mp3".to_string(),
            VoiceMediaKind::VideoNote => ".mp4".to_string(),
        };
    };

    match mime_type {
        "audio/ogg" | "audio/opus" | "application/ogg" => ".ogg",
        "audio/mpeg" | "audio/mp3" => ".mp3",
        "audio/mp4" | "audio/x-m4a" => ".m4a",
        "audio/wav" | "audio/x-wav" => ".wav",
        "audio/flac" => ".flac",
        "video/mp4" => ".mp4",
        "video/webm" => ".webm",
        _ => ".bin",
    }
    .to_string()
}

fn bytes_to_mb(size: u64) -> u64 {
    size.div_ceil(1024 * 1024)
}

fn format_duration(seconds: u32) -> String {
    let minutes = seconds / 60;
    let rest = seconds % 60;
    if minutes == 0 {
        format!("{rest} сек.")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

#[allow(dead_code)]
fn _path_exists(path: &Path) -> bool {
    path.exists()
}
