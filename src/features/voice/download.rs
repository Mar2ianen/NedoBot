use std::path::PathBuf;

use teloxide::net::Download;
use teloxide::prelude::*;
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;

use crate::config::Config;
use crate::features::voice::types::{VoiceMedia, VoiceMediaKind};

pub struct DownloadedMedia {
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
}

impl VoiceDownloadSkip {
    pub fn user_message(&self) -> String {
        match self {
            Self::TooLong {
                duration_sec,
                max_sec,
            } => format!(
                "Запись слишком длинная: {}. Сейчас лимит {}.",
                format_duration(*duration_sec),
                format_duration(*max_sec)
            ),
            Self::TooLarge { size, max_size } => format!(
                "Запись слишком большая: {} MB. Сейчас лимит {} MB.",
                bytes_to_mb(*size),
                bytes_to_mb(*max_size)
            ),
        }
    }
}

pub fn validate_media(media: &VoiceMedia, config: &Config) -> Result<(), VoiceDownloadSkip> {
    validate_limits(
        media.duration_sec,
        media.file_size,
        config.voice_max_duration_sec,
        config.voice_max_file_mb,
    )
}

fn validate_limits(
    duration_sec: Option<u32>,
    file_size: Option<u64>,
    max_duration_sec: u32,
    max_file_mb: u32,
) -> Result<(), VoiceDownloadSkip> {
    if let Some(duration_sec) = duration_sec
        && duration_sec > max_duration_sec
    {
        return Err(VoiceDownloadSkip::TooLong {
            duration_sec,
            max_sec: max_duration_sec,
        });
    }

    if let Some(file_size) = file_size {
        let max_size = max_file_mb as u64 * 1024 * 1024;
        if file_size > max_size {
            return Err(VoiceDownloadSkip::TooLarge {
                size: file_size,
                max_size,
            });
        }
    }

    Ok(())
}

pub async fn download_media_file(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    media: &VoiceMedia,
) -> anyhow::Result<DownloadedMedia> {
    let file = bot.get_file(media.file_id.clone()).await?;
    let suffix = file_suffix(media);
    let named = tempfile::Builder::new()
        .prefix("tg-media-")
        .suffix(&suffix)
        .tempfile()?;
    let temp_path = named.into_temp_path();
    let path = temp_path.to_path_buf();

    let mut dst = tokio::fs::File::create(&path).await?;
    bot.download_file(&file.path, &mut dst).await?;
    dst.flush().await?;

    let size = tokio::fs::metadata(&path).await?.len();
    Ok(DownloadedMedia {
        path,
        filename: format!("telegram-{}{}", media.kind.as_str(), file_suffix(media)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::voice::types::VIDEO_NOTE_MIME_TYPE;

    fn media(
        kind: VoiceMediaKind,
        duration_sec: Option<u32>,
        file_size: Option<u64>,
        mime_type: Option<&str>,
    ) -> VoiceMedia {
        VoiceMedia {
            chat_id: -100,
            message_id: 42,
            user_id: Some(7),
            kind,
            file_id: "file-id".to_string(),
            file_unique_id: Some("unique-id".to_string()),
            duration_sec,
            file_size,
            mime_type: mime_type.map(ToString::to_string),
        }
    }

    #[test]
    fn video_note_uses_mp4_file_extension() {
        let media = media(
            VoiceMediaKind::VideoNote,
            Some(60),
            Some(20 * 1024 * 1024),
            Some(VIDEO_NOTE_MIME_TYPE),
        );

        assert_eq!(file_suffix(&media), ".mp4");
    }

    #[test]
    fn video_note_keeps_duration_and_size_limits() {
        assert_eq!(
            validate_limits(Some(61), Some(1024), 60, 20),
            Err(VoiceDownloadSkip::TooLong {
                duration_sec: 61,
                max_sec: 60,
            })
        );
        assert_eq!(
            validate_limits(Some(1), Some(20 * 1024 * 1024 + 1), 60, 20),
            Err(VoiceDownloadSkip::TooLarge {
                size: 20 * 1024 * 1024 + 1,
                max_size: 20 * 1024 * 1024,
            })
        );
    }
}
