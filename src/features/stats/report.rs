use std::path::Path;

use sqlx::PgPool;
use teloxide::net::Download;
use teloxide::prelude::*;
use tokio::io::AsyncWriteExt;

use crate::config::Config;
use crate::db::telegram::refresh_chat_member_snapshot;
use crate::features::stats::types::{
    ChatStatsSummary, StatsPeriod, StatsRender, UserPresentation, display_name,
};
use crate::features::user_profiles::service::refresh_profile;
use crate::telegram::html::{Html, truncate_text};
use crate::telegram::render::{escape_html, send_html, send_rich_html};
use crate::text::normalize_ai_markers;

const HTML_TOP_LIMIT: i64 = 20;
const RICH_TOP_LIMIT: i64 = 30;
const USER_TOP_WORDS_LIMIT: i64 = 10;

const USER_TOP_WORD_STOP_WORDS: &[&str] = &[
    // Russian pronouns, particles, conjunctions and filler words. Keep brand/tech words
    // like `амд`, `amd`, `nvidia`, `нвидиа`, `rtx`, `dlss` visible in user profiles.
    "а",
    "без",
    "более",
    "будем",
    "будет",
    "будешь",
    "больше",
    "будто",
    "буду",
    "будут",
    "будь",
    "был",
    "была",
    "были",
    "было",
    "быть",
    "вам",
    "вас",
    "ваще",
    "ведь",
    "везде",
    "весь",
    "вполне",
    "вроде",
    "все",
    "всего",
    "всем",
    "всему",
    "всех",
    "всю",
    "вся",
    "всё",
    "всегда",
    "вообще",
    "вот",
    "времени",
    "время",
    "вряд",
    "выглядит",
    "где",
    "говорит",
    "говорить",
    "говорю",
    "говоря",
    "говорят",
    "год",
    "года",
    "году",
    "давно",
    "давай",
    "даже",
    "далеко",
    "дальше",
    "данный",
    "два",
    "две",
    "делать",
    "деле",
    "дело",
    "день",
    "действительно",
    "для",
    "долго",
    "достаточно",
    "друг",
    "другая",
    "другие",
    "других",
    "другое",
    "другой",
    "думаешь",
    "думаю",
    "думал",
    "его",
    "ее",
    "её",
    "если",
    "есть",
    "еще",
    "ещё",
    "жаль",
    "ждать",
    "ждем",
    "ждём",
    "жду",
    "зачем",
    "зато",
    "здесь",
    "знаешь",
    "знает",
    "знаю",
    "ибо",
    "или",
    "именно",
    "иначе",
    "иногда",
    "интересно",
    "их",
    "какая",
    "какие",
    "каким",
    "каких",
    "какое",
    "какой",
    "каком",
    "какую",
    "каждый",
    "кажется",
    "как",
    "когда",
    "кого",
    "кому",
    "кто",
    "конечно",
    "короче",
    "которого",
    "которой",
    "которое",
    "которую",
    "которые",
    "который",
    "которых",
    "кроме",
    "кстати",
    "куда",
    "ладно",
    "лет",
    "либо",
    "лишь",
    "лучше",
    "любой",
    "максимум",
    "мало",
    "менее",
    "меня",
    "между",
    "мере",
    "мне",
    "много",
    "мог",
    "могли",
    "могу",
    "могут",
    "может",
    "можешь",
    "можно",
    "мой",
    "моя",
    "мои",
    "мол",
    "момент",
    "нам",
    "над",
    "надо",
    "наверное",
    "надеюсь",
    "наконец",
    "намного",
    "нас",
    "настолько",
    "насколько",
    "нахуй",
    "начал",
    "начала",
    "начали",
    "неё",
    "нее",
    "ней",
    "него",
    "некоторые",
    "некоторых",
    "нельзя",
    "нем",
    "немного",
    "нет",
    "нету",
    "нехуй",
    "нибудь",
    "ниже",
    "них",
    "нихуя",
    "ничего",
    "но",
    "норм",
    "нормально",
    "нужно",
    "нужен",
    "нужна",
    "нужны",
    "обычно",
    "одна",
    "однако",
    "одно",
    "одного",
    "одной",
    "одну",
    "один",
    "около",
    "она",
    "они",
    "оно",
    "опять",
    "особенно",
    "особо",
    "остальные",
    "остальное",
    "откуда",
    "очевидно",
    "очень",
    "пару",
    "перед",
    "пиздец",
    "под",
    "пока",
    "пол",
    "полностью",
    "получается",
    "понял",
    "понимаешь",
    "понимаю",
    "понять",
    "понятно",
    "пор",
    "после",
    "последний",
    "последние",
    "посмотрим",
    "почему",
    "похоже",
    "похуй",
    "походу",
    "поэтому",
    "прав",
    "правда",
    "практически",
    "при",
    "придется",
    "пример",
    "примерно",
    "принципе",
    "про",
    "просто",
    "против",
    "проще",
    "прям",
    "прямо",
    "пусть",
    "ради",
    "раз",
    "раза",
    "разве",
    "разные",
    "разных",
    "разницу",
    "раньше",
    "реально",
    "решил",
    "решили",
    "речь",
    "сам",
    "сама",
    "самая",
    "сами",
    "самого",
    "самое",
    "самом",
    "самый",
    "свое",
    "своего",
    "своей",
    "своим",
    "своими",
    "своих",
    "свой",
    "свою",
    "свои",
    "себе",
    "себя",
    "сейчас",
    "сильно",
    "сказал",
    "сколько",
    "скоро",
    "слишком",
    "сложно",
    "слова",
    "смотря",
    "снова",
    "совсем",
    "спасибо",
    "спустя",
    "сразу",
    "стал",
    "стали",
    "стало",
    "столько",
    "стоит",
    "стоят",
    "судя",
    "сути",
    "суть",
    "так",
    "такая",
    "такие",
    "таким",
    "такими",
    "такого",
    "такое",
    "такой",
    "таком",
    "такую",
    "там",
    "твой",
    "твои",
    "тебе",
    "тебя",
    "тем",
    "теперь",
    "типа",
    "типо",
    "того",
    "тоже",
    "ток",
    "только",
    "том",
    "тому",
    "тот",
    "точно",
    "три",
    "туда",
    "тут",
    "тупо",
    "тысяч",
    "увидел",
    "уверен",
    "угодно",
    "уже",
    "условно",
    "учитывая",
    "факт",
    "факту",
    "хоть",
    "хотел",
    "хотеть",
    "хочется",
    "хочешь",
    "хочу",
    "хотя",
    "хотят",
    "хуй",
    "хуйня",
    "хуже",
    "целом",
    "чего",
    "чел",
    "чем",
    "через",
    "честно",
    "чет",
    "чисто",
    "чтобы",
    "чтоб",
    "что",
    "чуть",
    "щас",
    "это",
    "этого",
    "этой",
    "этом",
    "этому",
    "этот",
    "эту",
    "эти",
    "этих",
    "этим",
    "явно",
    // Common English/link noise.
    "and",
    "are",
    "com",
    "for",
    "https",
    "not",
    "that",
    "the",
    "this",
    "with",
    "www",
    "you",
    "youtu",
    "youtube",
];

