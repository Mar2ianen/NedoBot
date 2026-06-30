use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sqlx::PgPool;
use teloxide::{
    dispatching::UpdateFilterExt,
    net::Download,
    prelude::*,
    requests::RequesterExt,
    types::{
        ChatMemberUpdated, MessageId, MessageReactionCountUpdated, MessageReactionUpdated,
        ParseMode,
    },
};

mod config;
mod db;
mod features;
mod llm;
mod state;
mod telegram;
mod text;

use config::Config;
use db::telegram::{
    refresh_known_member_snapshots, save_chat_member_event, save_message_reaction,
    save_message_reaction_count, save_telegram_message,
};
use db::{build_pool, migrate};
use features::first_comment::candidate::comment_candidate;
use features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use features::first_comment::render::build_comment_html;
use features::memory::service::{MemoryNote, load_relevant_memory_notes, remember_post};
use llm::service::generate_text;
use state::AppState;
use telegram::command_handler::handle_command;
use telegram::commands::Command;
use telegram::render::{send_html, send_html_reply};
use text::{normalize_ai_markers, strip_links};

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
    if let Err(err) = refresh_known_member_snapshots(&bot, &pool, &config).await {
        tracing::warn!(%err, "failed to refresh member snapshots");
    }
    let state = AppState::new(pool, config);

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                .branch(dptree::endpoint(handle_message)),
        )
        .branch(Update::filter_message_reaction_updated().endpoint(handle_message_reaction))
        .branch(
            Update::filter_message_reaction_count_updated().endpoint(handle_message_reaction_count),
        )
        .branch(Update::filter_chat_member().endpoint(handle_chat_member));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
) -> ResponseResult<()> {
    if let Err(err) = maybe_comment_post(&bot, &msg, &state.pool, &state.config).await {
        tracing::error!(%err, "failed to process message");
    }

    Ok(())
}

async fn handle_message_reaction(
    reaction: MessageReactionUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if let Err(err) = save_message_reaction(&state.pool, &reaction).await {
        tracing::error!(%err, "failed to save message reaction");
    }

    Ok(())
}

async fn handle_message_reaction_count(
    reaction_count: MessageReactionCountUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if let Err(err) = save_message_reaction_count(&state.pool, &reaction_count).await {
        tracing::error!(%err, "failed to save message reaction count");
    }

    Ok(())
}

async fn handle_chat_member(member: ChatMemberUpdated, state: AppState) -> ResponseResult<()> {
    if let Err(err) = save_chat_member_event(&state.pool, &member).await {
        tracing::error!(%err, "failed to save chat member event");
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

    let image_base64 = match download_largest_photo_base64(bot, msg).await {
        Ok(image) => image,
        Err(err) => {
            tracing::warn!(%err, "failed to download post image, continue text-only");
            None
        }
    };
    let chat_member_count = get_chat_member_count(bot, config).await;
    let memory_notes = load_relevant_memory_notes(pool, &clean_post).await?;
    let recent_comments = load_recent_bot_comments(pool).await?;
    let prompt = build_llm_prompt(
        &clean_post,
        chat_member_count,
        &memory_notes,
        &recent_comments,
    );
    let generation = generate_text(
        config,
        &prompt,
        image_base64.as_deref(),
        config.llm_temperature,
        config.llm_max_tokens,
    )
    .await?;
    let final_html = build_comment_html(&generation.content, config);

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
        values ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(job_id)
    .bind(&generation.provider)
    .bind(&generation.model)
    .bind(&prompt)
    .bind(generation.image_used)
    .bind(&generation.content)
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
    recent_comments: &[String],
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
    let recent_context = render_recent_comment_context(recent_comments);

    format!(
        "{system_prompt}\n\nRAG для факт-чека, не пересказывать:\n{tech_rag}\n\nПамять прошлых новостей, использовать только если релевантно:\n{memory_context}\n\nПоследние комментарии бота, не повторять стиль и CTA:\n{recent_context}\n\nКонтекст чата:\n{chat_context}\n\nПост:\n{post_text}"
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

fn render_recent_comment_context(recent_comments: &[String]) -> String {
    if recent_comments.is_empty() {
        return "Нет последних комментариев.".to_string();
    }

    recent_comments
        .iter()
        .take(6)
        .map(|comment| format!("- {}", strip_html_tags(comment)))
        .collect::<Vec<_>>()
        .join("\n")
}

async fn load_recent_bot_comments(pool: &PgPool) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        select coalesce(response, final_html)
        from llm_generations
        where coalesce(response, final_html) is not null
        order by created_at desc
        limit 6
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(text,)| normalize_ai_markers(&strip_links(&text)))
        .filter(|text| !text.trim().is_empty())
        .collect())
}

fn strip_html_tags(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;

    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(ch),
            _ => {}
        }
    }

    result
}
