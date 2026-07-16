use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use base64::Engine;
use teloxide::net::Download;
use teloxide::prelude::*;
use tokio::io::AsyncWrite;

const MAX_PROFILE_AVATAR_BYTES: usize = 10 * 1024 * 1024;

pub struct CachedProfileAvatar {
    filename: String,
    // The next classifier slice reads the cached image through `base64()`.
    #[allow(dead_code)]
    path: PathBuf,
}

impl CachedProfileAvatar {
    pub fn filename(&self) -> &str {
        &self.filename
    }

    #[allow(dead_code)] // Public for the avatar classifier added in the next slice.
    pub async fn base64(&self) -> anyhow::Result<String> {
        let bytes = tokio::fs::read(&self.path).await?;
        if bytes.len() > MAX_PROFILE_AVATAR_BYTES {
            anyhow::bail!("cached profile avatar exceeds byte limit");
        }
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

pub async fn cache_profile_avatar(
    bot: &Bot,
    static_files_dir: &str,
    user_id: i64,
    file_id: Option<&str>,
    unique_id: Option<&str>,
) -> anyhow::Result<Option<CachedProfileAvatar>> {
    let Some(file_id) = file_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };

    let avatars_dir = Path::new(static_files_dir).join("avatars");
    let filename = format!(
        "{}_{}.jpg",
        user_id,
        safe_static_name(unique_id.unwrap_or("photo"))
    );
    let path = avatars_dir.join(&filename);

    if tokio::fs::metadata(&path).await.is_err() {
        download_profile_avatar(bot, file_id, &avatars_dir, &path).await?;
    }

    Ok(Some(CachedProfileAvatar { filename, path }))
}

async fn download_profile_avatar(
    bot: &Bot,
    file_id: &str,
    avatars_dir: &Path,
    path: &Path,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(avatars_dir).await?;
    let file = bot.get_file(file_id.to_string()).await?;
    if usize::try_from(file.size).unwrap_or(usize::MAX) > MAX_PROFILE_AVATAR_BYTES {
        anyhow::bail!("profile avatar exceeds byte limit");
    }

    let mut bytes = LimitedBytesWriter::new(MAX_PROFILE_AVATAR_BYTES);
    bot.download_file(&file.path, &mut bytes).await?;
    let bytes = bytes.into_inner();
    if bytes.is_empty() {
        anyhow::bail!("downloaded profile avatar is empty");
    }

    let tmp_path = path.with_extension("tmp");
    tokio::fs::write(&tmp_path, bytes).await?;
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

fn safe_static_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect::<String>()
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
                "profile avatar exceeds byte limit",
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

    #[test]
    fn bounded_writer_rejects_overflow() {
        let mut writer = LimitedBytesWriter::new(3);
        assert!(matches!(
            Pin::new(&mut writer)
                .poll_write(&mut Context::from_waker(std::task::Waker::noop()), b"abcd"),
            Poll::Ready(Err(_))
        ));
    }

    #[test]
    fn static_filename_removes_unsafe_characters() {
        assert_eq!(safe_static_name("a/b:c_1"), "abc_1");
    }
}