#[derive(sqlx::FromRow)]
struct TopReactedRow {
    message_id: i32,
    user_id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    is_bot: bool,
    status: String,
    is_admin: bool,
    is_present: bool,
    text: Option<String>,
    has_photo: bool,
    has_video: bool,
    has_document: bool,
    has_audio: bool,
    has_voice: bool,
    has_sticker: bool,
    has_animation: bool,
    total_count: i64,
}

pub async fn send_chat_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
    render: StatsRender,
) -> ResponseResult<()> {
    let report = match render {
        StatsRender::Html => build_chat_stats_report(pool, config, period).await,
        StatsRender::Rich => build_chat_stats_rich_report(pool, config, period).await,
    }
    .map_err(|err| {
        tracing::error!(%err, "failed to build chat stats");
        teloxide::RequestError::Io(std::io::Error::other("stats failed"))
    })?;

    send_stats_report(bot, chat_id, report, render).await?;

    Ok(())
}

pub async fn send_top_messages(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    render: StatsRender,
) -> ResponseResult<()> {
    refresh_top_message_users(bot, pool, config).await;

    let report = match render {
        StatsRender::Html => build_top_messages_report(pool, config).await,
        StatsRender::Rich => build_top_messages_rich_report(pool, config).await,
    }
    .map_err(|err| {
        tracing::error!(%err, "failed to build top messages report");
        teloxide::RequestError::Io(std::io::Error::other("top messages failed"))
    })?;

    send_stats_report(bot, chat_id, report, render).await?;

    Ok(())
}

pub async fn send_top_reacted(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    render: StatsRender,
) -> ResponseResult<()> {
    refresh_top_reacted_users(bot, pool, config).await;

    let report = match render {
        StatsRender::Html => build_top_reacted_report(pool, config).await,
        StatsRender::Rich => build_top_reacted_rich_report(pool, config).await,
    }
    .map_err(|err| {
        tracing::error!(%err, "failed to build top reacted report");
        teloxide::RequestError::Io(std::io::Error::other("top reacted failed"))
    })?;

    send_stats_report(bot, chat_id, report, render).await?;

    Ok(())
}

pub async fn send_user_stats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
    render: StatsRender,
) -> ResponseResult<()> {
    if let Some(user_id) = numeric_target_user_id(target).or(reply_user_id) {
        refresh_user_profile_from_telegram(bot, pool, config, user_id).await;
    }

    let report = match render {
        StatsRender::Html => build_user_stats_report(pool, config, target, reply_user_id).await,
        StatsRender::Rich => {
            build_user_stats_rich_report(bot, pool, config, target, reply_user_id).await
        }
    }
    .map_err(|err| {
        tracing::error!(%err, "failed to build user stats");
        teloxide::RequestError::Io(std::io::Error::other("user stats failed"))
    })?;

    send_stats_report(bot, chat_id, report, render).await?;

    Ok(())
}

async fn send_stats_report(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    report: String,
    render: StatsRender,
) -> ResponseResult<Message> {
    match render {
        StatsRender::Html => send_html(bot, chat_id, report).await,
        StatsRender::Rich => send_rich_html(chat_id, report).await,
    }
}

#[derive(sqlx::FromRow)]
struct TopMessageRow {
    user_id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    is_bot: bool,
    status: String,
    is_admin: bool,
    is_present: bool,
    messages: i64,
    replies: i64,
    media: i64,
    voices: i64,
    links: i64,
    reactions_received: i64,
}

#[derive(sqlx::FromRow)]
struct PeriodTopUserRow {
    user_id: i64,
    username: Option<String>,
    first_name: Option<String>,
    last_name: Option<String>,
    messages: i64,
    replies: i64,
    links: i64,
    media: i64,
    status: String,
    is_admin: bool,
    is_present: bool,
}

async fn build_chat_stats_rich_report(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<String> {
    let summary = chat_stats_summary(pool, config, period).await?;
    let attraction = chat_attraction_metrics(pool, config, period).await?;
    let top_users = rich_period_top_users(pool, config, period).await?;
    let bot_comments = rich_bot_comments(pool, config, period).await?;

    let summary_table = rich_table_no_header(&[
        vec![
            "Период с".to_string(),
            escape_html(&format!("{} МСК", summary.start_label)),
        ],
        vec!["Сообщения".to_string(), bold_num(summary.messages)],
        vec![
            "Активные пользователи".to_string(),
            bold_num(summary.active_users),
        ],
        vec!["Реплаи".to_string(), bold_num(summary.replies)],
        vec!["Ссылки".to_string(), bold_num(summary.links)],
        vec!["Медиа".to_string(), bold_num(summary.media)],
        vec!["Посты канала".to_string(), bold_num(summary.channel_posts)],
        vec![
            "Комментарии бота".to_string(),
            bold_num(summary.bot_comments),
        ],
        vec![
            "Реплаи на бота".to_string(),
            bold_num(summary.replies_to_bot),
        ],
        vec![
            "Реакции events".to_string(),
            bold_num(summary.reaction_events),
        ],
        vec![
            "Reaction count updates".to_string(),
            bold_num(summary.reaction_count_updates),
        ],
        vec![
            "Реакции на комменты бота".to_string(),
            bold_num(summary.bot_comment_reactions),
        ],
        vec![
            "Входы / выходы".to_string(),
            format!("{} / {}", bold_num(summary.joins), bold_num(summary.leaves)),
        ],
    ]);
    let attraction_table = rich_table(
        &["Окно", "Среднее"],
        &[
            vec![
                "5 минут".to_string(),
                format!("<strong>{}</strong> сообщений", escape_html(&attraction.0)),
            ],
            vec![
                "30 минут".to_string(),
                format!("<strong>{}</strong> сообщений", escape_html(&attraction.1)),
            ],
            vec![
                "Людей за 30 минут".to_string(),
                format!("<strong>{}</strong>", escape_html(&attraction.2)),
            ],
        ],
    );

    Ok(format!(
        "<h1>Статистика за {}</h1><details open><summary>Сводка периода</summary>{}</details><details open><summary>Завлечение после комментария</summary>{}</details><details open><summary>Топ пользователей</summary>{}</details><details><summary>Комментарии бота</summary>{}</details><hr/><footer>Rich-версия построена отдельным рендером: таблицы, секции и кликабельные профили без конвертации старого HTML.</footer>",
        escape_html(period.title()),
        summary_table,
        attraction_table,
        top_users,
        bot_comments
    ))
}

async fn build_top_messages_rich_report(pool: &PgPool, config: &Config) -> anyhow::Result<String> {
    let rows = top_message_rows(pool, config, RICH_TOP_LIMIT).await?;
    if rows.is_empty() {
        return Ok("<h1>Топ пишущих</h1><p>Нет данных.</p>".to_string());
    }

    let mut detail_rows = Vec::new();
    let table_rows = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let user = top_message_user(&row);
            let user_link = rich_user_link(&row.username, row.user_id, &user.display_name);
            detail_rows.push(vec![
                user_link.clone(),
                row.replies.to_string(),
                format!("{} / {}", row.media, row.voices),
                row.links.to_string(),
            ]);
            vec![
                (index + 1).to_string(),
                user_link,
                bold_num(row.messages),
                row.reactions_received.to_string(),
            ]
        })
        .collect::<Vec<_>>();

    Ok(format!(
        "<h1>Топ пишущих</h1>{}<details><summary>Дополнительно</summary>{}</details><hr/><footer>Имена кликабельны, основная таблица короткая; расширенные метрики спрятаны ниже.</footer>",
        rich_table(&["#", "Кто", "Соо", "Реакции"], &table_rows),
        rich_table(&["Кто", "Reply", "Медиа / voice", "Ссылки"], &detail_rows)
    ))
}

