use std::{
    io,
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
};

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
        config.voice.max_duration_sec,
        config.voice.max_file_mb,
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
    max_size_bytes: u64,
) -> anyhow::Result<DownloadedMedia> {
    let file = bot
        .get_file(teloxide::types::FileId(media.file_id.clone()))
        .await?;
    let suffix = file_suffix(media);
    let named = tempfile::Builder::new()
        .prefix("tg-media-")
        .suffix(&suffix)
        .tempfile()?;
    let temp_path = named.into_temp_path();
    let path = temp_path.to_path_buf();

    let mut dst = tokio::fs::File::create(&path).await?;
    let size = {
        let mut bounded = LimitedWriter::new(&mut dst, max_size_bytes);
        bot.download_file(&file.path, &mut bounded).await?;
        bounded.written
    };
    dst.flush().await?;

    Ok(DownloadedMedia {
        path,
        filename: format!("telegram-{}{}", media.kind.as_str(), file_suffix(media)),
        mime_type: media.mime_type.clone(),
        size,
        _temp_path: temp_path,
    })
}

struct LimitedWriter<W> {
    inner: W,
    written: u64,
    limit: u64,
}

impl<W> LimitedWriter<W> {
    fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            written: 0,
            limit,
        }
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for LimitedWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buffer.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let remaining = this.limit.saturating_sub(this.written);
        if remaining == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "download exceeds configured media size limit",
            )));
        }
        let remaining = usize::try_from(remaining).unwrap_or(usize::MAX);
        let write_len = buffer.len().min(remaining);
        match Pin::new(&mut this.inner).poll_write(context, &buffer[..write_len]) {
            Poll::Ready(Ok(written)) => {
                this.written += written as u64;
                Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(context)
    }

    fn poll_shutdown(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(context)
    }
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
    use axum::{
        Json, Router,
        http::Uri,
        response::{IntoResponse, Response},
        routing::any,
    };
    use proptest::{
        prelude::*,
        test_runner::{Config, RngSeed, TestRunner},
    };
    use serde_json::json;

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

    #[test]
    fn generated_media_metadata_matches_independent_size_and_duration_oracle() {
        let seed = std::env::var("NEDOBOT_PROPTEST_SEED")
            .map(|value| value.parse::<u64>().expect("seed must be unsigned"))
            .unwrap_or(0x0706_0504_0302_0100);
        let strategy = (
            1_u32..100_000,
            -1_i32..=1,
            1_u32..=20,
            -1_i32..=1,
            proptest::option::of(0_u8..=1),
        )
            .prop_map(
                |(duration_limit, duration_delta, max_file_mb, size_delta, known_size)| {
                    let duration = (i64::from(duration_limit) + i64::from(duration_delta)) as u32;
                    let max_bytes = u64::from(max_file_mb) * 1024 * 1024;
                    let size = match size_delta {
                        -1 => max_bytes - 1,
                        0 => max_bytes,
                        _ => max_bytes + 1,
                    };
                    (
                        duration_limit,
                        duration,
                        max_file_mb,
                        known_size.map(|_| size),
                    )
                },
            );
        let mut runner = TestRunner::new(Config {
            cases: 256,
            rng_seed: RngSeed::Fixed(seed),
            failure_persistence: None,
            ..Config::default()
        });
        runner
            .run(&strategy, |(duration_limit, duration, max_file_mb, file_size)| {
                let max_bytes = u64::from(max_file_mb) * 1024 * 1024;
                let expected = if duration > duration_limit {
                    Err(VoiceDownloadSkip::TooLong {
                        duration_sec: duration,
                        max_sec: duration_limit,
                    })
                } else if file_size.is_some_and(|size| size > max_bytes) {
                    Err(VoiceDownloadSkip::TooLarge {
                        size: file_size.unwrap(),
                        max_size: max_bytes,
                    })
                } else {
                    Ok(())
                };
                prop_assert_eq!(
                    validate_limits(Some(duration), file_size, duration_limit, max_file_mb),
                    expected
                );
                Ok(())
            })
            .unwrap_or_else(|error| {
                panic!("case_id=voice-media-bounds seed={seed} replay=sh scripts/replay_quality_case.sh voice-media-bounds {seed}: {error}")
            });
    }

    #[tokio::test]
    async fn downloaded_bytes_are_bounded_even_when_telegram_size_is_missing_or_wrong() {
        async fn fake_telegram(uri: Uri) -> Response {
            if uri
                .path()
                .rsplit('/')
                .next()
                .is_some_and(|method| method.eq_ignore_ascii_case("getFile"))
            {
                return Json(json!({
                    "ok": true,
                    "result": {
                        "file_id": "file-id",
                        "file_unique_id": "unique-id",
                        "file_size": 1,
                        "file_path": "media/test.oga"
                    }
                }))
                .into_response();
            }
            b"0123456789".as_slice().into_response()
        }

        let app = Router::new().route("/{*path}", any(fake_telegram));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("local fake Telegram must bind");
        let address = listener.local_addr().expect("fake address must exist");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fake Telegram must run");
        });
        let bot = Bot::new("123456:TEST_TOKEN")
            .set_api_url(format!("http://{address}/").parse().expect("fake URL"))
            .parse_mode(teloxide::types::ParseMode::Html);
        let mut media = media(VoiceMediaKind::Voice, Some(1), None, Some("audio/ogg"));
        media.file_size = None;

        let result = download_media_file(&bot, &media, 4).await;
        server.abort();
        assert!(
            result.is_err(),
            "oversized actual response must be rejected"
        );
    }

    #[tokio::test]
    async fn limited_writer_never_persists_bytes_after_its_cap() {
        use tokio::io::AsyncWriteExt;

        let file = tempfile::NamedTempFile::new().expect("temporary output must be created");
        let path = file.path().to_owned();
        let mut output = tokio::fs::File::from_std(file.reopen().expect("file must reopen"));
        let (error_kind, written) = {
            let mut bounded = LimitedWriter::new(&mut output, 4);
            let error = bounded
                .write_all(b"0123456789")
                .await
                .expect_err("writer must reject the first byte beyond its cap");
            (error.kind(), bounded.written)
        };
        assert_eq!(error_kind, io::ErrorKind::FileTooLarge);
        assert_eq!(written, 4);
        output.flush().await.expect("written prefix must flush");
        assert_eq!(tokio::fs::metadata(path).await.unwrap().len(), 4);
    }
}
