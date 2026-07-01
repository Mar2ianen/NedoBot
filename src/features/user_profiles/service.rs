use std::time::Duration;

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::types::chrono::Utc;
use teloxide::{
    payloads::GetUserProfilePhotosSetters,
    prelude::*,
    types::{Chat, ChatKind, PhotoSize, UserId, UserProfilePhotos},
};
use tokio::time::sleep;

use crate::db::telegram::{
    UserProfileDetails, mark_user_profile_refresh_error, update_user_profile_details,
};

#[allow(dead_code)]
pub struct RefreshUserProfilesQuery {
    pub chat_id: i64,
    pub limit: i64,
    pub include_refreshed: bool,
    pub only_spammers: bool,
}

#[allow(dead_code)]
pub struct ProfileRefreshStats {
    pub attempted: usize,
    pub refreshed: usize,
    pub failed: usize,
}

#[allow(dead_code)]
pub async fn load_user_ids(
    pool: &PgPool,
    query: &RefreshUserProfilesQuery,
) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>(
        r#"
        select cu.telegram_user_id
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        where cu.chat_id = $1
          and (
              $2
              or p.profile_refreshed_at is null
              or p.personal_channel_refreshed_at is null
          )
          and (not $3 or cu.is_spammer)
          and not coalesce(p.is_bot, false)
        order by
            cu.is_spammer desc,
            p.profile_refreshed_at asc nulls first,
            cu.last_seen_at desc nulls last
        limit $4
        "#,
    )
    .bind(query.chat_id)
    .bind(query.include_refreshed)
    .bind(query.only_spammers)
    .bind(query.limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

#[allow(dead_code)]
pub async fn refresh_profiles(
    bot: &Bot,
    pool: &PgPool,
    user_ids: &[i64],
    sleep_ms: u64,
) -> ProfileRefreshStats {
    let mut stats = ProfileRefreshStats {
        attempted: user_ids.len(),
        refreshed: 0,
        failed: 0,
    };

    for user_id in user_ids.iter().copied() {
        match refresh_profile(bot, pool, user_id).await {
            Ok(()) => stats.refreshed += 1,
            Err(err) => {
                stats.failed += 1;
                let message = err.to_string();
                if let Err(save_err) =
                    mark_user_profile_refresh_error(pool, user_id, &message).await
                {
                    tracing::warn!(%save_err, user_id, "failed to save profile refresh error");
                }
                tracing::debug!(%err, user_id, "failed to refresh user profile");
            }
        }

        sleep(Duration::from_millis(sleep_ms)).await;
    }

    stats
}

pub async fn refresh_profile(bot: &Bot, pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    let user_id_u64 = u64::try_from(user_id).context("negative user id")?;
    let user_id = UserId(user_id_u64);

    let personal_channel_future = fetch_personal_channel_messages(user_id.0 as i64);
    let (chat_result, photos_result, personal_channel_result) = tokio::join!(
        bot.get_chat(ChatId(user_id.0 as i64)),
        bot.get_user_profile_photos(user_id).limit(1),
        personal_channel_future,
    );

    let chat = chat_result.as_ref().ok();
    let photos = photos_result.as_ref().ok();
    let personal_channel = personal_channel_result.as_ref().ok();
    let personal_channel_error = personal_channel_result
        .as_ref()
        .err()
        .map(|err| err.to_string());
    if chat.is_none() && photos.is_none() && personal_channel.is_none() {
        let chat_error = chat_result.err().map(|err| err.to_string());
        let photos_error = photos_result.err().map(|err| err.to_string());
        bail!(
            "profile API calls failed: chat={:?}, photos={:?}, personal_channel={:?}",
            chat_error,
            photos_error,
            personal_channel_error
        );
    }

    let details = build_details(
        user_id.0 as i64,
        chat,
        photos,
        personal_channel,
        personal_channel_error,
    );
    update_user_profile_details(pool, details).await?;
    Ok(())
}

fn build_details(
    telegram_user_id: i64,
    chat: Option<&Chat>,
    photos: Option<&UserProfilePhotos>,
    personal_channel: Option<&PersonalChannelData>,
    personal_channel_error: Option<String>,
) -> UserProfileDetails {
    let private = chat.and_then(|chat| match &chat.kind {
        ChatKind::Private(private) => Some(private),
        ChatKind::Public(_) => None,
    });
    let chat_photo = chat.and_then(|chat| chat.photo.as_ref());
    let profile_photo = photos
        .and_then(|photos| photos.photos.first())
        .and_then(|sizes| sizes.iter().max_by_key(|photo| photo.width * photo.height));
    let personal_channel_refreshed_at = if personal_channel.is_some()
        || personal_channel_error
            .as_deref()
            .is_some_and(is_definitive_personal_channel_error)
    {
        Some(Utc::now())
    } else {
        None
    };

    UserProfileDetails {
        telegram_user_id,
        username: private.and_then(|private| private.username.clone()),
        first_name: private.and_then(|private| private.first_name.clone()),
        last_name: private.and_then(|private| private.last_name.clone()),
        bio: private.and_then(|private| private.bio.clone()),
        small_photo_file_id: chat_photo.map(|photo| photo.small_file_id.clone()),
        small_photo_file_unique_id: chat_photo.map(|photo| photo.small_file_unique_id.clone()),
        big_photo_file_id: chat_photo.map(|photo| photo.big_file_id.clone()),
        big_photo_file_unique_id: chat_photo.map(|photo| photo.big_file_unique_id.clone()),
        profile_photo_file_id: profile_photo.map(|photo| photo.file.id.clone()),
        profile_photo_file_unique_id: profile_photo.map(|photo| photo.file.unique_id.clone()),
        profile_photo_width: profile_photo.map(photo_width),
        profile_photo_height: profile_photo.map(photo_height),
        profile_photo_count: photos.map(|photos| photos.total_count as i32),
        emoji_status_custom_emoji_id: chat
            .and_then(|chat| chat.chat_full_info.emoji_status_custom_emoji_id.clone()),
        profile_accent_color_id: chat
            .and_then(|chat| chat.chat_full_info.profile_accent_color_id.map(i16::from)),
        personal_channel_chat_id: personal_channel.and_then(|channel| channel.chat_id),
        personal_channel_title: personal_channel.and_then(|channel| channel.title.clone()),
        personal_channel_username: personal_channel.and_then(|channel| channel.username.clone()),
        personal_channel_message_count: personal_channel.map(|channel| channel.message_count),
        personal_channel_last_message_id: personal_channel
            .and_then(|channel| channel.last_message_id),
        personal_channel_last_message_at: personal_channel
            .and_then(|channel| channel.last_message_at),
        personal_channel_last_text: personal_channel.and_then(|channel| channel.last_text.clone()),
        personal_channel_has_adult_links: personal_channel
            .is_some_and(|channel| channel.has_adult_links),
        personal_channel_raw_json: personal_channel.map(|channel| channel.raw_json.clone()),
        personal_channel_refreshed_at,
        personal_channel_fetch_error: personal_channel_error,
        raw_json: json!({
            "chat": chat,
            "profile_photos": photos,
            "personal_channel": personal_channel.map(|channel| &channel.raw_json),
        }),
    }
}

fn photo_width(photo: &PhotoSize) -> i32 {
    i32::try_from(photo.width).unwrap_or(i32::MAX)
}

fn photo_height(photo: &PhotoSize) -> i32 {
    i32::try_from(photo.height).unwrap_or(i32::MAX)
}

struct PersonalChannelData {
    chat_id: Option<i64>,
    title: Option<String>,
    username: Option<String>,
    message_count: i32,
    last_message_id: Option<i32>,
    last_message_at: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>,
    last_text: Option<String>,
    has_adult_links: bool,
    raw_json: Value,
}

#[derive(Deserialize)]
struct TelegramApiResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct PersonalChannelMessage {
    message_id: i32,
    date: i64,
    chat: PersonalChannelChat,
    text: Option<String>,
    caption: Option<String>,
}

#[derive(Deserialize)]
struct PersonalChannelChat {
    id: i64,
    title: Option<String>,
    username: Option<String>,
}

async fn fetch_personal_channel_messages(user_id: i64) -> anyhow::Result<PersonalChannelData> {
    let token = std::env::var("TELOXIDE_TOKEN").or_else(|_| std::env::var("BOT_TOKEN"))?;
    let url = format!("https://api.telegram.org/bot{token}/getUserPersonalChatMessages");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;
    let response = client
        .post(url)
        .json(&json!({
            "user_id": user_id,
            "limit": 5,
        }))
        .send()
        .await?;

    let raw_json: Value = response.json().await?;
    let api: TelegramApiResponse<Vec<PersonalChannelMessage>> =
        serde_json::from_value(raw_json.clone())?;

    if !api.ok {
        anyhow::bail!(
            "{}",
            api.description
                .unwrap_or_else(|| "getUserPersonalChatMessages failed".to_string())
        );
    }

    Ok(build_personal_channel_data(
        api.result.unwrap_or_default(),
        raw_json,
    ))
}

fn build_personal_channel_data(
    messages: Vec<PersonalChannelMessage>,
    raw_json: Value,
) -> PersonalChannelData {
    let first = messages.first();
    let last_text = first.and_then(|message| message.text.clone().or(message.caption.clone()));
    let has_adult_links = messages.iter().any(|message| {
        let text = message
            .text
            .as_deref()
            .or(message.caption.as_deref())
            .unwrap_or_default()
            .to_lowercase();
        text.contains("t.me/+")
            && (text.contains("хочешь увидеть")
                || text.contains("заходи")
                || text.contains("18+")
                || text.contains("приват")
                || text.contains("вход для своих"))
    });

    PersonalChannelData {
        chat_id: first.map(|message| message.chat.id),
        title: first.and_then(|message| message.chat.title.clone()),
        username: first.and_then(|message| message.chat.username.clone()),
        message_count: i32::try_from(messages.len()).unwrap_or(i32::MAX),
        last_message_id: first.map(|message| message.message_id),
        last_message_at: first
            .and_then(|message| sqlx::types::chrono::DateTime::from_timestamp(message.date, 0)),
        last_text,
        has_adult_links,
        raw_json,
    }
}

fn is_definitive_personal_channel_error(error: &str) -> bool {
    error.contains("USER_PERSONAL_CHANNEL_MISSING")
}
