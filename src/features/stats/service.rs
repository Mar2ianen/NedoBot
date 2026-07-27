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
use sqlx::PgPool;
use teloxide::prelude::*;

use crate::config::Config;
use crate::db::telegram::refresh_chat_member_snapshot;
use crate::features::stats::repo;
use crate::features::stats::types::{
    AttractionMetrics, BotCommentStats, ChatStatsReportData, MessageMediaPreview, PeriodTopUser,
    TopMessageUser, TopMessagesReportData, TopReactedMessage, TopReactedReportData,
    UserPresentation, UserStatsReportData, UserTotals, display_name,
};
use crate::features::user_profiles::avatar::cache_profile_avatar;
use crate::features::user_profiles::service::refresh_profile;

const PERIOD_TOP_USERS_LIMIT: i64 = 8;
const BOT_COMMENTS_LIMIT: i64 = 5;
pub const HTML_TOP_LIMIT: i64 = 20;
pub const RICH_TOP_LIMIT: i64 = 30;
pub const USER_TOP_WORDS_LIMIT: i64 = 10;

pub async fn chat_stats_report_data(
    pool: &PgPool,
    config: &Config,
    period: crate::features::stats::types::StatsPeriod,
) -> anyhow::Result<ChatStatsReportData> {
    let chat_id = config.discussion_chat_id;
    let summary = repo::chat_stats_summary(pool, chat_id, period).await?;
    let attraction = repo::chat_attraction_metrics(pool, chat_id, period).await?;
    let top_users = repo::period_top_users(pool, chat_id, period, PERIOD_TOP_USERS_LIMIT)
        .await?
        .into_iter()
        .map(period_top_user)
        .collect();
    let bot_comments = repo::bot_comments_for_period(pool, chat_id, period, BOT_COMMENTS_LIMIT)
        .await?
        .into_iter()
        .map(|row| BotCommentStats {
            source_message_id: row.source_message_id,
            response: row.response,
            messages_30m: row.messages_30m,
            direct_replies: row.direct_replies,
            reactions: row.reactions,
        })
        .collect();

    Ok(ChatStatsReportData {
        period,
        summary,
        attraction: AttractionMetrics {
            messages_5m: attraction.messages_5m,
            messages_30m: attraction.messages_30m,
            users_30m: attraction.users_30m,
        },
        top_users,
        bot_comments,
    })
}

pub async fn top_messages_report_data(
    pool: &PgPool,
    config: &Config,
    limit: i64,
) -> anyhow::Result<TopMessagesReportData> {
    let users = repo::top_message_users(pool, config.discussion_chat_id, limit)
        .await?
        .into_iter()
        .map(|row| TopMessageUser {
            user: UserPresentation {
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
            },
            username: row.username,
            messages: row.messages,
            replies: row.replies,
            media: row.media,
            voices: row.voices,
            links: row.links,
            reactions_received: row.reactions_received,
        })
        .collect();
    Ok(TopMessagesReportData { users })
}

pub async fn top_reacted_report_data(
    pool: &PgPool,
    config: &Config,
    limit: i64,
) -> anyhow::Result<TopReactedReportData> {
    let messages = repo::top_reacted_messages(pool, config.discussion_chat_id, limit)
        .await?
        .into_iter()
        .map(|row| TopReactedMessage {
            message_id: row.message_id,
            user: UserPresentation {
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
            },
            username: row.username,
            text: row.text,
            media: MessageMediaPreview {
                has_photo: row.has_photo,
                has_video: row.has_video,
                has_document: row.has_document,
                has_audio: row.has_audio,
                has_voice: row.has_voice,
                has_sticker: row.has_sticker,
                has_animation: row.has_animation,
            },
            total_count: row.total_count,
        })
        .collect();
    Ok(TopReactedReportData { messages })
}

