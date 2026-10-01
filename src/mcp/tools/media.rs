//! Bounded photo and document delivery for public-chat messages.

use std::{
    env, io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rmcp::{
    ErrorData,
    model::{CallToolResult, ContentBlock, ResourceContents},
    schemars::{self, JsonSchema},
};
use serde::Deserialize;
use teloxide::{Bot, net::Download, prelude::Requester, types::FileId};
use tokio::io::AsyncWrite;

use crate::features::chat_read_api::{ChatReadApi, types::ChatMediaAttachment};

use super::{invalid_arguments, read_error};

pub const ENABLED_ENV: &str = "MCP_MEDIA_ENABLED";
pub const TELEGRAM_BOT_TOKEN_ENV: &str = "MCP_TELEGRAM_BOT_TOKEN";

const MAX_MEDIA_BYTES: u64 = 5 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct TelegramMediaConfig {
    bot: Bot,
}

impl TelegramMediaConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let enabled = env::var_os(ENABLED_ENV)
            .map(|value| {
                value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("{ENABLED_ENV} must be a valid Unicode boolean"))
            })
            .transpose()?
            .unwrap_or_else(|| "false".to_owned());
        let enabled = match enabled.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => anyhow::bail!("{ENABLED_ENV} must be a boolean"),
        };
        if !enabled {
            return Ok(None);
        }

        let token = env::var(TELEGRAM_BOT_TOKEN_ENV).map_err(|_| {
            anyhow::anyhow!("{TELEGRAM_BOT_TOKEN_ENV} is required when media is enabled")
        })?;
        let valid_token = token.split_once(':').is_some_and(|(bot_id, secret)| {
            !bot_id.is_empty()
                && bot_id.bytes().all(|byte| byte.is_ascii_digit())
                && !secret.is_empty()
                && secret.len() <= 256
                && secret
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
        anyhow::ensure!(
            valid_token,
            "{TELEGRAM_BOT_TOKEN_ENV} has an invalid format"
        );

        Ok(Some(Self {
            bot: Bot::new(token),
        }))
    }

    async fn download(
        &self,
        file_id: &str,
        declared_size: Option<i64>,
    ) -> Result<Vec<u8>, DownloadError> {
        if declared_size.is_some_and(|size| size > MAX_MEDIA_BYTES as i64) {
            return Err(DownloadError::TooLarge(
                declared_size.unwrap_or_default() as u64
            ));
        }
        tokio::time::timeout(HTTP_TIMEOUT, self.download_bounded(file_id))
            .await
            .map_err(|_| DownloadError::Provider)?
    }

    async fn download_bounded(&self, file_id: &str) -> Result<Vec<u8>, DownloadError> {
        let file = self
            .bot
            .get_file(FileId(file_id.to_owned()))
            .await
            .map_err(|_| DownloadError::Provider)?;
        if file.size as u64 > MAX_MEDIA_BYTES {
            return Err(DownloadError::TooLarge(file.size as u64));
        }

        let mut writer = BoundedMediaWriter::new(MAX_MEDIA_BYTES as usize);
        if self
            .bot
            .download_file(&file.path, &mut writer)
            .await
            .is_err()
        {
            if let Some(size) = writer.exceeded_at {
                return Err(DownloadError::TooLarge(size as u64));
            }
            return Err(DownloadError::Provider);
        }
        Ok(writer.bytes)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetMediaInput {
    /// ID of a public-chat message containing a photo or a document.
    pub message_id: i32,
}

enum DownloadError {
    TooLarge(u64),
    Provider,
}

pub async fn get_media(
    api: &ChatReadApi,
    config: Option<&TelegramMediaConfig>,
    input: GetMediaInput,
) -> Result<CallToolResult, ErrorData> {
    if input.message_id <= 0 {
        return Err(invalid_arguments("message_id must be positive"));
    }
    let Some(config) = config else {
        return Ok(tool_error("Раздача файлов через MCP выключена."));
    };

    let attachments = api
        .message_media(input.message_id)
        .await
        .map_err(|_| read_error("media metadata lookup failed"))?;
    if attachments.is_empty() {
        return Ok(tool_error(
            "В сообщении нет доступной фотографии или небольшого файла.",
        ));
    }

    let mut content = Vec::with_capacity(attachments.len() * 2);
    for attachment in attachments {
        append_attachment(&mut content, config, attachment).await;
    }
    if content
        .iter()
        .all(|block| matches!(block, ContentBlock::Text(_)))
    {
        return Ok(CallToolResult::error(content));
    }
    Ok(CallToolResult::success(content))
}

async fn append_attachment(
    content: &mut Vec<ContentBlock>,
    config: &TelegramMediaConfig,
    attachment: ChatMediaAttachment,
) {
    let display_name = attachment_name(&attachment);
    match config
        .download(&attachment.file_id, attachment.file_size)
        .await
    {
        Ok(bytes) => match attachment.media_kind.as_str() {
            "photo" => {
                content.push(ContentBlock::text(format!(
                    "Фото из сообщения {} ({} байт)",
                    attachment.message_id,
                    bytes.len()
                )));
                content.push(ContentBlock::image(BASE64.encode(bytes), "image/jpeg"));
            }
            "document" => {
                let uri = format!(
                    "nedonews://telegram-media/{}/document",
                    attachment.message_id
                );
                content.push(ContentBlock::text(format!(
                    "Файл {display_name} из сообщения {} ({} байт)",
                    attachment.message_id,
                    bytes.len()
                )));
                content.push(ContentBlock::resource(
                    ResourceContents::blob(BASE64.encode(bytes), uri)
                        .with_mime_type("application/octet-stream"),
                ));
            }
            _ => content.push(ContentBlock::text(
                "Этот тип вложения не поддерживается.",
            )),
        },
        Err(DownloadError::TooLarge(size)) => content.push(ContentBlock::text(format!(
            "Файл {display_name} пропущен: размер {size} байт превышает лимит {MAX_MEDIA_BYTES} байт."
        ))),
        Err(DownloadError::Provider) => content.push(ContentBlock::text(format!(
            "Не удалось получить {display_name} из Telegram."
        ))),
    }
}

fn attachment_name(attachment: &ChatMediaAttachment) -> String {
    if attachment.media_kind == "photo" {
        return format!("message-{}.jpg", attachment.message_id);
    }
    let name = attachment
        .file_name
        .as_deref()
        .unwrap_or("document.bin")
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("document.bin")
        .chars()
        .filter(|character| !character.is_control())
        .take(120)
        .collect::<String>();
    if name.trim().is_empty() {
        "document.bin".to_owned()
    } else {
        name
    }
}

struct BoundedMediaWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    exceeded_at: Option<usize>,
}

impl BoundedMediaWriter {
    fn new(max_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            max_bytes,
            exceeded_at: None,
        }
    }
}

impl AsyncWrite for BoundedMediaWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        let Some(next_size) = self.bytes.len().checked_add(buffer.len()) else {
            self.exceeded_at = Some(usize::MAX);
            return Poll::Ready(Err(io::Error::other("media exceeds configured size limit")));
        };
        if next_size > self.max_bytes {
            self.exceeded_at = Some(next_size);
            return Poll::Ready(Err(io::Error::other("media exceeds configured size limit")));
        }
        self.bytes.extend_from_slice(buffer);
        Poll::Ready(Ok(buffer.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn tool_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}
