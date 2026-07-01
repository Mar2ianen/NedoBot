use std::time::Duration;

use anyhow::{Context, bail};
use serde_json::json;
use sqlx::PgPool;
use teloxide::{
    payloads::GetUserProfilePhotosSetters,
    prelude::*,
    types::{Chat, ChatKind, PhotoSize, UserId, UserProfilePhotos},
};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{
        build_pool, migrate,
        telegram::{
            UserProfileDetails, mark_user_profile_refresh_error, update_user_profile_details,
        },
    },
};
use tokio::time::sleep;

#[derive(Debug)]
struct Args {
    chat_id: Option<i64>,
    limit: i64,
    sleep_ms: u64,
    all: bool,
    only_spammers: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let config = Config::from_env();
    let chat_id = args.chat_id.unwrap_or(config.discussion_chat_id);
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let bot = Bot::from_env();

    let user_ids = load_user_ids(&pool, chat_id, &args).await?;
    println!(
        "refresh user profiles: chat_id={} users={} sleep_ms={} mode={}",
        chat_id,
        user_ids.len(),
        args.sleep_ms,
        if args.only_spammers {
            "spammers"
        } else if args.all {
            "all"
        } else {
            "missing"
        }
    );

    let mut refreshed = 0usize;
    let mut failed = 0usize;
    for (index, user_id) in user_ids.iter().copied().enumerate() {
        match refresh_profile(&bot, &pool, user_id).await {
            Ok(()) => refreshed += 1,
            Err(err) => {
                failed += 1;
                let message = err.to_string();
                mark_user_profile_refresh_error(&pool, user_id, &message).await?;
                println!("failed user_id={user_id}: {err:#}");
            }
        }

        if (index + 1) % 50 == 0 || index + 1 == user_ids.len() {
            println!(
                "progress: {}/{} refreshed={} failed={}",
                index + 1,
                user_ids.len(),
                refreshed,
                failed
            );
        }

        sleep(Duration::from_millis(args.sleep_ms)).await;
    }

    println!("done: refreshed={refreshed} failed={failed}");
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut chat_id = None;
    let mut limit = 200i64;
    let mut sleep_ms = 100u64;
    let mut all = false;
    let mut only_spammers = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--all" => all = true,
            "--only-spammers" => only_spammers = true,
            "--chat-id" => {
                chat_id = Some(
                    args.next()
                        .context("--chat-id requires value")?
                        .parse()
                        .context("invalid --chat-id")?,
                );
            }
            "--limit" => {
                limit = args
                    .next()
                    .context("--limit requires value")?
                    .parse()
                    .context("invalid --limit")?;
            }
            "--sleep-ms" => {
                sleep_ms = args
                    .next()
                    .context("--sleep-ms requires value")?
                    .parse()
                    .context("invalid --sleep-ms")?;
            }
            "-h" | "--help" => {
                println!(
                    "Usage: refresh_user_profiles [--all|--only-spammers] [--chat-id -100...] [--limit 200] [--sleep-ms 100]"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown option: {arg}"),
        }
    }

    Ok(Args {
        chat_id,
        limit,
        sleep_ms,
        all,
        only_spammers,
    })
}

async fn load_user_ids(pool: &PgPool, chat_id: i64, args: &Args) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>(
        r#"
        select cu.telegram_user_id
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        where cu.chat_id = $1
          and ($2 or p.profile_refreshed_at is null)
          and (not $3 or cu.is_spammer)
        order by
            cu.is_spammer desc,
            p.profile_refreshed_at asc nulls first,
            cu.last_seen_at desc nulls last
        limit $4
        "#,
    )
    .bind(chat_id)
    .bind(args.all)
    .bind(args.only_spammers)
    .bind(args.limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

async fn refresh_profile(bot: &Bot, pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    let user_id_u64 = u64::try_from(user_id).context("negative user id")?;
    let user_id = UserId(user_id_u64);

    let chat_result = bot.get_chat(ChatId(user_id.0 as i64)).await;
    let photos_result = bot.get_user_profile_photos(user_id).limit(1).await;

    let chat = chat_result.as_ref().ok();
    let photos = photos_result.as_ref().ok();
    if chat.is_none() && photos.is_none() {
        let chat_error = chat_result.err().map(|err| err.to_string());
        let photos_error = photos_result.err().map(|err| err.to_string());
        bail!(
            "getChat and getUserProfilePhotos failed: chat={:?}, photos={:?}",
            chat_error,
            photos_error
        );
    }

    let details = build_details(user_id.0 as i64, chat, photos);
    update_user_profile_details(pool, details).await?;
    Ok(())
}

fn build_details(
    telegram_user_id: i64,
    chat: Option<&Chat>,
    photos: Option<&UserProfilePhotos>,
) -> UserProfileDetails {
    let private = chat.and_then(|chat| match &chat.kind {
        ChatKind::Private(private) => Some(private),
        ChatKind::Public(_) => None,
    });
    let chat_photo = chat.and_then(|chat| chat.photo.as_ref());
    let profile_photo = photos
        .and_then(|photos| photos.photos.first())
        .and_then(|sizes| sizes.iter().max_by_key(|photo| photo.width * photo.height));

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
        raw_json: json!({
            "chat": chat,
            "profile_photos": photos,
        }),
    }
}

fn photo_width(photo: &PhotoSize) -> i32 {
    i32::try_from(photo.width).unwrap_or(i32::MAX)
}

fn photo_height(photo: &PhotoSize) -> i32 {
    i32::try_from(photo.height).unwrap_or(i32::MAX)
}
