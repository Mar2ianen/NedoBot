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
use features::first_comment::prompt::build_llm_prompt;
use features::first_comment::render::build_comment_html;
use features::first_comment::repo::{
    LlmGenerationInsert, create_post_comment_job, insert_llm_generation, load_recent_bot_comments,
    mark_post_comment_sent,
};
use features::memory::service::{load_relevant_memory_notes, remember_post};
use llm::service::generate_text;
use state::AppState;
use telegram::command_handler::handle_command;
use telegram::commands::Command;
use telegram::render::{send_html, send_html_reply};

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

    mark_post_comment_sent(pool, job_id, sent.id.0).await?;
    insert_llm_generation(
        pool,
        LlmGenerationInsert {
            job_id,
            provider: &generation.provider,
            model: &generation.model,
            prompt: &prompt,
            image_used: generation.image_used,
            response: &generation.content,
            final_html: &final_html,
        },
    )
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
