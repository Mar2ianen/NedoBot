use anyhow::{Context, bail};
use sqlx::PgPool;
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::first_message_spam::analyze_first_message,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let (chat_id, limit, only_spam, user_id) = parse_args()?;
    let config = Config::from_env();
    let chat_id = chat_id.unwrap_or(config.discussion_chat_id);
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let user_ids = match user_id {
        Some(user_id) => vec![user_id],
        None => candidates(&pool, chat_id, limit, only_spam).await?,
    };
    println!("first-message analysis: users={}", user_ids.len());
    for user_id in user_ids {
        match analyze_first_message(&pool, &config, chat_id, user_id).await {
            Ok(true) => println!("analyzed user_id={user_id}"),
            Ok(false) => println!("skipped user_id={user_id}"),
            Err(err) => println!("failed user_id={user_id}: {err:#}"),
        }
    }
    Ok(())
}

async fn candidates(
    pool: &PgPool,
    chat_id: i64,
    limit: i64,
    only_spam: bool,
) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>(
        r#"
        select a.telegram_user_id
        from telegram_new_user_profile_audits a
        join telegram_chat_users u on u.chat_id = a.chat_id and u.telegram_user_id = a.telegram_user_id
        where a.chat_id = $1 and a.first_message_analysis_at is null and a.first_message_text is not null
          and (not $2 or u.is_spammer)
        order by u.is_spammer desc, a.analyzed_at asc
        limit $3
        "#,
    )
    .bind(chat_id)
    .bind(only_spam)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

fn parse_args() -> anyhow::Result<(Option<i64>, i64, bool, Option<i64>)> {
    let mut chat_id = None;
    let mut limit = 100;
    let mut only_spam = false;
    let mut user_id = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chat-id" => {
                chat_id = Some(args.next().context("--chat-id requires value")?.parse()?)
            }
            "--limit" => limit = args.next().context("--limit requires value")?.parse()?,
            "--only-spam" => only_spam = true,
            "--user-id" => {
                user_id = Some(args.next().context("--user-id requires value")?.parse()?)
            }
            "-h" | "--help" => {
                println!(
                    "Usage: backfill_first_message_spam [--chat-id -100...] [--limit 100] [--only-spam] [--user-id ID]"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown option: {arg}"),
        }
    }
    Ok((chat_id, limit, only_spam, user_id))
}
