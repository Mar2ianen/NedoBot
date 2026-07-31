use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::types::chrono::Utc;
use teloxide::{
    payloads::GetUserProfilePhotosSetters,
    prelude::*,
    types::{ChatFullInfo, Message, PhotoSize, UserId, UserProfilePhotos},
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

    let personal_channel_future = fetch_personal_channel_messages(bot, user_id);
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
    chat: Option<&ChatFullInfo>,
    photos: Option<&UserProfilePhotos>,
    personal_channel: Option<&PersonalChannelData>,
    personal_channel_error: Option<String>,
) -> UserProfileDetails {
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
        username: chat.and_then(|chat| chat.username().map(str::to_owned)),
        first_name: chat.and_then(|chat| chat.first_name().map(str::to_owned)),
        last_name: chat.and_then(|chat| chat.last_name().map(str::to_owned)),
        bio: chat.and_then(|chat| chat.bio().map(str::to_owned)),
        small_photo_file_id: chat_photo.map(|photo| photo.small_file_id.to_string()),
        small_photo_file_unique_id: chat_photo.map(|photo| photo.small_file_unique_id.to_string()),
        big_photo_file_id: chat_photo.map(|photo| photo.big_file_id.to_string()),
        big_photo_file_unique_id: chat_photo.map(|photo| photo.big_file_unique_id.to_string()),
        profile_photo_file_id: profile_photo.map(|photo| photo.file.id.to_string()),
        profile_photo_file_unique_id: profile_photo.map(|photo| photo.file.unique_id.to_string()),
        profile_photo_width: profile_photo.map(photo_width),
        profile_photo_height: profile_photo.map(photo_height),
        profile_photo_count: photos.map(|photos| photos.total_count as i32),
        emoji_status_custom_emoji_id: chat
            .and_then(|chat| chat.emoji_status_custom_emoji_id.as_ref())
            .map(ToString::to_string),
        profile_accent_color_id: chat.and_then(|chat| chat.profile_accent_color_id.map(i16::from)),
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

async fn fetch_personal_channel_messages(
    bot: &Bot,
    user_id: UserId,
) -> anyhow::Result<PersonalChannelData> {
    let messages = bot.get_user_personal_chat_messages(user_id, 5).await?;
    let result = serde_json::to_value(&messages)?;
    let raw_json = json!({"ok": true, "result": result});

    Ok(build_personal_channel_data(messages, raw_json))
}

fn build_personal_channel_data(messages: Vec<Message>, raw_json: Value) -> PersonalChannelData {
    let first = messages.first();
    let last_text = first.and_then(message_text).map(str::to_owned);
    let has_adult_links = messages.iter().any(|message| {
        let text = message_text(message).unwrap_or_default().to_lowercase();
        text.contains("t.me/+")
            && (text.contains("хочешь увидеть")
                || text.contains("заходи")
                || text.contains("18+")
                || text.contains("приват")
                || text.contains("вход для своих"))
    });

    PersonalChannelData {
        chat_id: first.map(|message| message.chat.id.0),
        title: first.and_then(|message| message.chat.title().map(str::to_owned)),
        username: first.and_then(|message| message.chat.username().map(str::to_owned)),
        message_count: i32::try_from(messages.len()).unwrap_or(i32::MAX),
        last_message_id: first.map(|message| message.id.0),
        last_message_at: first.map(|message| message.date),
        last_text,
        has_adult_links,
        raw_json,
    }
}

fn message_text(message: &Message) -> Option<&str> {
    message.text().or_else(|| message.caption())
}

fn is_definitive_personal_channel_error(error: &str) -> bool {
    error.contains("USER_PERSONAL_CHANNEL_MISSING")
}
