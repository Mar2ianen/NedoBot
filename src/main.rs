use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, postgres::PgPoolOptions};
use teloxide::{
    dispatching::UpdateFilterExt,
    net::Download,
    prelude::*,
    requests::RequesterExt,
    types::{
        LinkPreviewOptions, MessageEntityKind, MessageId, MessageOrigin, ParseMode, ReplyParameters,
    },
    utils::command::BotCommands,
};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase")]
enum Command {
    #[command(description = "показать это меню")]
    Help,
    #[command(description = "проверить, что бот жив")]
    Ping,
    #[command(description = "проверить подключение к базе")]
    Db,
    #[command(description = "показать custom_emoji_id из сообщения")]
    EmojiIds,
    #[command(description = "проверить формат первого комментария")]
    FormatTest(String),
}

#[derive(Clone)]
struct Config {
    source_channel_id: i64,
    discussion_chat_id: i64,
    chat_invite_url: String,
    chat_invite_label: String,
    post_signature_marker: String,
    ollama_base_url: String,
    ollama_api_key: String,
    vision_model: String,
    owner_telegram_id: Option<i64>,
    send_owner_preview: bool,
    comment_custom_emoji_id: Option<String>,
    tech_custom_emoji_id: Option<String>,
    amd_custom_emoji_id: Option<String>,
    radeon_custom_emoji_id: Option<String>,
    ryzen_custom_emoji_id: Option<String>,
}

struct CommentCandidate<'a> {
    source_channel_id: i64,
    source_message_id: MessageId,
    post_text: &'a str,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,teloxide=info".into()),
        )
        .init();

    let bot = Bot::from_env().parse_mode(ParseMode::Html);
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let config = Config::from_env();

    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(handle_command),
        )
        .branch(dptree::endpoint(handle_message));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![pool, config])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

impl Config {
    fn from_env() -> Self {
        Self {
            source_channel_id: env_i64("SOURCE_CHANNEL_ID", -1001575496091),
            discussion_chat_id: env_i64("DISCUSSION_CHAT_ID", -1001932061163),
            chat_invite_url: env_or("CHAT_INVITE_URL", "https://t.me/+RxmPtw7Bs-IxNzEy"),
            chat_invite_label: env_or("CHAT_INVITE_LABEL", "Присоединяйтесь к чату"),
            post_signature_marker: env_or("POST_SIGNATURE_MARKER", "Не теряем связь"),
            ollama_base_url: env_or("OLLAMA_BASE_URL", "https://ollama.com"),
            ollama_api_key: env_or("OLLAMA_API_KEY", ""),
            vision_model: env_optional("VISION_MODEL")
                .or_else(|| env_optional("OLLAMA_MODEL"))
                .unwrap_or_else(|| "gemma4:31b".to_string()),
            owner_telegram_id: env_optional("OWNER_TELEGRAM_ID")
                .and_then(|value| value.parse().ok()),
            send_owner_preview: env_or("SEND_OWNER_PREVIEW", "true") == "true",
            comment_custom_emoji_id: env_optional("COMMENT_CUSTOM_EMOJI_ID"),
            tech_custom_emoji_id: env_optional("TECH_CUSTOM_EMOJI_ID"),
            amd_custom_emoji_id: env_optional("AMD_CUSTOM_EMOJI_ID"),
            radeon_custom_emoji_id: env_optional("RADEON_CUSTOM_EMOJI_ID"),
            ryzen_custom_emoji_id: env_optional("RYZEN_CUSTOM_EMOJI_ID"),
        }
    }
}

fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

async fn build_pool() -> anyhow::Result<PgPool> {
    let database_url = std::env::var("DATABASE_URL")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    Ok(pool)
}

async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

async fn handle_command(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    cmd: Command,
    pool: PgPool,
    config: Config,
) -> ResponseResult<()> {
    if let Err(err) = save_telegram_message(&pool, &msg).await {
        tracing::error!(%err, "failed to save command message");
    }

    match cmd {
        Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Ping => {
            bot.send_message(msg.chat.id, "pong").await?;
        }
        Command::Db => {
            let row: (i64,) = sqlx::query_as("select 1")
                .fetch_one(&pool)
                .await
                .map_err(|err| {
                    tracing::error!(%err, "database check failed");
                    teloxide::RequestError::Io(std::io::Error::other("database check failed"))
                })?;

            bot.send_message(msg.chat.id, format!("db ok: {}", row.0))
                .await?;
        }
        Command::EmojiIds => {
            send_custom_emoji_ids(&bot, &msg).await?;
        }
        Command::FormatTest(post_text) => {
            if !should_generate_comment(&post_text, &config) {
                bot.send_message(
                    msg.chat.id,
                    "Пропускаю: в посте нет сигнатуры обычного поста, похоже на рекламу или служебный пост.",
                )
                .await?;
                return Ok(());
            }

            let clean_post = clean_post_for_llm(&post_text, &config);
            let text = build_comment_html(&clean_post, &config);
            send_html(&bot, msg.chat.id, text).await?;
        }
    }

    Ok(())
}

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    pool: PgPool,
    config: Config,
) -> ResponseResult<()> {
    if let Err(err) = maybe_comment_post(&bot, &msg, &pool, &config).await {
        tracing::error!(%err, "failed to process message");
    }

    Ok(())
}