async fn build_top_reacted_rich_report(pool: &PgPool, config: &Config) -> anyhow::Result<String> {
    let rows = top_reacted_rows(pool, config, RICH_TOP_LIMIT).await?;
    if rows.is_empty() {
        return Ok("<h1>Топ сообщений по реакциям</h1><p>Нет данных.</p>".to_string());
    }

    let mut preview_rows = Vec::new();
    let table_rows = rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let user = top_reacted_user(&row);
            let preview = message_preview(
                row.text.as_deref(),
                MessageMediaPreview {
                    has_photo: row.has_photo,
                    has_video: row.has_video,
                    has_document: row.has_document,
                    has_audio: row.has_audio,
                    has_voice: row.has_voice,
                    has_sticker: row.has_sticker,
                    has_animation: row.has_animation,
                },
            );
            let message_link = Html::link(
                "сообщение",
                message_url(config.discussion_chat_id, row.message_id),
            )
            .into_string();
            preview_rows.push(vec![
                (index + 1).to_string(),
                message_link.clone(),
                escape_html(&truncate_text(&preview, 120)),
            ]);
            vec![
                (index + 1).to_string(),
                rich_user_or_message_link(
                    &row.username,
                    row.user_id,
                    &user.display_name,
                    config.discussion_chat_id,
                    row.message_id,
                ),
                bold_num(row.total_count),
                message_link,
            ]
        })
        .collect::<Vec<_>>();

    Ok(format!(
        "<h1>Топ реакций</h1>{}<details><summary>Превью сообщений</summary>{}</details><hr/><footer>Автор и сообщение кликабельны; тексты спрятаны, чтобы топ не превращался в простыню.</footer>",
        rich_table(&["#", "Автор", "❤", "Открыть"], &table_rows),
        rich_table(&["#", "Ссылка", "Текст"], &preview_rows)
    ))
}

