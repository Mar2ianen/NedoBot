use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{DateTime, Utc};
use sqlx::{PgPool, postgres::PgPoolOptions};
use teloxide::{
    dispatching::UpdateFilterExt,
    net::Download,
    prelude::*,
    requests::RequesterExt,
    types::{
        ChatMemberKind, ChatMemberUpdated, LinkPreviewOptions, MessageEntityKind, MessageId,
        MessageOrigin, MessageReactionCountUpdated, MessageReactionUpdated, ParseMode,
        ReplyParameters, User,
    },
    utils::command::BotCommands,
};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case")]
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
    #[command(description = "статистика за текущий день с 05:00 МСК")]
    StatsDay,
    #[command(description = "статистика за текущую неделю с понедельника 05:00 МСК")]
    StatsWeek,
    #[command(description = "статистика за текущий месяц с 1 числа 05:00 МСК")]
    StatsMonth,
    #[command(
        rename = "userstats",
        description = "статистика пользователя: /userstats <id|@username>"
    )]
    UserStats(String),
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

#[derive(Clone, Copy)]
enum StatsPeriod {
    Day,
    Week,
    Month,
}

type ChatStatsSummary = (
    String,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
    i64,
);

struct MemberSnapshot {
    chat_id: i64,
    user_id: i64,
    status: String,
    is_admin: bool,
    is_present: bool,
    raw_json: serde_json::Value,
    observed_at: DateTime<Utc>,
}

struct UserPresentation {
    user_id: i64,
    display_name: String,
    is_bot: bool,
    status: Option<String>,
    is_admin: bool,
    is_present: Option<bool>,
}

impl UserPresentation {
    fn linked_name(&self) -> String {
        let visible = if self.display_name.trim().is_empty() {
            "пользователь"
        } else {
            self.display_name.trim()
        };

        format!(
            r#"<a href="tg://user?id={}">{}</a>"#,
            self.user_id,
            escape_html(visible)
        )
    }

    fn badges(&self) -> String {
        let mut parts = Vec::new();

        if self.is_bot {
            parts.push("бот");
        }

        if self.is_admin {
            parts.push("админ");
        } else if let Some(status) = self.status.as_deref() {
            parts.push(human_member_status(status));
        } else if self.is_present == Some(true) {
            parts.push("в чате");
        } else if self.is_present == Some(false) {
            parts.push("не в чате");
        }

        if parts.is_empty() {
            "статус неизвестен".to_string()
        } else {
            parts.join(", ")
        }
    }

    fn linked_with_badges(&self) -> String {
        format!("{} ({})", self.linked_name(), self.badges())
    }
}

fn display_name(
    username: Option<&str>,
    first_name: Option<&str>,
    last_name: Option<&str>,
    fallback_user_id: i64,
) -> String {
    if let Some(username) = username.filter(|value| !value.trim().is_empty()) {
        return format!("@{username}");
    }

    let full_name = format!(
        "{} {}",
        first_name.unwrap_or_default(),
        last_name.unwrap_or_default()
    )
    .trim()
    .to_string();

    if full_name.is_empty() {
        fallback_user_id.to_string()
    } else {
        full_name
    }
}

