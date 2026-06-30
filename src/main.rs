use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use sqlx::PgPool;
use sqlx::types::chrono::{DateTime, Utc};
use teloxide::{
    dispatching::UpdateFilterExt,
    net::Download,
    prelude::*,
    requests::RequesterExt,
    types::{
        ChatMemberKind, ChatMemberUpdated, MessageId, MessageReactionCountUpdated,
        MessageReactionUpdated, ParseMode, User,
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
use db::{build_pool, migrate};
use features::first_comment::candidate::comment_candidate;
use features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use features::first_comment::render::build_comment_html;
use features::memory::service::{MemoryNote, load_relevant_memory_notes, remember_post};
use llm::service::generate_text;
use state::AppState;
use telegram::command_handler::handle_command;
use telegram::commands::Command;
use telegram::entities::{forwarded_channel_post, message_has_links, message_text};
use telegram::render::{send_html, send_html_reply};
use text::{normalize_ai_markers, strip_links};

struct MemberSnapshot {
    chat_id: i64,
    user_id: i64,
    status: String,
    is_admin: bool,
    is_present: bool,
    raw_json: serde_json::Value,
    observed_at: DateTime<Utc>,
}

struct ChatUserMemberEvent<'a> {
    chat_id: i64,
    user_id: i64,
    old_status: &'a str,
    new_status: &'a str,
    invite_link: Option<&'a str>,
    via_chat_folder_invite_link: bool,
    event_at: DateTime<Utc>,
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

pub(crate) async fn save_telegram_message(pool: &PgPool, msg: &Message) -> anyhow::Result<()> {
    let (source_channel_id, source_message_id) = forwarded_channel_post(msg)
        .map(|(chat_id, message_id)| (Some(chat_id), Some(message_id.0)))
        .unwrap_or((None, None));
    let user_id = msg.from.as_ref().map(|user| user.id.0 as i64);
    if let Some(user) = msg.from.as_ref() {
        upsert_user_profile(pool, user).await?;
    }

    if let Some(reply_user) = msg.reply_to_message().and_then(|reply| reply.from.as_ref()) {
        upsert_user_profile(pool, reply_user).await?;
    }

    let reply_to_message_id = msg.reply_to_message().map(|reply| reply.id.0);
    let reply_to_user_id = msg
        .reply_to_message()
        .and_then(|reply| reply.from.as_ref())
        .map(|user| user.id.0 as i64);
    let sender_chat_id = msg.sender_chat.as_ref().map(|chat| chat.id.0);
    let via_bot_id = msg.via_bot.as_ref().map(|bot| bot.id.0 as i64);
    // Keep the raw payload while the bot is young: Telegram update shapes vary,
    // and raw_json makes production debugging much faster.
    let raw_json = serde_json::to_value(msg)?;

    let (inserted,): (bool,) = sqlx::query_as(
        r#"
        insert into telegram_messages
            (
                chat_id, message_id, user_id, source_channel_id, source_message_id,
                is_automatic_forward, text, raw_json, reply_to_message_id,
                reply_to_user_id, sender_chat_id, via_bot_id, has_photo, has_video,
                has_document, has_audio, has_voice, has_sticker, has_animation,
                has_links
            )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
        on conflict (chat_id, message_id) do update set
            text = excluded.text,
            raw_json = excluded.raw_json,
            reply_to_message_id = excluded.reply_to_message_id,
            reply_to_user_id = excluded.reply_to_user_id,
            sender_chat_id = excluded.sender_chat_id,
            via_bot_id = excluded.via_bot_id,
            has_photo = excluded.has_photo,
            has_video = excluded.has_video,
            has_document = excluded.has_document,
            has_audio = excluded.has_audio,
            has_voice = excluded.has_voice,
            has_sticker = excluded.has_sticker,
            has_animation = excluded.has_animation,
            has_links = excluded.has_links
        returning (xmax = 0) as inserted
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
    .bind(reply_to_message_id)
    .bind(reply_to_user_id)
    .bind(sender_chat_id)
    .bind(via_bot_id)
    .bind(msg.photo().is_some())
    .bind(msg.video().is_some())
    .bind(msg.document().is_some())
    .bind(msg.audio().is_some())
    .bind(msg.voice().is_some())
    .bind(msg.sticker().is_some())
    .bind(msg.animation().is_some())
    .bind(message_has_links(msg))
    .fetch_one(pool)
    .await?;

    if inserted {
        upsert_chat_user_activity(pool, msg, source_channel_id).await?;
    }

    Ok(())
}

async fn upsert_chat_user_activity(
    pool: &PgPool,
    msg: &Message,
    source_channel_id: Option<i64>,
) -> anyhow::Result<()> {
    if source_channel_id.is_some() {
        return Ok(());
    }

    let Some(user_id) = msg.from.as_ref().map(|user| user.id.0 as i64) else {
        return Ok(());
    };

    let reply_to_message_id = msg.reply_to_message().map(|reply| reply.id.0);
    let has_media = msg.photo().is_some()
        || msg.video().is_some()
        || msg.document().is_some()
        || msg.audio().is_some()
        || msg.voice().is_some()
        || msg.sticker().is_some()
        || msg.animation().is_some();

    sqlx::query(
        r#"
        with flags as (
            select
                exists (
                    select 1
                    from post_comment_jobs
                    where discussion_chat_id = $1
                      and discussion_message_id = $5
                ) as reply_to_channel_post,
                exists (
                    select 1
                    from post_comment_jobs
                    where discussion_chat_id = $1
                      and bot_comment_message_id = $5
                ) as reply_to_bot
        )
        insert into telegram_chat_users
            (
                chat_id, telegram_user_id, first_seen_at, last_seen_at,
                first_message_id, last_message_id, message_count, reply_count,
                link_count, media_count, reply_to_channel_post_count,
                reply_to_bot_count, updated_at
            )
        select
            $1,
            $2,
            $3,
            $3,
            $4,
            $4,
            1,
            case when $5 is null then 0 else 1 end,
            case when $6 then 1 else 0 end,
            case when $7 then 1 else 0 end,
            case when flags.reply_to_channel_post then 1 else 0 end,
            case when flags.reply_to_bot then 1 else 0 end,
            now()
        from flags
        on conflict (chat_id, telegram_user_id) do update set
            first_seen_at = case
                when telegram_chat_users.first_seen_at is null
                  or excluded.first_seen_at < telegram_chat_users.first_seen_at
                then excluded.first_seen_at
                else telegram_chat_users.first_seen_at
            end,
            last_seen_at = case
                when telegram_chat_users.last_seen_at is null
                  or excluded.last_seen_at > telegram_chat_users.last_seen_at
                then excluded.last_seen_at
                else telegram_chat_users.last_seen_at
            end,
            first_message_id = case
                when telegram_chat_users.first_seen_at is null
                  or excluded.first_seen_at < telegram_chat_users.first_seen_at
                then excluded.first_message_id
                else telegram_chat_users.first_message_id
            end,
            last_message_id = case
                when telegram_chat_users.last_seen_at is null
                  or excluded.last_seen_at > telegram_chat_users.last_seen_at
                then excluded.last_message_id
                else telegram_chat_users.last_message_id
            end,
            message_count = telegram_chat_users.message_count + excluded.message_count,
            reply_count = telegram_chat_users.reply_count + excluded.reply_count,
            link_count = telegram_chat_users.link_count + excluded.link_count,
            media_count = telegram_chat_users.media_count + excluded.media_count,
            reply_to_channel_post_count = telegram_chat_users.reply_to_channel_post_count + excluded.reply_to_channel_post_count,
            reply_to_bot_count = telegram_chat_users.reply_to_bot_count + excluded.reply_to_bot_count,
            updated_at = now()
        "#,
    )
    .bind(msg.chat.id.0)
    .bind(user_id)
    .bind(msg.date)
    .bind(msg.id.0)
    .bind(reply_to_message_id)
    .bind(message_has_links(msg))
    .bind(has_media)
    .execute(pool)
    .await?;

    Ok(())
}

async fn upsert_user_profile(pool: &PgPool, user: &User) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into telegram_user_profiles
            (telegram_user_id, username, first_name, last_name, is_bot, is_premium, language_code)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict (telegram_user_id) do update set
            username = excluded.username,
            first_name = excluded.first_name,
            last_name = excluded.last_name,
            is_bot = excluded.is_bot,
            is_premium = excluded.is_premium,
            language_code = excluded.language_code,
            last_seen_at = now(),
            updated_at = now()
        "#,
    )
    .bind(user.id.0 as i64)
    .bind(&user.username)
    .bind(&user.first_name)
    .bind(&user.last_name)
    .bind(user.is_bot)
    .bind(user.is_premium)
    .bind(&user.language_code)
    .execute(pool)
    .await?;

    Ok(())
}

async fn save_message_reaction(
    pool: &PgPool,
    reaction: &MessageReactionUpdated,
) -> anyhow::Result<()> {
    // Telegram only sends these updates when bot permissions and allowed
    // updates line up; old reactions cannot be backfilled through Bot API.
    if let Some(user) = reaction.user.as_ref() {
        upsert_user_profile(pool, user).await?;
    }

    let raw_json = serde_json::to_value(reaction)?;
    let old_reactions = serde_json::to_value(&reaction.old_reaction)?;
    let new_reactions = serde_json::to_value(&reaction.new_reaction)?;

    sqlx::query(
        r#"
        insert into telegram_message_reactions
            (chat_id, message_id, user_id, actor_chat_id, old_reactions, new_reactions, raw_json, event_at)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(reaction.chat.id.0)
    .bind(reaction.message_id.0)
    .bind(reaction.user.as_ref().map(|user| user.id.0 as i64))
    .bind(reaction.actor_chat.as_ref().map(|chat| chat.id.0))
    .bind(old_reactions)
    .bind(new_reactions)
    .bind(raw_json)
    .bind(reaction.date)
    .execute(pool)
    .await?;

    Ok(())
}

async fn save_message_reaction_count(
    pool: &PgPool,
    reaction_count: &MessageReactionCountUpdated,
) -> anyhow::Result<()> {
    let raw_json = serde_json::to_value(reaction_count)?;
    let reactions = serde_json::to_value(&reaction_count.reactions)?;
    let total_count = reaction_count
        .reactions
        .iter()
        .map(|reaction| reaction.total_count as i64)
        .sum::<i64>();

    sqlx::query(
        r#"
        insert into telegram_message_reaction_counts
            (chat_id, message_id, reactions, total_count, raw_json, event_at)
        values ($1, $2, $3, $4, $5, $6)
        on conflict (chat_id, message_id) do update set
            reactions = excluded.reactions,
            total_count = excluded.total_count,
            raw_json = excluded.raw_json,
            event_at = excluded.event_at,
            updated_at = now()
        "#,
    )
    .bind(reaction_count.chat.id.0)
    .bind(reaction_count.message_id.0)
    .bind(reactions)
    .bind(total_count as i32)
    .bind(raw_json)
    .bind(reaction_count.date)
    .execute(pool)
    .await?;

    Ok(())
}

async fn save_chat_member_event(pool: &PgPool, member: &ChatMemberUpdated) -> anyhow::Result<()> {
    upsert_user_profile(pool, &member.from).await?;
    upsert_user_profile(pool, &member.new_chat_member.user).await?;

    let raw_json = serde_json::to_value(member)?;
    let old_status = chat_member_status(&member.old_chat_member.kind);
    let new_status = chat_member_status(&member.new_chat_member.kind);
    let is_admin = member.new_chat_member.kind.is_privileged();
    let is_present = member.new_chat_member.kind.is_present();

    sqlx::query(
        r#"
        insert into telegram_chat_member_events
            (
                chat_id, telegram_user_id, actor_user_id, old_status, new_status,
                invite_link, via_chat_folder_invite_link, raw_json, event_at
            )
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(member.chat.id.0)
    .bind(member.new_chat_member.user.id.0 as i64)
    .bind(member.from.id.0 as i64)
    .bind(&old_status)
    .bind(&new_status)
    .bind(
        member
            .invite_link
            .as_ref()
            .map(|link| link.invite_link.clone()),
    )
    .bind(member.via_chat_folder_invite_link)
    .bind(raw_json.clone())
    .bind(member.date)
    .execute(pool)
    .await?;

    upsert_member_snapshot(
        pool,
        MemberSnapshot {
            chat_id: member.chat.id.0,
            user_id: member.new_chat_member.user.id.0 as i64,
            status: new_status.clone(),
            is_admin,
            is_present,
            raw_json,
            observed_at: member.date,
        },
    )
    .await?;

    update_chat_user_member_event(
        pool,
        ChatUserMemberEvent {
            chat_id: member.chat.id.0,
            user_id: member.new_chat_member.user.id.0 as i64,
            old_status: &old_status,
            new_status: &new_status,
            invite_link: member
                .invite_link
                .as_ref()
                .map(|link| link.invite_link.as_str()),
            via_chat_folder_invite_link: member.via_chat_folder_invite_link,
            event_at: member.date,
        },
    )
    .await?;

    Ok(())
}

async fn refresh_known_member_snapshots(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<()> {
    // chat_member updates are sparse without admin rights, so startup refresh
    // improves reports for already-seen users without blocking the bot.
    let users = sqlx::query_as::<_, (i64,)>(
        r#"
        select distinct user_id
        from telegram_messages
        where chat_id = $1 and user_id is not null
        order by user_id
        limit 250
        "#,
    )
    .bind(config.discussion_chat_id)
    .fetch_all(pool)
    .await?;

    for (user_id,) in users {
        match bot
            .get_chat_member(ChatId(config.discussion_chat_id), UserId(user_id as u64))
            .await
        {
            Ok(member) => {
                upsert_user_profile(pool, &member.user).await?;
                let raw_json = serde_json::to_value(&member)?;
                upsert_member_snapshot(
                    pool,
                    MemberSnapshot {
                        chat_id: config.discussion_chat_id,
                        user_id,
                        status: chat_member_status(&member.kind),
                        is_admin: member.kind.is_privileged(),
                        is_present: member.kind.is_present(),
                        raw_json,
                        observed_at: Utc::now(),
                    },
                )
                .await?;
            }
            Err(err) => {
                tracing::debug!(%err, user_id, "failed to refresh chat member");
            }
        }
    }

    Ok(())
}

async fn upsert_member_snapshot(pool: &PgPool, snapshot: MemberSnapshot) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into telegram_chat_member_snapshots
            (chat_id, telegram_user_id, status, is_admin, is_present, raw_json, observed_at)
        values ($1, $2, $3, $4, $5, $6, $7)
        on conflict (chat_id, telegram_user_id) do update set
            status = excluded.status,
            is_admin = excluded.is_admin,
            is_present = excluded.is_present,
            raw_json = excluded.raw_json,
            observed_at = excluded.observed_at
        "#,
    )
    .bind(snapshot.chat_id)
    .bind(snapshot.user_id)
    .bind(&snapshot.status)
    .bind(snapshot.is_admin)
    .bind(snapshot.is_present)
    .bind(snapshot.raw_json)
    .bind(snapshot.observed_at)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        insert into telegram_chat_users
            (
                chat_id, telegram_user_id, member_status, is_admin,
                is_present, member_observed_at, updated_at
            )
        values ($1, $2, $3, $4, $5, $6, now())
        on conflict (chat_id, telegram_user_id) do update set
            member_status = excluded.member_status,
            is_admin = excluded.is_admin,
            is_present = excluded.is_present,
            member_observed_at = excluded.member_observed_at,
            updated_at = now()
        "#,
    )
    .bind(snapshot.chat_id)
    .bind(snapshot.user_id)
    .bind(&snapshot.status)
    .bind(snapshot.is_admin)
    .bind(snapshot.is_present)
    .bind(snapshot.observed_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn update_chat_user_member_event(
    pool: &PgPool,
    event: ChatUserMemberEvent<'_>,
) -> anyhow::Result<()> {
    let was_present = !matches!(event.old_status, "left" | "banned");
    let is_present = !matches!(event.new_status, "left" | "banned");
    let is_join = !was_present && is_present;
    let is_leave = was_present && !is_present;

    sqlx::query(
        r#"
        insert into telegram_chat_users
            (
                chat_id, telegram_user_id, first_joined_at, last_joined_at,
                last_left_at, last_invite_link, via_chat_folder_invite_link,
                updated_at
            )
        values (
            $1,
            $2,
            case when $3 then $4 else null end,
            case when $3 then $4 else null end,
            case when $7 then $4 else null end,
            $5,
            $6,
            now()
        )
        on conflict (chat_id, telegram_user_id) do update set
            first_joined_at = case
                when $3 and telegram_chat_users.first_joined_at is null then $4
                else telegram_chat_users.first_joined_at
            end,
            last_joined_at = case
                when $3 then $4
                else telegram_chat_users.last_joined_at
            end,
            last_left_at = case
                when $7 then $4
                else telegram_chat_users.last_left_at
            end,
            last_invite_link = coalesce(excluded.last_invite_link, telegram_chat_users.last_invite_link),
            via_chat_folder_invite_link = excluded.via_chat_folder_invite_link,
            updated_at = now()
        "#,
    )
    .bind(event.chat_id)
    .bind(event.user_id)
    .bind(is_join)
    .bind(event.event_at)
    .bind(event.invite_link)
    .bind(event.via_chat_folder_invite_link)
    .bind(is_leave)
    .execute(pool)
    .await?;

    Ok(())
}

fn chat_member_status(kind: &ChatMemberKind) -> String {
    format!("{:?}", kind.status()).to_lowercase()
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