async fn build_user_stats_rich_report(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<String> {
    let Some(user_id) = resolve_user_id(pool, target, reply_user_id).await? else {
        return Ok("<h1>Профиль не найден</h1><p>Не нашёл пользователя. Используй id, username из уже виденных ботом пользователей или reply на сообщение.</p>".to_string());
    };

    let profile = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            Option<String>,
            Option<String>,
            Option<String>,
        ),
    >(
        r#"
        select username, first_name, last_name, is_bot, bio,
               profile_photo_file_id, profile_photo_file_unique_id
        from telegram_user_profiles
        where telegram_user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let totals = user_totals(pool, config, user_id).await?;
    let reactions_given = user_reactions_given(pool, config, user_id).await?;
    let reactions_received = user_reactions_received(pool, config, user_id).await?;
    let member = sqlx::query_as::<_, (String, bool, bool, Option<String>, Option<String>)>(
        r#"
        select status,
               is_admin,
               is_present,
               to_char(observed_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               coalesce(
                   nullif(raw_json #>> '{custom_title}', ''),
                   nullif(raw_json #>> '{kind,custom_title}', ''),
                   nullif(raw_json #>> '{administrator,custom_title}', ''),
                   nullif(raw_json #>> '{owner,custom_title}', ''),
                   nullif(raw_json #>> '{member,custom_title}', ''),
                   nullif(raw_json #>> '{restricted,custom_title}', '')
               ) as written_tag
        from telegram_chat_member_snapshots
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let user_data = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<i32>,
            Option<i64>,
            Option<i64>,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        r#"
        select to_char(first_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               to_char(last_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               first_message_id,
               last_message_id,
               floor(extract(epoch from (now() - first_seen_at)) / 86400)::bigint,
               floor(extract(epoch from (now() - last_seen_at)) / 86400)::bigint,
               message_count,
               reply_count,
               link_count,
               media_count,
               reply_to_channel_post_count,
               reply_to_bot_count,
               0::bigint as voice_count
        from telegram_chat_users
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let (username, first_name, last_name, is_bot, bio, photo_file_id, photo_unique_id) =
        profile.unwrap_or((None, None, None, false, None, None, None));
    let avatar_url = cached_profile_photo_url(
        bot,
        config,
        user_id,
        photo_file_id.as_deref(),
        photo_unique_id.as_deref(),
    )
    .await;
    let (status, is_admin, is_present, _observed_at, written_tag) =
        member.unwrap_or(("unknown".to_string(), false, false, None, None));
    let user = UserPresentation {
        user_id,
        display_name: display_name(
            username.as_deref(),
            first_name.as_deref(),
            last_name.as_deref(),
            user_id,
        ),
        is_bot,
        status: Some(status),
        is_admin,
        is_present: Some(is_present),
    };
    let avatar_block = avatar_url
        .as_deref()
        .map(|url| format!("<img src=\"{}\"/>", escape_html(url)))
        .unwrap_or_default();
    let mut profile_rows = vec![vec![
        "имя".to_string(),
        rich_user_link(&username, user_id, &user.display_name),
    ]];
    if let Some(tag) = profile_written_tag(written_tag.as_deref()) {
        profile_rows.push(vec!["тег".to_string(), tag]);
    }
    let profile_block = rich_table_no_header(&profile_rows);
    let activity_block = rich_table_no_header(&[
        vec!["сообщения".to_string(), bold_num(totals.0)],
        vec!["reply".to_string(), bold_num(totals.1)],
        vec![
            "комментарии / боту".to_string(),
            format!("{} / {}", bold_num(totals.4), bold_num(totals.5)),
        ],
        vec!["ссылки".to_string(), bold_num(totals.2)],
        vec![
            "медиа / voice".to_string(),
            format!("{} / {}", bold_num(totals.3), bold_num(totals.7)),
        ],
        vec!["активных дней".to_string(), bold_num(totals.6)],
        vec![
            "реакции".to_string(),
            format!(
                "поставил {} / получил {}",
                bold_num(reactions_given),
                bold_num(reactions_received)
            ),
        ],
    ]);
    let (
        first_seen_at,
        last_seen_at,
        first_message_id,
        last_message_id,
        first_seen_days_ago,
        last_seen_days_ago,
        _cached_messages,
        _cached_replies,
        _cached_links,
        _cached_media,
        _cached_post_comments,
        _cached_replies_to_bot,
        _cached_voices,
    ) = user_data
        .map(
            |(
                first_seen_at,
                last_seen_at,
                first_message_id,
                last_message_id,
                first_seen_days_ago,
                last_seen_days_ago,
                cached_messages,
                cached_replies,
                cached_links,
                cached_media,
                cached_post_comments,
                cached_replies_to_bot,
                cached_voices,
            )| {
                (
                    first_seen_at.unwrap_or_else(|| "нет данных".to_string()),
                    last_seen_at.unwrap_or_else(|| "нет данных".to_string()),
                    first_message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "нет данных".to_string()),
                    last_message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "нет данных".to_string()),
                    first_seen_days_ago,
                    last_seen_days_ago,
                    cached_messages,
                    cached_replies,
                    cached_links,
                    cached_media,
                    cached_post_comments,
                    cached_replies_to_bot,
                    cached_voices,
                )
            },
        )
        .unwrap_or_else(|| {
            (
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                None,
                None,
                totals.0,
                totals.1,
                totals.2,
                totals.3,
                totals.4,
                totals.5,
                totals.7,
            )
        });
    let first_message = linked_message(
        config.discussion_chat_id,
        &first_seen_at,
        &first_message_id,
        first_seen_days_ago,
    );
    let last_message = linked_message(
        config.discussion_chat_id,
        &last_seen_at,
        &last_message_id,
        last_seen_days_ago,
    );
    let top_words = user_top_words(pool, config, user_id).await?;
    let top_words_text = if top_words.is_empty() {
        "нет данных".to_string()
    } else {
        top_words
            .iter()
            .map(|(word, count)| format!("{} ({})", escape_html(word), count))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let extra_block = rich_table_no_header(&[
        vec!["первое сообщение".to_string(), first_message],
        vec!["последнее сообщение".to_string(), last_message],
        vec!["частые слова".to_string(), top_words_text],
    ]);
    let bio_block = bio
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|bio| format!("<section><h3>Bio</h3><p>{}</p></section>", escape_html(bio)))
        .unwrap_or_default();
    Ok(format!(
        "<h1>{}</h1>{}<details open><summary>Основное</summary>{}</details>{}<details open><summary>Активность</summary>{}</details><details><summary>Дополнительно</summary>{}</details>",
        escape_html(&user.display_name),
        avatar_block,
        profile_block,
        bio_block,
        activity_block,
        extra_block,
    ))
}

fn rich_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut table = String::from("<table bordered striped><tr>");
    for header in headers {
        table.push_str("<th>");
        table.push_str(&escape_html(header));
        table.push_str("</th>");
    }
    table.push_str("</tr>");
    push_rich_table_rows(&mut table, rows);
    table.push_str("</table>");
    table
}

fn rich_table_no_header(rows: &[Vec<String>]) -> String {
    let mut table = String::from("<table bordered striped>");
    push_rich_table_rows(&mut table, rows);
    table.push_str("</table>");
    table
}

fn push_rich_table_rows(table: &mut String, rows: &[Vec<String>]) {
    for row in rows {
        table.push_str("<tr>");
        for cell in row {
            table.push_str("<td>");
            table.push_str(cell);
            table.push_str("</td>");
        }
        table.push_str("</tr>");
    }
}

fn bold_num(value: i64) -> String {
    format!("<strong>{value}</strong>")
}

fn profile_written_tag(tag: Option<&str>) -> Option<String> {
    tag.map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(escape_html)
}

async fn user_top_words(
    pool: &PgPool,
    config: &Config,
    user_id: i64,
) -> anyhow::Result<Vec<(String, i64)>> {
    let stop_words = USER_TOP_WORD_STOP_WORDS
        .iter()
        .map(|word| (*word).to_string())
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, (String, i64)>(
        r#"
        select word, count(*)::bigint as usage_count
        from telegram_messages m
        cross join lateral regexp_split_to_table(
            lower(coalesce(m.text, '')),
            '[^[:alnum:]а-яё]+'
        ) as word
        where m.chat_id = $1
          and m.user_id = $2
          and char_length(word) >= 3
          and word !~ '^[0-9]+$'
          and not (word = any($3::text[]))
        group by word
        order by usage_count desc, word asc
        limit $4
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .bind(&stop_words)
    .bind(USER_TOP_WORDS_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

fn rich_user_link(username: &Option<String>, user_id: i64, display_name: &str) -> String {
    let username = username
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches('@'));

    let url = username
        .map(|username| format!("https://t.me/{username}"))
        .unwrap_or_else(|| format!("tg://user?id={user_id}"));

    Html::link(display_name, url).into_string()
}

fn rich_user_or_message_link(
    username: &Option<String>,
    user_id: i64,
    display_name: &str,
    chat_id: i64,
    message_id: i32,
) -> String {
    let has_public_username = username
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.trim_start_matches('@').is_empty());

    if has_public_username {
        rich_user_link(username, user_id, display_name)
    } else {
        Html::link(display_name, message_url(chat_id, message_id)).into_string()
    }
}

async fn cached_profile_photo_url(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
    user_id: i64,
    file_id: Option<&str>,
    unique_id: Option<&str>,
) -> Option<String> {
    let public_base_url = config
        .public_base_url
        .as_deref()?
        .trim()
        .trim_end_matches('/');
    let file_id = file_id?.trim();
    if file_id.is_empty() {
        return None;
    }

    let avatars_dir = Path::new(&config.static_files_dir).join("avatars");
    let filename = format!(
        "{}_{}.jpg",
        user_id,
        safe_static_name(unique_id.unwrap_or("photo"))
    );
    let path = avatars_dir.join(&filename);

    if tokio::fs::metadata(&path).await.is_err()
        && let Err(err) = download_profile_photo(bot, file_id, &avatars_dir, &path).await
    {
        tracing::debug!(%err, user_id, "failed to cache profile photo");
        return None;
    }

    Some(format!(
        "{public_base_url}/tg-ai-bot-static/avatars/{filename}"
    ))
}

async fn download_profile_photo(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    file_id: &str,
    avatars_dir: &Path,
    path: &Path,
) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(avatars_dir).await?;
    let file = bot.get_file(file_id.to_string()).await?;
    let tmp_path = path.with_extension("tmp");
    let mut dst = tokio::fs::File::create(&tmp_path).await?;
    bot.download_file(&file.path, &mut dst).await?;
    dst.flush().await?;
    tokio::fs::rename(tmp_path, path).await?;
    Ok(())
}

fn safe_static_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect::<String>()
}

async fn chat_stats_summary(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<ChatStatsSummary> {
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

    sqlx::query_as(&summary_sql)
        .bind(config.discussion_chat_id)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

async fn chat_attraction_metrics(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<(String, String, String)> {
    let start_sql = period.start_sql();
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

    sqlx::query_as(&attraction_sql)
        .bind(config.discussion_chat_id)
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

async fn rich_period_top_users(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<String> {
    let rows = period_top_user_rows(pool, config, period).await?;
    if rows.is_empty() {
        return Ok("<p>Нет данных.</p>".to_string());
    }

    Ok(rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let user = UserPresentation {
                user_id: row.user_id,
                display_name: display_name(
                    row.username.as_deref(),
                    row.first_name.as_deref(),
                    row.last_name.as_deref(),
                    row.user_id,
                ),
                is_bot: false,
                status: Some(row.status),
                is_admin: row.is_admin,
                is_present: Some(row.is_present),
            };
            let stats = rich_table_no_header(&[
                vec!["сообщения".to_string(), bold_num(row.messages)],
                vec!["reply".to_string(), row.replies.to_string()],
                vec!["ссылки".to_string(), row.links.to_string()],
                vec!["медиа".to_string(), row.media.to_string()],
            ]);
            format!(
                "<details open><summary><strong>{}.</strong> {}</summary>{}</details>",
                index + 1,
                rich_user_link(&row.username, row.user_id, &user.display_name),
                stats
            )
        })
        .collect::<Vec<_>>()
        .join(""))
}

async fn period_top_user_rows(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<Vec<PeriodTopUserRow>> {
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

    sqlx::query_as::<_, PeriodTopUserRow>(&sql)
        .bind(config.discussion_chat_id)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

async fn rich_bot_comments(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<String> {
    let rows = bot_comment_rows(pool, config, period).await?;
    if rows.is_empty() {
        return Ok("<p>Нет данных.</p>".to_string());
    }

    let rows = rows
        .into_iter()
        .map(
            |(source_message_id, response, msg_30m, direct_replies, reactions)| {
                let clean_response = human_comment_preview(&response);
                vec![
                    Html::link(
                        format!("#{}", source_message_id),
                        message_url(config.discussion_chat_id, source_message_id),
                    )
                    .into_string(),
                    bold_num(msg_30m),
                    direct_replies.to_string(),
                    reactions.to_string(),
                    escape_html(&truncate_text(&clean_response, 120)),
                ]
            },
        )
        .collect::<Vec<_>>();

    Ok(rich_table(
        &["Пост", "30м", "Reply", "Реакции", "Комментарий"],
        &rows,
    ))
}

async fn bot_comment_rows(
    pool: &PgPool,
    config: &Config,
    period: StatsPeriod,
) -> anyhow::Result<Vec<(i32, String, i64, i64, i64)>> {
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

    sqlx::query_as::<_, (i32, String, i64, i64, i64)>(&sql)
        .bind(config.discussion_chat_id)
        .fetch_all(pool)
        .await
        .map_err(Into::into)
}

async fn top_message_rows(
    pool: &PgPool,
    config: &Config,
    limit: i64,
) -> anyhow::Result<Vec<TopMessageRow>> {
    sqlx::query_as::<_, TopMessageRow>(
        r#"
        select m.user_id,
               p.username,
               coalesce(
                   nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''),
                   raw_name.display_name,
                   'скрытый пользователь'
               ) as first_name,
               p.last_name,
               coalesce(p.is_bot, false) as is_bot,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present,
               count(*)::bigint as messages,
               count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               count(*) filter (where m.has_voice)::bigint as voices,
               count(*) filter (where m.has_links)::bigint as links,
               coalesce(sum(rc.total_count), 0)::bigint as reactions_received
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        left join lateral (
            select coalesce(
                       nullif(tm.raw_json #>> '{from,first_name}', ''),
                       nullif(tm.raw_json ->> 'from', '')
                   ) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id
              and tm.user_id = m.user_id
              and coalesce(
                      nullif(tm.raw_json #>> '{from,first_name}', ''),
                      nullif(tm.raw_json ->> 'from', '')
                  ) is not null
            order by tm.created_at desc
            limit 1
        ) raw_name on true
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        group by m.user_id, p.username, p.first_name, p.last_name, p.is_bot, s.status, s.is_admin, s.is_present, raw_name.display_name
        order by messages desc, reactions_received desc
        limit $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

fn top_message_user(row: &TopMessageRow) -> UserPresentation {
    UserPresentation {
        user_id: row.user_id,
        display_name: display_name(
            row.username.as_deref(),
            row.first_name.as_deref(),
            row.last_name.as_deref(),
            row.user_id,
        ),
        is_bot: row.is_bot,
        status: Some(row.status.clone()),
        is_admin: row.is_admin,
        is_present: Some(row.is_present),
    }
}

async fn top_reacted_rows(
    pool: &PgPool,
    config: &Config,
    limit: i64,
) -> anyhow::Result<Vec<TopReactedRow>> {
    sqlx::query_as::<_, TopReactedRow>(
        r#"
        select m.message_id,
               m.user_id,
               p.username,
               coalesce(
                   nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''),
                   raw_name.display_name,
                   'скрытый пользователь'
               ) as first_name,
               p.last_name,
               coalesce(p.is_bot, false) as is_bot,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present,
               m.text,
               m.has_photo,
               m.has_video,
               m.has_document,
               m.has_audio,
               m.has_voice,
               m.has_sticker,
               m.has_animation,
               rc.total_count
        from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join lateral (
            select coalesce(
                       nullif(tm.raw_json #>> '{from,first_name}', ''),
                       nullif(tm.raw_json ->> 'from', '')
                   ) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id
              and tm.user_id = m.user_id
              and coalesce(
                      nullif(tm.raw_json #>> '{from,first_name}', ''),
                      nullif(tm.raw_json ->> 'from', '')
                  ) is not null
            order by tm.created_at desc
            limit 1
        ) raw_name on true
        where rc.chat_id = $1
          and rc.total_count > 0
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        order by rc.total_count desc, m.created_at desc
        limit $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

fn top_reacted_user(row: &TopReactedRow) -> UserPresentation {
    UserPresentation {
        user_id: row.user_id,
        display_name: display_name(
            row.username.as_deref(),
            row.first_name.as_deref(),
            row.last_name.as_deref(),
            row.user_id,
        ),
        is_bot: row.is_bot,
        status: Some(row.status.clone()),
        is_admin: row.is_admin,
        is_present: Some(row.is_present),
    }
}

async fn user_totals(
    pool: &PgPool,
    config: &Config,
    user_id: i64,
) -> anyhow::Result<(i64, i64, i64, i64, i64, i64, i64, i64)> {
    sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64, i64)>(
        r#"
        with recursive post_thread_messages as (
            select chat_id, message_id
            from telegram_messages
            where chat_id = $1 and source_channel_id is not null

            union

            select child.chat_id, child.message_id
            from telegram_messages child
            join post_thread_messages parent
              on parent.chat_id = child.chat_id
             and parent.message_id = child.reply_to_message_id
            where child.chat_id = $1
              and child.source_channel_id is null
        )
        select count(*)::bigint as messages,
               count(*) filter (where reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where has_links)::bigint as links,
               count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
               count(*) filter (where message_id in (select message_id from post_thread_messages))::bigint as post_comments,
               count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs))::bigint as replies_to_bot,
               count(distinct date_trunc('day', created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days,
               count(*) filter (where has_voice)::bigint as voices
        from telegram_messages
        where chat_id = $1 and user_id = $2 and source_channel_id is null
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

async fn user_reactions_given(pool: &PgPool, config: &Config, user_id: i64) -> anyhow::Result<i64> {
    Ok(sqlx::query_as::<_, (i64,)>(
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
    .0)
}

async fn user_reactions_received(
    pool: &PgPool,
    config: &Config,
    user_id: i64,
) -> anyhow::Result<i64> {
    Ok(sqlx::query_as::<_, (i64,)>(
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
    .0)
}

#[allow(dead_code)]
fn html_to_rich_markdown(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;

    while let Some(start) = rest.find("<a href=\"") {
        out.push_str(&decode_basic_html(&rest[..start]));
        let after_href = &rest[start + "<a href=\"".len()..];
        let Some(end_url) = after_href.find("\">") else {
            out.push_str(&decode_basic_html(after_href));
            return html_tags_to_markdown(&out);
        };
        let url = &after_href[..end_url];
        let after_url = &after_href[end_url + 2..];
        let Some(end_text) = after_url.find("</a>") else {
            out.push_str(&decode_basic_html(after_url));
            return html_tags_to_markdown(&out);
        };
        let text = decode_basic_html(&after_url[..end_text]);
        out.push_str(&format!("[{}]({})", escape_rich_link_text(&text), url));
        rest = &after_url[end_text + "</a>".len()..];
    }

    out.push_str(&decode_basic_html(rest));
    html_tags_to_markdown(&out)
}

fn html_tags_to_markdown(text: &str) -> String {
    text.replace("<b>", "**")
        .replace("</b>", "**")
        .replace("<code>", "`")
        .replace("</code>", "`")
}

fn decode_basic_html(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
}

fn escape_rich_link_text(text: &str) -> String {
    text.replace('[', "\\[").replace(']', "\\]")
}

async fn build_top_messages_report(pool: &PgPool, config: &Config) -> anyhow::Result<String> {
    let rows = sqlx::query_as::<
        _,
        (
            i64,
            Option<String>,
            Option<String>,
            Option<String>,
            bool,
            String,
            bool,
            bool,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        r#"
        select m.user_id,
               p.username,
               coalesce(
                   nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''),
                   raw_name.display_name,
                   'скрытый пользователь'
               ) as first_name,
               p.last_name,
               coalesce(p.is_bot, false) as is_bot,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present,
               count(*)::bigint as messages,
               count(*) filter (where m.reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where m.has_photo or m.has_video or m.has_document or m.has_audio or m.has_voice or m.has_sticker or m.has_animation)::bigint as media,
               count(*) filter (where m.has_voice)::bigint as voices,
               count(*) filter (where m.has_links)::bigint as links,
               coalesce(sum(rc.total_count), 0)::bigint as reactions_received
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join telegram_message_reaction_counts rc on rc.chat_id = m.chat_id and rc.message_id = m.message_id
        left join lateral (
            select coalesce(
                       nullif(tm.raw_json #>> '{from,first_name}', ''),
                       nullif(tm.raw_json ->> 'from', '')
                   ) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id
              and tm.user_id = m.user_id
              and coalesce(
                      nullif(tm.raw_json #>> '{from,first_name}', ''),
                      nullif(tm.raw_json ->> 'from', '')
                  ) is not null
            order by tm.created_at desc
            limit 1
        ) raw_name on true
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        group by m.user_id, p.username, p.first_name, p.last_name, p.is_bot, s.status, s.is_admin, s.is_present, raw_name.display_name
        order by messages desc, reactions_received desc
        limit $2
        "#,
    )
        .bind(config.discussion_chat_id)
        .bind(HTML_TOP_LIMIT)
        .fetch_all(pool)
        .await?;

    let mut report = String::from("<b>Топ пишущих</b>\nЗа всё время\n");

    if rows.is_empty() {
        report.push_str("\nНет данных.");
        return Ok(report);
    }

    for (
        index,
        (
            user_id,
            username,
            first_name,
            last_name,
            is_bot,
            status,
            is_admin,
            is_present,
            messages,
            replies,
            media,
            voices,
            links,
            reactions_received,
        ),
    ) in rows.into_iter().enumerate()
    {
        let user = UserPresentation {
            user_id,
            display_name: display_name(
                username.as_deref(),
                first_name.as_deref(),
                last_name.as_deref(),
                user_id,
            ),
            is_bot,
            status: Some(status),
            is_admin,
            is_present: Some(is_present),
        };

        report.push_str(&format!(
            "\n{}. {}: <b>{}</b> соо, {} reply, {} медиа, {} голосовых, {} ссылок, {} реакций",
            index + 1,
            user.linked_with_known_badges(),
            messages,
            replies,
            media,
            voices,
            links,
            reactions_received
        ));
    }

    Ok(report)
}

async fn build_top_reacted_report(pool: &PgPool, config: &Config) -> anyhow::Result<String> {
    let rows = sqlx::query_as::<_, TopReactedRow>(
        r#"
        select m.message_id,
               m.user_id,
               p.username,
               coalesce(
                   nullif(case when p.first_name = 'пользователь' then '' else p.first_name end, ''),
                   raw_name.display_name,
                   'скрытый пользователь'
               ) as first_name,
               p.last_name,
               coalesce(p.is_bot, false) as is_bot,
               coalesce(s.status, 'unknown') as status,
               coalesce(s.is_admin, false) as is_admin,
               coalesce(s.is_present, false) as is_present,
               m.text,
               m.has_photo,
               m.has_video,
               m.has_document,
               m.has_audio,
               m.has_voice,
               m.has_sticker,
               m.has_animation,
               rc.total_count
        from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        left join telegram_chat_member_snapshots s on s.chat_id = m.chat_id and s.telegram_user_id = m.user_id
        left join lateral (
            select coalesce(
                       nullif(tm.raw_json #>> '{from,first_name}', ''),
                       nullif(tm.raw_json ->> 'from', '')
                   ) as display_name
            from telegram_messages tm
            where tm.chat_id = m.chat_id
              and tm.user_id = m.user_id
              and coalesce(
                      nullif(tm.raw_json #>> '{from,first_name}', ''),
                      nullif(tm.raw_json ->> 'from', '')
                  ) is not null
            order by tm.created_at desc
            limit 1
        ) raw_name on true
        where rc.chat_id = $1
          and rc.total_count > 0
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        order by rc.total_count desc, m.created_at desc
        limit $2
        "#,
    )
        .bind(config.discussion_chat_id)
        .bind(HTML_TOP_LIMIT)
        .fetch_all(pool)
        .await?;

    let mut report = String::from("<b>Топ сообщений по реакциям</b>\nЗа всё время\n");

    if rows.is_empty() {
        report.push_str("\nНет данных.");
        return Ok(report);
    }

    for (index, row) in rows.into_iter().enumerate() {
        let user = UserPresentation {
            user_id: row.user_id,
            display_name: display_name(
                row.username.as_deref(),
                row.first_name.as_deref(),
                row.last_name.as_deref(),
                row.user_id,
            ),
            is_bot: row.is_bot,
            status: Some(row.status),
            is_admin: row.is_admin,
            is_present: Some(row.is_present),
        };
        let preview = message_preview(
            row.text.as_deref(),
            MessageMediaPreview {
                has_photo: row.has_photo,
                has_video: row.has_video,
                has_document: row.has_document,
                has_audio: row.has_audio,
                has_voice: row.has_voice,
                has_sticker: row.has_sticker,
                has_animation: row.has_animation,
            },
        );
        let author_link = Html::link(
            user.display_name,
            message_url(config.discussion_chat_id, row.message_id),
        )
        .into_string();
        let preview = truncate_text(&preview, 64);

        report.push_str(&format!(
            "\n{}. <b>{}</b> - {}: {}",
            index + 1,
            row.total_count,
            author_link,
            Html::text(preview).into_string()
        ));
    }

    Ok(report)
}

async fn refresh_user_profile_from_telegram(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    user_id: i64,
) {
    if let Err(err) = refresh_chat_member_snapshot(bot, pool, config, user_id).await {
        tracing::debug!(%err, user_id, "failed to refresh member snapshot from Telegram");
    }

    if let Err(err) = refresh_profile(bot.inner(), pool, user_id).await {
        tracing::debug!(%err, user_id, "failed to refresh full user profile from Telegram");
    }
}

async fn refresh_top_message_users(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
) {
    let user_ids = match sqlx::query_as::<_, (i64,)>(
        r#"
        select m.user_id
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        group by m.user_id
        order by count(*) desc
        limit $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(RICH_TOP_LIMIT)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "failed to load top message users for refresh");
            return;
        }
    };

    refresh_member_snapshots_for_users(bot, pool, config, user_ids).await;
}

async fn refresh_top_reacted_users(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
) {
    let user_ids = match sqlx::query_as::<_, (i64,)>(
        r#"
        select m.user_id
        from telegram_message_reaction_counts rc
        join telegram_messages m on m.chat_id = rc.chat_id and m.message_id = rc.message_id
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where rc.chat_id = $1
          and rc.total_count > 0
          and m.user_id is not null
          and m.source_channel_id is null
          and m.user_id <> 777000
          and coalesce(p.is_bot, false) = false
        group by m.user_id
        order by max(rc.total_count) desc
        limit $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(RICH_TOP_LIMIT)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(%err, "failed to load top reacted users for refresh");
            return;
        }
    };

    refresh_member_snapshots_for_users(bot, pool, config, user_ids).await;
}

async fn refresh_member_snapshots_for_users(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    user_ids: Vec<(i64,)>,
) {
    for (user_id,) in user_ids {
        if let Err(err) = refresh_chat_member_snapshot(bot, pool, config, user_id).await {
            tracing::debug!(%err, user_id, "failed to refresh top user from Telegram");
        }
    }
}

fn numeric_target_user_id(target: Option<&str>) -> Option<i64> {
    clean_target_arg(target?).parse().ok()
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
        escape_html(&summary.start_label),
        summary.messages,
        summary.active_users,
        summary.replies,
        summary.links,
        summary.media,
        summary.channel_posts,
        summary.bot_comments,
        summary.replies_to_bot,
        summary.reaction_events,
        summary.reaction_count_updates,
        summary.bot_comment_reactions,
        summary.joins,
        summary.leaves,
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
                    user.linked_with_known_badges(),
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
                    Html::text(truncate_text(&clean_response, 110)).into_string()
                )
            },
        )
        .collect())
}

fn human_comment_preview(text: &str) -> String {
    normalize_ai_markers(text)
        .replace("{CHAT_LINK}", "чат")
        .replace("  ", " ")
        .trim()
        .to_string()
}

#[derive(Clone, Copy, Default)]
struct MessageMediaPreview {
    has_photo: bool,
    has_video: bool,
    has_document: bool,
    has_audio: bool,
    has_voice: bool,
    has_sticker: bool,
    has_animation: bool,
}

fn message_preview(text: Option<&str>, media: MessageMediaPreview) -> String {
    if let Some(text) = text.map(str::trim).filter(|value| !value.is_empty()) {
        return normalize_ai_markers(text);
    }

    let media = [
        (media.has_photo, "фото"),
        (media.has_video, "видео"),
        (media.has_document, "файл"),
        (media.has_audio, "аудио"),
        (media.has_voice, "голосовое"),
        (media.has_sticker, "стикер"),
        (media.has_animation, "GIF"),
    ]
    .into_iter()
    .filter_map(|(enabled, label)| enabled.then_some(label))
    .collect::<Vec<_>>();

    if media.is_empty() {
        "сообщение без текста".to_string()
    } else {
        format!("медиа: {}", media.join(", "))
    }
}

async fn build_user_stats_report(
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<String> {
    let Some(user_id) = resolve_user_id(pool, target, reply_user_id).await? else {
        let hint = match target.map(str::trim).filter(|value| !value.is_empty()) {
            Some(_) => "Не нашёл пользователя. Используй id, username из уже виденных ботом пользователей или reply на сообщение.".to_string(),
            None => "Не понял, кого смотреть. Отправь команду обычным сообщением, ответь ей на сообщение пользователя или передай id/username.".to_string(),
        };
        return Ok(hint);
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

    let user_data = sqlx::query_as::<
        _,
        (
            Option<String>,
            Option<String>,
            Option<i32>,
            Option<i32>,
            Option<i64>,
            Option<i64>,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        ),
    >(
        r#"
        select to_char(first_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               to_char(last_seen_at at time zone 'Europe/Moscow', 'YYYY-MM-DD HH24:MI'),
               first_message_id,
               last_message_id,
               floor(extract(epoch from (now() - first_seen_at)) / 86400)::bigint,
               floor(extract(epoch from (now() - last_seen_at)) / 86400)::bigint,
               message_count,
               reply_count,
               link_count,
               media_count,
               reply_to_channel_post_count,
               reply_to_bot_count,
               0::bigint as voice_count
        from telegram_chat_users
        where chat_id = $1 and telegram_user_id = $2
        "#,
    )
    .bind(config.discussion_chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let totals = sqlx::query_as::<_, (i64, i64, i64, i64, i64, i64, i64, i64)>(
        r#"
        with recursive post_thread_messages as (
            select chat_id, message_id
            from telegram_messages
            where chat_id = $1 and source_channel_id is not null

            union

            select child.chat_id, child.message_id
            from telegram_messages child
            join post_thread_messages parent
              on parent.chat_id = child.chat_id
             and parent.message_id = child.reply_to_message_id
            where child.chat_id = $1
              and child.source_channel_id is null
        )
        select count(*)::bigint as messages,
               count(*) filter (where reply_to_message_id is not null)::bigint as replies,
               count(*) filter (where has_links)::bigint as links,
               count(*) filter (where has_photo or has_video or has_document or has_audio or has_voice or has_sticker or has_animation)::bigint as media,
               count(*) filter (where message_id in (select message_id from post_thread_messages))::bigint as post_comments,
               count(*) filter (where reply_to_message_id in (select bot_comment_message_id from post_comment_jobs))::bigint as replies_to_bot,
               count(distinct date_trunc('day', created_at at time zone 'Europe/Moscow' - interval '5 hours'))::bigint as active_days,
               count(*) filter (where has_voice)::bigint as voices
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

    let (
        first_seen_at,
        last_seen_at,
        first_message_id,
        last_message_id,
        first_seen_days_ago,
        last_seen_days_ago,
        messages,
        replies,
        links,
        media,
        replies_to_channel_posts,
        replies_to_bot,
        voices,
    ) = user_data
        .map(
            |(
                first_seen_at,
                last_seen_at,
                first_message_id,
                last_message_id,
                first_seen_days_ago,
                last_seen_days_ago,
                messages,
                replies,
                links,
                media,
                replies_to_channel_posts,
                replies_to_bot,
                voices,
            )| {
                (
                    first_seen_at.unwrap_or_else(|| "нет данных".to_string()),
                    last_seen_at.unwrap_or_else(|| "нет данных".to_string()),
                    first_message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "нет данных".to_string()),
                    last_message_id
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "нет данных".to_string()),
                    first_seen_days_ago,
                    last_seen_days_ago,
                    messages,
                    replies,
                    links,
                    media,
                    replies_to_channel_posts,
                    replies_to_bot,
                    voices,
                )
            },
        )
        .unwrap_or_else(|| {
            (
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                None,
                None,
                totals.0,
                totals.1,
                totals.2,
                totals.3,
                totals.4,
                totals.5,
                totals.7,
            )
        });

    let first_message = linked_message(
        config.discussion_chat_id,
        &first_seen_at,
        &first_message_id,
        first_seen_days_ago,
    );
    let last_message = linked_message(
        config.discussion_chat_id,
        &last_seen_at,
        &last_message_id,
        last_seen_days_ago,
    );

    Ok(format!(
        "<b>Статистика пользователя</b>\n{}\nСтатус обновлён: <code>{}</code>\nПервое сообщение: {}\nПоследнее сообщение: {}\n\nСообщения: <b>{}</b>\nРеплаи: <b>{}</b>\nКомментарии: <b>{}</b>\nРеплаи на бота: <b>{}</b>\nСсылки: <b>{}</b>, медиа: <b>{}</b>, голосовые: <b>{}</b>\nАктивных дней: <b>{}</b>\nРеакций поставил: <b>{}</b>\nРеакций получил: <b>{}</b>",
        user.linked_with_badges(),
        escape_html(observed_at),
        first_message,
        last_message,
        totals.0.max(messages),
        totals.1.max(replies),
        totals.4.max(replies_to_channel_posts),
        totals.5.max(replies_to_bot),
        totals.2.max(links),
        totals.3.max(media),
        totals.7.max(voices),
        totals.6,
        reactions_given,
        reactions_received,
    ))
}

async fn resolve_user_id(
    pool: &PgPool,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<Option<i64>> {
    let clean = target.map(clean_target_arg).unwrap_or_default();
    if clean.is_empty() {
        return Ok(reply_user_id);
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

fn clean_target_arg(target: &str) -> String {
    target
        .split_whitespace()
        .filter(|part| !matches!(*part, "-r" | "--rich" | "-p" | "--plain" | "--poor"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn linked_message(
    chat_id: i64,
    date_label: &str,
    message_id: &str,
    days_ago: Option<i64>,
) -> String {
    let label = match days_ago {
        Some(days) => format!("{date_label} ({days} дн. назад)"),
        None => date_label.to_string(),
    };

    match message_id.parse::<i32>() {
        Ok(message_id) => format!(
            "{} (#<code>{}</code>)",
            Html::link(label, message_url(chat_id, message_id)).into_string(),
            message_id
        ),
        Err(_) => format!(
            "{} (#<code>{}</code>)",
            escape_html(date_label),
            escape_html(message_id)
        ),
    }
}

fn message_url(chat_id: i64, message_id: i32) -> String {
    let internal_chat_id = chat_id
        .to_string()
        .strip_prefix("-100")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| chat_id.abs().to_string());

    format!("https://t.me/c/{internal_chat_id}/{message_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_preview_falls_back_to_media() {
        assert_eq!(
            message_preview(
                None,
                MessageMediaPreview {
                    has_photo: true,
                    has_voice: true,
                    ..Default::default()
                },
            ),
            "медиа: фото, голосовое"
        );
    }

    #[test]
    fn converts_html_links_to_rich_markdown_links() {
        assert_eq!(
            html_to_rich_markdown("<a href=\"tg://user?id=42\">Миша</a>: <b>7</b>"),
            "[Миша](tg://user?id=42): **7**"
        );
    }
}
