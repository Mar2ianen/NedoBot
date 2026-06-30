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
    #[command(description = "показать последние заметки памяти")]
    Memory,
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

#[derive(Debug)]
struct MemoryNote {
    title: String,
    summary: String,
    cautions: String,
    keywords: Vec<String>,
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
        Command::Memory => {
            send_memory_notes(&bot, msg.chat.id, &pool).await?;
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
    let memory_notes = load_relevant_memory_notes(pool, &clean_post).await?;
    let prompt = build_llm_prompt(&clean_post, chat_member_count, &memory_notes);
    let llm_body =
        generate_with_ollama(config, &prompt, image_base64.as_deref(), 0.45, 140).await?;
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

    if let Err(err) = remember_post(
        pool,
        config,
        candidate.source_channel_id,
        candidate.source_message_id.0,
        &clean_post,
    )
    .await
    {
        tracing::warn!(%err, "failed to save post memory note");
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

async fn send_memory_notes(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
) -> ResponseResult<()> {
    let notes = sqlx::query_as::<_, (String, String, String)>(
        r#"
        select title, summary, array_to_string(keywords, ', ')
        from post_memory_notes
        order by created_at desc
        limit 5
        "#,
    )
    .fetch_all(pool)
    .await
    .map_err(|err| {
        tracing::error!(%err, "failed to load memory notes");
        teloxide::RequestError::Io(std::io::Error::other("memory check failed"))
    })?;

    if notes.is_empty() {
        bot.send_message(chat_id, "Память пока пустая.").await?;
        return Ok(());
    }

    let text = notes
        .into_iter()
        .map(|(title, summary, keywords)| {
            format!(
                "<b>{}</b>\n{}\n<code>{}</code>",
                escape_html(&title),
                escape_html(&summary),
                escape_html(&keywords)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    send_html(bot, chat_id, text).await?;

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

fn build_llm_prompt(
    post_text: &str,
    chat_member_count: Option<u32>,
    memory_notes: &[MemoryNote],
) -> String {
    let system_prompt = include_str!("../prompts/first_comment.md");
    let tech_rag = include_str!("../prompts/tech_rag.md");
    let chat_context = match chat_member_count {
        Some(count) => format!(
            "В чате сейчас {count} участников. Это реальное число из Telegram API, но используй его редко."
        ),
        None => "Число участников чата неизвестно, не называй конкретное количество.".to_string(),
    };
    let memory_context = render_memory_context(memory_notes);

    format!(
        "{system_prompt}\n\nRAG для факт-чека, не пересказывать:\n{tech_rag}\n\nПамять прошлых новостей, использовать только если релевантно:\n{memory_context}\n\nКонтекст чата:\n{chat_context}\n\nПост:\n{post_text}"
    )
}

fn render_memory_context(memory_notes: &[MemoryNote]) -> String {
    if memory_notes.is_empty() {
        return "Нет релевантных заметок.".to_string();
    }

    memory_notes
        .iter()
        .take(5)
        .map(|note| {
            format!(
                "- {}: {}{}",
                note.title,
                note.summary,
                if note.cautions.trim().is_empty() {
                    String::new()
                } else {
                    format!(" Осторожно: {}", note.cautions)
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    temperature: f32,
    num_predict: u32,
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
            temperature,
            num_predict,
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

async fn load_relevant_memory_notes(
    pool: &PgPool,
    post_text: &str,
) -> anyhow::Result<Vec<MemoryNote>> {
    let post_keywords = extract_keywords(post_text);
    if post_keywords.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as::<_, (i64, String, String, String, Vec<String>)>(
        r#"
        select id, title, summary, cautions, keywords
        from post_memory_notes
        order by created_at desc
        limit 80
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut scored = rows
        .into_iter()
        .filter_map(|(_id, title, summary, cautions, keywords)| {
            let score = keywords
                .iter()
                .filter(|keyword| post_keywords.contains(keyword))
                .count();

            (score > 0).then_some((
                score,
                MemoryNote {
                    title,
                    summary,
                    cautions,
                    keywords,
                },
            ))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|(left_score, _), (right_score, _)| right_score.cmp(left_score));

    Ok(scored.into_iter().take(5).map(|(_, note)| note).collect())
}

async fn remember_post(
    pool: &PgPool,
    config: &Config,
    source_channel_id: i64,
    source_message_id: i32,
    post_text: &str,
) -> anyhow::Result<()> {
    let note_prompt = build_memory_note_prompt(post_text);
    let raw_note = generate_with_ollama(config, &note_prompt, None, 0.2, 220).await?;
    let mut note = parse_memory_note(&raw_note, post_text);
    note.keywords = merge_keywords(note.keywords, extract_keywords(post_text));

    if let Some(existing) = find_merge_candidate(pool, &note.keywords).await? {
        let merged = merge_memory_notes(existing, note);
        sqlx::query(
            r#"
            update post_memory_notes
            set title = $2,
                summary = $3,
                cautions = $4,
                keywords = $5,
                raw_note = concat(raw_note, E'\n\n--- merged note ---\n', $6),
                merged_source_posts = merged_source_posts + 1,
                last_source_channel_id = $7,
                last_source_message_id = $8,
                updated_at = now()
            where id = $1
            "#,
        )
        .bind(merged.id)
        .bind(&merged.note.title)
        .bind(&merged.note.summary)
        .bind(&merged.note.cautions)
        .bind(&merged.note.keywords)
        .bind(&raw_note)
        .bind(source_channel_id)
        .bind(source_message_id)
        .execute(pool)
        .await?;

        return Ok(());
    }

    sqlx::query(
        r#"
        insert into post_memory_notes
            (source_channel_id, source_message_id, title, summary, cautions, keywords, raw_note, last_source_channel_id, last_source_message_id)
        values ($1, $2, $3, $4, $5, $6, $7, $1, $2)
        on conflict (source_channel_id, source_message_id) do update set
            title = excluded.title,
            summary = excluded.summary,
            cautions = excluded.cautions,
            keywords = excluded.keywords,
            raw_note = excluded.raw_note,
            updated_at = now()
        "#,
    )
    .bind(source_channel_id)
    .bind(source_message_id)
    .bind(&note.title)
    .bind(&note.summary)
    .bind(&note.cautions)
    .bind(&note.keywords)
    .bind(&raw_note)
    .execute(pool)
    .await?;

    Ok(())
}

struct MergeCandidate {
    id: i64,
    note: MemoryNote,
    score: usize,
}

async fn find_merge_candidate(
    pool: &PgPool,
    new_keywords: &[String],
) -> anyhow::Result<Option<MergeCandidate>> {
    if new_keywords.is_empty() {
        return Ok(None);
    }

    let rows = sqlx::query_as::<_, (i64, String, String, String, Vec<String>)>(
        r#"
        select id, title, summary, cautions, keywords
        from post_memory_notes
        where keywords && $1
        order by updated_at desc
        limit 30
        "#,
    )
    .bind(new_keywords)
    .fetch_all(pool)
    .await?;

    let mut candidates = rows
        .into_iter()
        .filter_map(|(id, title, summary, cautions, keywords)| {
            let score = keywords
                .iter()
                .filter(|keyword| new_keywords.contains(keyword))
                .count();

            (score >= 2).then_some(MergeCandidate {
                id,
                note: MemoryNote {
                    title,
                    summary,
                    cautions,
                    keywords,
                },
                score,
            })
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| right.score.cmp(&left.score));

    Ok(candidates.into_iter().next())
}

fn merge_memory_notes(existing: MergeCandidate, new_note: MemoryNote) -> MergeCandidate {
    let mut merged_note = MemoryNote {
        title: choose_memory_title(&existing.note.title, &new_note.title),
        summary: merge_text_lines(&existing.note.summary, &new_note.summary, 420),
        cautions: merge_text_lines(&existing.note.cautions, &new_note.cautions, 260),
        keywords: merge_keywords(existing.note.keywords, new_note.keywords),
    };

    if merged_note.cautions.trim().is_empty() {
        merged_note.cautions = "Не делать выводы шире фактов из поста.".to_string();
    }

    MergeCandidate {
        id: existing.id,
        note: merged_note,
        score: existing.score,
    }
}

fn choose_memory_title(existing: &str, new_title: &str) -> String {
    if existing.chars().count() <= 80 {
        existing.to_string()
    } else {
        first_text_chars(new_title, 80)
    }
}

fn merge_text_lines(existing: &str, new_text: &str, limit: usize) -> String {
    let mut parts = Vec::new();
    for part in [existing, new_text]
        .into_iter()
        .flat_map(|text| text.split(['\n', ';']))
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if !parts.iter().any(|saved: &String| saved == part) {
            parts.push(part.to_string());
        }
    }

    first_text_chars(&parts.join("; "), limit)
}

fn build_memory_note_prompt(post_text: &str) -> String {
    format!(
        r#"Сделай короткую заметку памяти для будущих комментариев под техно-новостями.
Не добавляй факты, которых нет в посте. Не пересказывай рекламный хвост. Не пиши стиль комментария.

Формат строго такой:
TITLE: короткая тема до 80 символов
KEYWORDS: 5-10 ключей через запятую, нижний регистр
SUMMARY: 1-2 коротких факта из поста
CAUTIONS: что нельзя утверждать без данных, одной фразой

Пост:
{post_text}"#
    )
}

fn parse_memory_note(raw_note: &str, post_text: &str) -> MemoryNote {
    let title = field_value(raw_note, "TITLE").unwrap_or_else(|| fallback_title(post_text));
    let keywords = field_value(raw_note, "KEYWORDS")
        .map(|value| {
            value
                .split(',')
                .map(normalize_keyword)
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let summary =
        field_value(raw_note, "SUMMARY").unwrap_or_else(|| first_text_chars(post_text, 220));
    let cautions = field_value(raw_note, "CAUTIONS").unwrap_or_default();

    MemoryNote {
        title,
        summary,
        cautions,
        keywords,
    }
}

fn field_value(raw_note: &str, field: &str) -> Option<String> {
    raw_note.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(field)
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn fallback_title(post_text: &str) -> String {
    post_text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| first_text_chars(line, 80))
        .unwrap_or_else(|| "Без темы".to_string())
}

fn first_text_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }

    trimmed.chars().take(limit).collect::<String>()
}

fn merge_keywords(mut left: Vec<String>, right: Vec<String>) -> Vec<String> {
    for keyword in right {
        if !left.contains(&keyword) {
            left.push(keyword);
        }
    }

    left.truncate(16);
    left
}

fn extract_keywords(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut keywords = Vec::new();

    for phrase in [
        "switch 2",
        "playstation 5 pro",
        "ps5 pro",
        "xbox series",
        "gta 6",
        "rtx 50",
        "radeon",
        "rx 9000",
        "rx 9070",
        "ryzen",
        "windows 10",
        "windows 11",
        "smart access memory",
        "sam",
        "amd",
        "nvidia",
        "intel",
        "apple",
        "microsoft",
        "xbox",
        "playstation",
        "nintendo",
        "драйвер",
        "fps",
        "предзаказ",
        "цена",
        "память",
        "видеокарта",
    ] {
        if lower.contains(phrase) {
            keywords.push(phrase.to_string());
        }
    }

    for token in lower
        .split(|ch: char| !ch.is_alphanumeric())
        .map(normalize_keyword)
        .filter(|token| token.chars().count() >= 4)
    {
        if !is_stop_keyword(&token) && !keywords.contains(&token) {
            keywords.push(token);
        }
    }

    keywords.truncate(24);
    keywords
}

fn normalize_keyword(value: impl AsRef<str>) -> String {
    value
        .as_ref()
        .trim()
        .trim_matches(|ch: char| !ch.is_alphanumeric())
        .to_lowercase()
}

fn is_stop_keyword(token: &str) -> bool {
    matches!(
        token,
        "это"
            | "что"
            | "как"
            | "для"
            | "или"
            | "еще"
            | "уже"
            | "если"
            | "также"
            | "которые"
            | "после"
            | "сейчас"
            | "будет"
            | "стало"
            | "стали"
            | "может"
            | "около"
            | "ранее"
            | "не"
            | "на"
            | "по"
            | "из"
            | "под"
            | "над"
            | "без"
            | "при"
            | "все"
            | "the"
            | "and"
            | "with"
            | "from"
    )
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