async fn maybe_comment_post(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<()> {
    save_telegram_message(pool, msg).await?;

    // The bot should never react to random chat messages. A valid target is only
    // Telegram's automatic channel post copy in the linked discussion chat.
    let Some(candidate) = comment_candidate(msg, config) else {
        return Ok(());
    };

    // Editorial posts carry the VK/MAX footer. Ads usually do not, so the marker
    // doubles as a cheap allowlist and keeps promotional posts out of the chat CTA.
    if !should_generate_comment(candidate.post_text, config) {
        tracing::info!(
            discussion_message_id = msg.id.0,
            "skip post without signature marker"
        );
        return Ok(());
    }

    let clean_post = clean_post_for_llm(candidate.post_text, config);
    let job_id = create_post_comment_job(
        pool,
        config.discussion_chat_id,
        msg.id.0,
        candidate.source_channel_id,
        candidate.source_message_id.0,
        &clean_post,
    )
    .await?;

    let Some(job_id) = job_id else {
        tracing::info!(
            discussion_message_id = msg.id.0,
            "comment job already exists, skip"
        );
        return Ok(());
    };

    // Gemma handles text and vision in one request. If Telegram attached several
    // photo sizes, use the largest one so charts and small text stay readable.
    let image_base64 = download_largest_photo_base64(bot, msg).await?;
    let chat_member_count = get_chat_member_count(bot, config).await;
    let prompt = build_llm_prompt(&clean_post, chat_member_count);
    let llm_body = generate_with_ollama(config, &prompt, image_base64.as_deref()).await?;
    let final_html = build_comment_html(&llm_body, config);

    let sent = send_html_reply(bot, msg.chat.id, msg.id, final_html.clone()).await?;

    sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'sent', bot_comment_message_id = $2, updated_at = now()
        where id = $1
        "#,
    )
    .bind(job_id)
    .bind(sent.id.0)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        insert into llm_generations
            (post_comment_job_id, provider, model, prompt, image_used, response, final_html)
        values ($1, 'ollama', $2, $3, $4, $5, $6)
        "#,
    )
    .bind(job_id)
    .bind(&config.vision_model)
    .bind(&prompt)
    .bind(image_base64.is_some())
    .bind(&llm_body)
    .bind(&final_html)
    .execute(pool)
    .await?;

    if let Some(owner_id) = owner_preview_chat(config) {
        send_owner_preview(bot, owner_id, &final_html, candidate.source_message_id).await;
    }

    Ok(())
}

fn owner_preview_chat(config: &Config) -> Option<i64> {
    config
        .send_owner_preview
        .then_some(config.owner_telegram_id)?
}

async fn send_owner_preview(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    owner_id: i64,
    final_html: &str,
    source_message_id: MessageId,
) {
    let preview = format!(
        "Комментарий отправлен:\n\n{}\n\n<code>source_message_id={}</code>",
        final_html, source_message_id.0
    );

    if let Err(err) = send_html(bot, ChatId(owner_id), preview).await {
        tracing::warn!(%err, "failed to send owner preview");
    }
}

fn comment_candidate<'a>(msg: &'a Message, config: &Config) -> Option<CommentCandidate<'a>> {
    match (
        msg.chat.id.0 == config.discussion_chat_id,
        msg.is_automatic_forward(),
        forwarded_channel_post(msg),
        message_text(msg),
    ) {
        (true, true, Some((source_channel_id, source_message_id)), Some(post_text))
            if source_channel_id == config.source_channel_id =>
        {
            Some(CommentCandidate {
                source_channel_id,
                source_message_id,
                post_text,
            })
        }
        _ => None,
    }
}

async fn send_custom_emoji_ids(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
) -> ResponseResult<()> {
    let ids = custom_emoji_ids(msg);
    if ids.is_empty() {
        send_html(
            bot,
            msg.chat.id,
            "В этом сообщении нет premium/custom emoji entities.",
        )
        .await?;
        return Ok(());
    }

    let lines = ids
        .iter()
        .map(|id| format!("<code>{}</code>", escape_html(id)))
        .collect::<Vec<_>>()
        .join("\n");

    send_html(
        bot,
        msg.chat.id,
        format!("Нашёл custom_emoji_id:\n{}", lines),
    )
    .await?;

    Ok(())
}