pub async fn user_stats_report_data(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    target: Option<&str>,
    reply_user_id: Option<i64>,
) -> anyhow::Result<Option<UserStatsReportData>> {
    let Some(user_id) = repo::resolve_user_id(pool, target, reply_user_id).await? else {
        return Ok(None);
    };
    let chat_id = config.discussion_chat_id;
    let profile = repo::user_profile(pool, user_id).await?;
    let member = repo::chat_member_snapshot(pool, chat_id, user_id).await?;
    let cached = repo::chat_user_stats(pool, chat_id, user_id).await?;
    let mut totals = user_totals(repo::user_totals(pool, chat_id, user_id).await?);
    let reactions_given = repo::user_reactions_given(pool, chat_id, user_id).await?;
    let reactions_received = repo::user_reactions_received(pool, chat_id, user_id).await?;
    let stop_words = USER_TOP_WORD_STOP_WORDS
        .iter()
        .map(|word| (*word).to_string())
        .collect::<Vec<_>>();
    let top_words =
        repo::user_top_words(pool, chat_id, user_id, &stop_words, USER_TOP_WORDS_LIMIT).await?;

    let user = user_presentation(user_id, profile.as_ref(), member.as_ref());
    let avatar_url = cached_profile_photo_url(
        bot,
        config,
        user_id,
        profile
            .as_ref()
            .and_then(|value| value.profile_photo_file_id.as_deref()),
        profile
            .as_ref()
            .and_then(|value| value.profile_photo_file_unique_id.as_deref()),
    )
    .await;
    if let Some(stats) = cached.as_ref() {
        totals.messages = totals.messages.max(stats.messages);
        totals.replies = totals.replies.max(stats.replies);
        totals.links = totals.links.max(stats.links);
        totals.media = totals.media.max(stats.media);
        totals.post_comments = totals.post_comments.max(stats.replies_to_channel_posts);
        totals.replies_to_bot = totals.replies_to_bot.max(stats.replies_to_bot);
        totals.voices = totals.voices.max(stats.voices);
    }
    let (
        first_seen_at,
        last_seen_at,
        first_message_id,
        last_message_id,
        first_seen_days_ago,
        last_seen_days_ago,
    ) = cached.as_ref().map_or_else(
        || {
            (
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                "нет данных".to_string(),
                None,
                None,
            )
        },
        |stats| {
            (
                stats
                    .first_seen_at
                    .clone()
                    .unwrap_or_else(|| "нет данных".to_string()),
                stats
                    .last_seen_at
                    .clone()
                    .unwrap_or_else(|| "нет данных".to_string()),
                stats
                    .first_message_id
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "нет данных".to_string()),
                stats
                    .last_message_id
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "нет данных".to_string()),
                stats.first_seen_days_ago,
                stats.last_seen_days_ago,
            )
        },
    );

    Ok(Some(UserStatsReportData {
        username: profile.as_ref().and_then(|value| value.username.clone()),
        bio: profile.as_ref().and_then(|value| value.bio.clone()),
        avatar_url,
        observed_at: member.as_ref().and_then(|value| value.observed_at.clone()),
        written_tag: member.as_ref().and_then(|value| value.written_tag.clone()),
        user,
        first_seen_at,
        last_seen_at,
        first_message_id,
        last_message_id,
        first_seen_days_ago,
        last_seen_days_ago,
        totals,
        reactions_given,
        reactions_received,
        top_words,
    }))
}

pub async fn refresh_user_profile(
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

pub async fn refresh_top_message_users(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
) {
    refresh_ranked_users(
        bot,
        pool,
        config,
        repo::top_message_user_ids(pool, config.discussion_chat_id, RICH_TOP_LIMIT).await,
    )
    .await;
}

pub async fn refresh_top_reacted_users(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
) {
    refresh_ranked_users(
        bot,
        pool,
        config,
        repo::top_reacted_user_ids(pool, config.discussion_chat_id, RICH_TOP_LIMIT).await,
    )
    .await;
}

async fn refresh_ranked_users(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    user_ids: anyhow::Result<Vec<i64>>,
) {
    let user_ids = match user_ids {
        Ok(user_ids) => user_ids,
        Err(err) => {
            tracing::warn!(%err, "failed to load ranked users for refresh");
            return;
        }
    };
    for user_id in user_ids {
        if let Err(err) = refresh_chat_member_snapshot(bot, pool, config, user_id).await {
            tracing::debug!(%err, user_id, "failed to refresh ranked user from Telegram");
        }
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
    let avatar = match cache_profile_avatar(
        bot.inner(),
        &config.static_files_dir,
        user_id,
        file_id,
        unique_id,
    )
    .await
    {
        Ok(Some(avatar)) => avatar,
        Ok(None) => return None,
        Err(err) => {
            tracing::debug!(%err, user_id, "failed to cache profile photo");
            return None;
        }
    };
    Some(format!(
        "{public_base_url}/tg-ai-bot-static/avatars/{}",
        avatar.filename()
    ))
}

fn period_top_user(row: repo::PeriodTopUser) -> PeriodTopUser {
    PeriodTopUser {
        user: UserPresentation {
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
        },
        username: row.username,
        messages: row.messages,
        replies: row.replies,
        links: row.links,
        media: row.media,
    }
}

fn user_presentation(
    user_id: i64,
    profile: Option<&repo::UserProfile>,
    member: Option<&repo::ChatMemberSnapshot>,
) -> UserPresentation {
    UserPresentation {
        user_id,
        display_name: profile.map_or_else(
            || user_id.to_string(),
            |profile| {
                display_name(
                    profile.username.as_deref(),
                    profile.first_name.as_deref(),
                    profile.last_name.as_deref(),
                    user_id,
                )
            },
        ),
        is_bot: profile.is_some_and(|profile| profile.is_bot),
        status: member.map(|member| member.status.clone()),
        is_admin: member.is_some_and(|member| member.is_admin),
        is_present: member.map(|member| member.is_present),
    }
}

fn user_totals(totals: repo::UserTotals) -> UserTotals {
    UserTotals {
        messages: totals.messages,
        replies: totals.replies,
        links: totals.links,
        media: totals.media,
        post_comments: totals.post_comments,
        replies_to_bot: totals.replies_to_bot,
        active_days: totals.active_days,
        voices: totals.voices,
    }
}