impl StatsPeriod {
    fn title(self) -> &'static str {
        match self {
            Self::Day => "день",
            Self::Week => "неделю",
            Self::Month => "месяц",
        }
    }

    fn start_sql(self) -> &'static str {
        match self {
            Self::Day => {
                "(date_trunc('day', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
            }
            Self::Week => {
                "(date_trunc('week', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
            }
            Self::Month => {
                "(date_trunc('month', now() at time zone 'Europe/Moscow' - interval '5 hours') + interval '5 hours') at time zone 'Europe/Moscow'"
            }
        }
    }
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
        Command::StatsDay => {
            send_chat_stats(&bot, msg.chat.id, &pool, &config, StatsPeriod::Day).await?;
        }
        Command::StatsWeek => {
            send_chat_stats(&bot, msg.chat.id, &pool, &config, StatsPeriod::Week).await?;
        }
        Command::StatsMonth => {
            send_chat_stats(&bot, msg.chat.id, &pool, &config, StatsPeriod::Month).await?;
        }
        Command::UserStats(target) => {
            send_user_stats(&bot, msg.chat.id, &pool, &config, &target).await?;
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

async fn handle_message_reaction(
    reaction: MessageReactionUpdated,
    pool: PgPool,
) -> ResponseResult<()> {
    if let Err(err) = save_message_reaction(&pool, &reaction).await {
        tracing::error!(%err, "failed to save message reaction");
    }

    Ok(())
}

async fn handle_message_reaction_count(
    reaction_count: MessageReactionCountUpdated,
    pool: PgPool,
) -> ResponseResult<()> {
    if let Err(err) = save_message_reaction_count(&pool, &reaction_count).await {
        tracing::error!(%err, "failed to save message reaction count");
    }

    Ok(())
}

async fn handle_chat_member(member: ChatMemberUpdated, pool: PgPool) -> ResponseResult<()> {
    if let Err(err) = save_chat_member_event(&pool, &member).await {
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

    // Gemma handles text and vision in one request. If Telegram attached several
    // photo sizes, use the largest one so charts and small text stay readable.
    let image_base64 = download_largest_photo_base64(bot, msg).await?;
    let chat_member_count = get_chat_member_count(bot, config).await;
    let memory_notes = load_relevant_memory_notes(pool, &clean_post).await?;
    let recent_comments = load_recent_bot_comments(pool).await?;
    let prompt = build_llm_prompt(
        &clean_post,
        chat_member_count,
        &memory_notes,
        &recent_comments,
    );
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

async fn send_chat_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> ResponseResult<()> {
    let report = build_chat_stats_report(pool, config, period)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build chat stats");
            teloxide::RequestError::Io(std::io::Error::other("stats failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

async fn send_user_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    target: &str,
) -> ResponseResult<()> {
    let report = build_user_stats_report(pool, config, target)
        .await
        .map_err(|err| {
            tracing::error!(%err, "failed to build user stats");
            teloxide::RequestError::Io(std::io::Error::other("user stats failed"))
        })?;

    send_html(bot, chat_id, report).await?;

    Ok(())
}

async fn build_chat_stats_report(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<String> {
    let start_sql = period.start_sql();
    let summary_sql = format!(
        r#"
        with bounds as (
            select {start_sql} as start_at, now() as end_at
        ),
        messages as (
            select m.*
            from telegram_messages m, bounds b
            where m.chat_id = $1
              and m.created_at >= b.start_at
              and m.created_at < b.end_at
        ),
        bot_comments as (
            select j.*
            from post_comment_jobs j, bounds b
            where j.discussion_chat_id = $1
              and j.created_at >= b.start_at
              and j.created_at < b.end_at
        ),
        reactions as (
            select r.*
            from telegram_message_reactions r, bounds b
            where r.chat_id = $1
              and r.event_at >= b.start_at
              and r.event_at < b.end_at
        ),
        reaction_counts as (
            select rc.*
            from telegram_message_reaction_counts rc, bounds b
            where rc.chat_id = $1
              and rc.event_at >= b.start_at
              and rc.event_at < b.end_at
        ),
        member_events as (
            select e.*
            from telegram_chat_member_events e, bounds b
            where e.chat_id = $1
              and e.event_at >= b.start_at
              and e.event_at < b.end_at
        )
        select
            to_char((select start_at from bounds) at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI') as start_label,
            count(*)::bigint as messages,
            count(distinct user_id) filter (where source_channel_id is null and coalesce(user_id, 0) <> 777000)::bigint as active_users,
            count(*) filter (where reply_to_message_id is not null)::bigint as replies,
            count(*) filter (where has_links)::bigint as links,
            count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
            count(*) filter (where is_automatic_forward)::bigint as channel_posts,
            (select count(*) from bot_comments)::bigint as bot_comments,
            (select count(*) from messages m join bot_comments j on m.reply_to_message_id = j.bot_comment_message_id)::bigint as replies_to_bot,
            (select count(*) from reactions)::bigint as reaction_events,
            (select count(*) from reaction_counts)::bigint as reaction_count_updates,
            (select coalesce(sum(rc.total_count), 0)::bigint from reaction_counts rc join post_comment_jobs j on j.discussion_chat_id = rc.chat_id and j.bot_comment_message_id = rc.message_id)::bigint as bot_comment_reactions,
            (select count(*) from member_events where old_status in ('left', 'banned') and new_status not in ('left', 'banned'))::bigint as joins,
            (select count(*) from member_events where old_status not in ('left', 'banned') and new_status in ('left', 'banned'))::bigint as leaves
        from messages
        "#
    );

    let summary: ChatStatsSummary = sqlx::query_as(&summary_sql)
        .bind(config.discussion_chat_id)
        .fetch_one(pool)
        .await?;

    let attraction_sql = format!(
        r#"
        with bounds as (
            select {start_sql} as start_at, now() as end_at
        ),
        metrics as (
            select j.source_message_id,
                   count(m.*) filter (where m.created_at <= j.created_at + interval '5 minutes' and coalesce(m.text,'') !~ '^/') as msg_5m,
                   count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as msg_30m,
                   count(distinct m.user_id) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/') as users_30m
            from post_comment_jobs j
            left join telegram_messages m
              on m.chat_id = j.discussion_chat_id
             and m.created_at > j.created_at
             and m.created_at <= j.created_at + interval '30 minutes'
             and m.message_id <> j.bot_comment_message_id
             and m.source_channel_id is null
            where j.discussion_chat_id = $1
              and j.created_at >= (select start_at from bounds)
              and j.created_at < (select end_at from bounds)
            group by j.source_message_id, j.created_at, j.bot_comment_message_id
        )
        select
            coalesce(round(avg(msg_5m)::numeric, 2), 0)::text,
            coalesce(round(avg(msg_30m)::numeric, 2), 0)::text,
            coalesce(round(avg(users_30m)::numeric, 2), 0)::text
        from metrics
        "#
    );
    let attraction: (String, String, String) = sqlx::query_as(&attraction_sql)
        .bind(config.discussion_chat_id)
        .fetch_one(pool)
        .await?;

    let top_users = top_users_for_period(pool, config, period).await?;
    let top_bot_comments = top_bot_comments_for_period(pool, config, period).await?;

    let mut report = format!(
        "<b>Статистика за {}</b>\nПериод с <code>{}</code> МСК\n\nСообщения: <b>{}</b>\nАктивных пользователей: <b>{}</b>\nРеплаи: <b>{}</b>, ссылки: <b>{}</b>, медиа: <b>{}</b>\nПосты канала: <b>{}</b>, комменты бота: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nРеакции events: <b>{}</b>, count updates: <b>{}</b>\nРеакции на комменты бота: <b>{}</b>\nВходы: <b>{}</b>, выходы: <b>{}</b>\n\nЗавлечение после коммента: 5м <b>{}</b>, 30м <b>{}</b>, людей 30м <b>{}</b>",
        period.title(),
        escape_html(&summary.0),
        summary.1,
        summary.2,
        summary.3,
        summary.4,
        summary.5,
        summary.6,
        summary.7,
        summary.8,
        summary.9,
        summary.10,
        summary.11,
        summary.12,
        summary.13,
        attraction.0,
        attraction.1,
        attraction.2,
    );

    if !top_users.is_empty() {
        report.push_str("\n\n<b>Топ пользователей</b>\n");
        report.push_str(&top_users.join("\n"));
    }

    if !top_bot_comments.is_empty() {
        report.push_str("\n\n<b>Комменты бота</b>\n");
        report.push_str(&top_bot_comments.join("\n"));
    }

    Ok(report)
}

async fn top_users_for_period(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<Vec<String>> {
    let sql = format!(
        r#"
        with bounds as (
            select {} as start_at, now() as end_at
        )
        select m.user_id,
               p.username,
               p.first_name,
               p.last_name,
               count(*)::bigint as messages,
               count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_links)::bigint as links,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
          and m.created_at >= (select start_at from bounds)
          and m.created_at < (select end_at from bounds)
        group by m.user_id, p.username, p.first_name, p.last_name, s.status, s.is_admin, s.is_present
        order by messages desc
        limit 8
        "#,
        period.start_sql()
    );

    let rows = sqlx::query_as::<
        _,
        (
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            i64,
            i64,
            String,
            bool,
            bool,
        ),
    >(&sql)
    .bind(config.discussion_chat_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                user_id,
                username,
                first_name,
                last_name,
                messages,
                replies,
                links,
                media,
                status,
                is_admin,
                is_present,
            )| {
                let user = UserPresentation {
                    user_id,
                    display_name: display_name(
                        username.as_deref(),
                        first_name.as_deref(),
                        last_name.as_deref(),
                        user_id,
                    ),
                    is_bot: false,
                    status: Some(status),
                    is_admin,
                    is_present: Some(is_present),
                };
                format!(
                    "{}: <b>{}</b> соо, {} реплаев, {} ссылок, {} медиа",
                    user.linked_with_badges(),
                    messages,
                    replies,
                    links,
                    media
                )
            },
        )
        .collect())
}

async fn top_bot_comments_for_period(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<Vec<String>> {
    let sql = format!(
        r#"
        with bounds as (
            select {} as start_at, now() as end_at
        )
        select j.source_message_id,
               coalesce(g.response, '') as response,
               count(m.*) filter (where m.created_at <= j.created_at + interval '30 minutes' and coalesce(m.text,'') !~ '^/')::bigint as msg_30m,
               count(m.*) filter (where m.reply_to_message_id = j.bot_comment_message_id)::bigint as direct_replies,
               coalesce(max(rc.total_count), 0)::bigint as reactions
        from post_comment_jobs j
        left join llm_generations g on g.post_comment_job_id = j.id
        left join telegram_messages m
          on m.chat_id = j.discussion_chat_id
         and m.created_at > j.created_at
         and m.created_at <= j.created_at + interval '30 minutes'
         and m.message_id <> j.bot_comment_message_id
         and m.source_channel_id is null
        left join telegram_message_reaction_counts rc
          on rc.chat_id = j.discussion_chat_id
         and rc.message_id = j.bot_comment_message_id
        where j.discussion_chat_id = $1
          and j.created_at >= (select start_at from bounds)
          and j.created_at < (select end_at from bounds)
        group by j.source_message_id, g.response
        order by msg_30m desc, direct_replies desc, reactions desc
        limit 5
        "#,
        period.start_sql()
    );

    let rows = sqlx::query_as::<_, (i32, String, i64, i64, i64)>(&sql)
        .bind(config.discussion_chat_id)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(
            |(source_message_id, response, msg_30m, direct_replies, reactions)| {
                let clean_response = human_comment_preview(&response);
                format!(
                    "#{}: {} соо за 30м, {} реплаев, {} реакций - {}",
                    source_message_id,
                    msg_30m,
                    direct_replies,
                    reactions,
                    escape_html(&first_text_chars(&clean_response, 110))
                )
            },
        )
        .collect())
}

fn human_member_status(status: &str) -> &'static str {
    match status {
        "administrator" => "админ",
        "owner" => "владелец",
        "member" => "в чате",
        "restricted" => "ограничен",
        "left" => "не в чате",
        "banned" => "забанен",
        _ => "статус неизвестен",
    }
}

fn human_comment_preview(text: &str) -> String {
    normalize_ai_markers(text)
        .replace("{CHAT_LINK}", "чат")
        .replace("  ", " ")
        .trim()
        .to_string()
}

async fn build_user_stats_report(
    pool: &PgPool,
    config: &Config,
    target: &str,
) -> anyhow::Result<String> {
    let Some(user_id) = resolve_user_id(pool, target).await? else {
        return Ok(format!(
            "Не нашёл пользователя <code>{}</code>. Используй id или @username из уже виденных ботом пользователей.",
            escape_html(target)
        ));
    };

    let profile = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, bool)>(
        r#"
        select username, first_name, last_name, is_bot
        from telegram_user_profiles
        where telegram_user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let member = sqlx::query_as::<_, (String, bool, bool, Option<String>)>(
        r#"
        select status, is_admin, is_present, to_char(observed_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI')
        from telegram_chat_member_snapshots
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let totals = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64)>(
        r#"
        select count(*)::bigint as messages,
               count(*) filter (where reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where has_links)::bigint as links,
               count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
               count(*) filter (where reply_to_message_id in (select discussion_message_id from post_comment_jobs))::bigint as replies_to_channel_posts,
               count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs))::bigint as replies_to_bot,
               count(distinct date_trunc('day', created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days
        from telegram_messages
        where chat_id = $1 and user_id = $2 and source_channel_id is null
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?;

    let reactions_given = sqlx::query_as::<_, (i64,)>(
        r#"
        select count(*)::bigint
        from telegram_message_reactions
        where chat_id = $1 and user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .0;

    let reactions_received = sqlx::query_as::<_, (i64,)>(
        r#"
        select coalesce(sum(rc.total_count), 0)::bigint
        from telegram_messages m
        join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        where m.chat_id = $1 and m.user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .0;

    let user = match (profile.as_ref(), member.as_ref()) {
        (
            Some((username, first_name, last_name, is_bot)),
            Some((status, is_admin, is_present, _)),
        ) => UserPresentation {
            user_id,
            display_name: display_name(
                username.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                user_id,
            ),
            is_bot: *is_bot,
            status: Some(status.clone()),
            is_admin: *is_admin,
            is_present: Some(*is_present),
        },
        (Some((username, first_name, last_name, is_bot)), None) => UserPresentation {
            user_id,
            display_name: display_name(
                username.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                user_id,
            ),
            is_bot: *is_bot,
            status: None,
            is_admin: false,
            is_present: None,
        },
        (None, Some((status, is_admin, is_present, _))) => UserPresentation {
            user_id,
            display_name: user_id.to_string(),
            is_bot: false,
            status: Some(status.clone()),
            is_admin: *is_admin,
            is_present: Some(*is_present),
        },
        (None, None) => UserPresentation {
            user_id,
            display_name: user_id.to_string(),
            is_bot: false,
            status: None,
            is_admin: false,
            is_present: None,
        },
    };

    let observed_at = member
        .as_ref()
        .and_then(|(_, _, _, observed_at)| observed_at.as_deref())
        .unwrap_or("нет данных");

    Ok(format!(
        "<b>Статистика пользователя</b>\n{}\nСтатус обновлён: <code>{}</code>\n\nСообщения: <b>{}</b>\nРеплаи: <b>{}</b>\nРеплаи на посты: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nСсылки: <b>{}</b>, медиа: <b>{}</b>\nАктивных дней: <b>{}</b>\nРеакций поставил: <b>{}</b>\nРеакций получил: <b>{}</b>",
        user.linked_with_badges(),
        escape_html(observed_at),
        totals.0,
        totals.1,
        totals.4,
        totals.5,
        totals.2,
        totals.3,
        totals.6,
        reactions_given,
        reactions_received,
    ))
}

async fn resolve_user_id(pool: &PgPool, target: &str) -> anyhow::Result<Option<i64>> {
    let clean = target.trim();
    if clean.is_empty() {
        return Ok(None);
    }

    if let Ok(user_id) = clean.parse::<i64>() {
        return Ok(Some(user_id));
    }

    let username = clean.trim_start_matches('@').to_lowercase();
    let row = sqlx::query_as::<_, (i64,)>(
        r#"
        select telegram_user_id
        from telegram_user_profiles
        where lower(username) = $1
        order by updated_at desc
        limit 1
        "#,
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(user_id,)| user_id))
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

    sqlx::query(
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

fn message_has_links(msg: &Message) -> bool {
    let text_has_links = message_text(msg)
        .map(|text| text.contains("http://") || text.contains("https://") || text.contains("t.me/"))
        .unwrap_or(false);

    text_has_links
        || msg
            .entities()
            .into_iter()
            .flatten()
            .chain(msg.caption_entities().into_iter().flatten())
            .any(|entity| {
                matches!(
                    entity.kind,
                    MessageEntityKind::Url | MessageEntityKind::TextLink { .. }
                )
            })
}

async fn save_message_reaction(
    pool: &PgPool,
    reaction: &MessageReactionUpdated,
) -> anyhow::Result<()> {
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
            status: new_status,
            is_admin,
            is_present,
            raw_json,
            observed_at: member.date,
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

            (score >= 3).then_some(MergeCandidate {
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
        if keyword_phrase_matches(&lower, phrase) {
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

fn keyword_phrase_matches(text: &str, phrase: &str) -> bool {
    if phrase.chars().count() <= 3 && phrase.chars().all(|ch| ch.is_ascii_alphanumeric()) {
        return text
            .split(|ch: char| !ch.is_alphanumeric())
            .any(|token| token == phrase);
    }

    text.contains(phrase)
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