fn custom_emoji_ids(msg: &Message) -> Vec<String> {
    msg.entities()
        .into_iter()
        .flatten()
        .chain(msg.caption_entities().into_iter().flatten())
        .filter_map(|entity| match &entity.kind {
            MessageEntityKind::CustomEmoji { custom_emoji_id } => Some(custom_emoji_id.clone()),
            _ => None,
        })
        .collect()
}

fn build_comment_html(llm_body: &str, config: &Config) -> String {
    // The model is instructed to use {CHAT_LINK}; code owns the actual HTML
    // anchor so the URL is stable and link preview can stay disabled.
    let clean_body = normalize_ai_markers(&strip_links(llm_body))
        .trim()
        .to_string();

    if clean_body.is_empty() {
        return String::new();
    }

    let body = render_chat_link_placeholder(&clean_body, config);

    match pick_comment_emoji(llm_body, config) {
        Some(custom_emoji_id) => format!(
            r#"<tg-emoji emoji-id="{}">😎</tg-emoji> {}"#,
            escape_html(custom_emoji_id),
            body
        ),
        None => body,
    }
}

async fn send_html(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    bot.send_message(chat_id, text.into())
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .await
}

async fn send_html_reply(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    reply_to_message_id: MessageId,
    text: impl Into<String>,
) -> ResponseResult<Message> {
    bot.send_message(chat_id, text.into())
        .reply_parameters(ReplyParameters::new(reply_to_message_id).allow_sending_without_reply())
        .link_preview_options(LinkPreviewOptions {
            is_disabled: true,
            url: None,
            prefer_small_media: false,
            prefer_large_media: false,
            show_above_text: false,
        })
        .await
}

fn forwarded_channel_post(msg: &Message) -> Option<(i64, MessageId)> {
    match msg.forward_origin()? {
        MessageOrigin::Channel {
            chat, message_id, ..
        } => Some((chat.id.0, *message_id)),
        _ => None,
    }
}

fn message_text(msg: &Message) -> Option<&str> {
    msg.text().or_else(|| msg.caption())
}

async fn save_telegram_message(pool: &PgPool, msg: &Message) -> anyhow::Result<()> {
    let (source_channel_id, source_message_id) = forwarded_channel_post(msg)
        .map(|(chat_id, message_id)| (Some(chat_id), Some(message_id.0)))
        .unwrap_or((None, None));
    let user_id = msg.from.as_ref().map(|user| user.id.0 as i64);
    // Keep the raw payload while the bot is young: Telegram update shapes vary,
    // and raw_json makes production debugging much faster.
    let raw_json = serde_json::to_value(msg)?;

    sqlx::query(
        r#"
        insert into telegram_messages
            (chat_id, message_id, user_id, source_channel_id, source_message_id, is_automatic_forward, text, raw_json)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        on conflict (chat_id, message_id) do update set
            text = excluded.text,
            raw_json = excluded.raw_json
        "#,
    )
    .bind(msg.chat.id.0)
    .bind(msg.id.0)
    .bind(user_id)
    .bind(source_channel_id)
    .bind(source_message_id)
    .bind(msg.is_automatic_forward())
    .bind(message_text(msg))
    .bind(raw_json)
    .execute(pool)
    .await?;

    Ok(())
}

async fn create_post_comment_job(
    pool: &PgPool,
    discussion_chat_id: i64,
    discussion_message_id: i32,
    source_channel_id: i64,
    source_message_id: i32,
    cleaned_post_text: &str,
) -> anyhow::Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        insert into post_comment_jobs
            (discussion_chat_id, discussion_message_id, source_channel_id, source_message_id, cleaned_post_text)
        values ($1, $2, $3, $4, $5)
        on conflict do nothing
        returning id
        "#,
    )
    .bind(discussion_chat_id)
    .bind(discussion_message_id)
    .bind(source_channel_id)
    .bind(source_message_id)
    .bind(cleaned_post_text)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id,)| id))
}

async fn download_largest_photo_base64(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
) -> anyhow::Result<Option<String>> {
    let Some(photo) = msg
        .photo()
        .and_then(|photos| photos.iter().max_by_key(|photo| photo.width * photo.height))
    else {
        return Ok(None);
    };

    let file = bot.get_file(photo.file.id.clone()).await?;
    let mut bytes = Vec::new();
    bot.download_file(&file.path, &mut bytes).await?;

    Ok(Some(BASE64.encode(bytes)))
}

