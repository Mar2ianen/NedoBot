use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use teloxide::{net::Download, prelude::*, types::FileId};
use tokio::io::AsyncWrite;

use crate::config::Config;

#[cfg(feature = "ask")]
pub(crate) async fn download_largest_photo_base64(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    config: &Config,
) -> anyhow::Result<Option<String>> {
    let image_file_id = msg
        .photo()
        .and_then(|photos| photos.iter().max_by_key(|photo| photo.width * photo.height))
        .map(|photo| photo.file.id.0.as_str());
    download_photo_base64(bot, image_file_id, config).await
}

pub(crate) async fn download_photo_base64(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    image_file_id: Option<&str>,
    config: &Config,
) -> anyhow::Result<Option<String>> {
    let Some(image_file_id) = image_file_id else {
        return Ok(None);
    };

    let file = bot.get_file(FileId(image_file_id.to_owned())).await?;
    let max_bytes = u64::from(config.first_comment_max_image_mb) * 1024 * 1024;
    if u64::from(file.size) > max_bytes {
        anyhow::bail!(
            "post image exceeds configured limit of {} MB",
            config.first_comment_max_image_mb
        );
    }
    let max_bytes =
        usize::try_from(max_bytes).map_err(|_| anyhow::anyhow!("image limit is too large"))?;
    let mut bytes = LimitedBytesWriter::new(max_bytes);
    bot.download_file(&file.path, &mut bytes).await?;

    Ok(Some(BASE64.encode(bytes.into_inner())))
}

struct LimitedBytesWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl LimitedBytesWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(1024 * 1024)),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl AsyncWrite for LimitedBytesWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "image exceeds configured download limit",
            )));
        }

        self.bytes.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn limited_bytes_writer_rejects_overflow() {
        let mut writer = LimitedBytesWriter::new(4);
        writer.write_all(b"1234").await.unwrap();
        let err = writer.write_all(b"5").await.unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::WriteZero);
        assert_eq!(writer.into_inner(), b"1234");
    }
}