async fn get_chat_member_count(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> Option<u32> {
    match bot
        .get_chat_member_count(ChatId(config.discussion_chat_id))
        .await
    {
        Ok(count) => Some(count),
        Err(err) => {
            tracing::warn!(%err, "failed to get chat member count");
            None
        }
    }
}

fn build_llm_prompt(post_text: &str, chat_member_count: Option<u32>) -> String {
    let system_prompt = include_str!("../prompts/first_comment.md");
    let chat_context = match chat_member_count {
        Some(count) => format!(
            "В чате сейчас {count} участников. Это реальное число из Telegram API, его можно использовать в приглашении."
        ),
        None => "Число участников чата неизвестно, не называй конкретное количество.".to_string(),
    };

    format!("{system_prompt}\n\nКонтекст чата:\n{chat_context}\n\nПост:\n{post_text}")
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage<'a>>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaMessage<'a> {
    role: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<&'a str>,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaResponseMessage>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

async fn generate_with_ollama(
    config: &Config,
    prompt: &str,
    image_base64: Option<&str>,
) -> anyhow::Result<String> {
    let images = image_base64.into_iter().collect::<Vec<_>>();
    let request = OllamaChatRequest {
        model: &config.vision_model,
        messages: vec![OllamaMessage {
            role: "user",
            content: prompt,
            images,
        }],
        stream: false,
        options: OllamaOptions {
            temperature: 0.55,
            num_predict: 180,
        },
    };

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "{}/api/chat",
            config.ollama_base_url.trim_end_matches('/')
        ))
        .bearer_auth(&config.ollama_api_key)
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json::<OllamaChatResponse>()
        .await?;

    if let Some(error) = response.error {
        anyhow::bail!(error);
    }

    let content = response
        .message
        .map(|message| message.content)
        .unwrap_or_default();

    if content.trim().is_empty() {
        anyhow::bail!("empty Ollama response");
    }

    Ok(content)
}

fn pick_comment_emoji<'a>(text: &str, config: &'a Config) -> Option<&'a str> {
    let lower = text.to_lowercase();
    // Brand emoji are custom stickers from the channel pack. Prefer exact
    // matches over the generic channel logo when the post topic is obvious.
    if lower.contains("radeon") || lower.contains("видеокарт") {
        return config
            .radeon_custom_emoji_id
            .as_deref()
            .or(config.amd_custom_emoji_id.as_deref())
            .or(config.comment_custom_emoji_id.as_deref());
    }

    if lower.contains("ryzen") {
        return config
            .ryzen_custom_emoji_id
            .as_deref()
            .or(config.amd_custom_emoji_id.as_deref())
            .or(config.comment_custom_emoji_id.as_deref());
    }

    if lower.contains("amd") {
        return config
            .amd_custom_emoji_id
            .as_deref()
            .or(config.comment_custom_emoji_id.as_deref());
    }

    let is_tech = lower.contains("amd")
        || lower.contains("windows")
        || lower.contains("драйвер")
        || lower.contains("fps")
        || lower.contains("пк")
        || lower.contains("видеокарт");

    if is_tech {
        config
            .tech_custom_emoji_id
            .as_deref()
            .or(config.comment_custom_emoji_id.as_deref())
    } else {
        config.comment_custom_emoji_id.as_deref()
    }
}

fn normalize_ai_markers(text: &str) -> String {
    text.replace(['—', '–'], "-")
        .replace(['«', '»'], "\"")
        .replace("Вот вариант:", "")
        .replace("Вариант:", "")
        .trim()
        .to_string()
}

fn render_chat_link_placeholder(text: &str, config: &Config) -> String {
    let link = format!(
        r#"<a href="{}">{}</a>"#,
        escape_html(&config.chat_invite_url),
        escape_html(&config.chat_invite_label),
    );

    if text.contains("{CHAT_LINK}") {
        escape_html(text).replace("{CHAT_LINK}", &link)
    } else {
        format!(
            r#"{} <a href="{}">в чате</a>"#,
            escape_html(text),
            escape_html(&config.chat_invite_url)
        )
    }
}

fn should_generate_comment(post_text: &str, config: &Config) -> bool {
    post_text.contains(&config.post_signature_marker)
}

fn clean_post_for_llm(post_text: &str, config: &Config) -> String {
    let without_signature = match post_text.find(&config.post_signature_marker) {
        Some(index) => &post_text[..index],
        None => post_text,
    };

    without_signature.trim().to_string()
}

fn strip_links(text: &str) -> String {
    text.split_whitespace()
        .filter(|word| !word.starts_with("http://") && !word.starts_with("https://"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
