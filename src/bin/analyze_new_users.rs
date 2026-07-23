use anyhow::{Context, bail};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::new_user_analysis::analyze_new_user_profile,
};

#[derive(Debug)]
struct Args {
    chat_id: Option<i64>,
    user_ids: Vec<i64>,
    limit: i64,
    max_messages: i64,
    include_analyzed: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = parse_args()?;
    let config = Config::from_env();
    let chat_id = args.chat_id.unwrap_or(config.discussion_chat_id);
    let pool = build_pool().await?;
    migrate(&pool).await?;

    let user_ids = match args.user_ids.is_empty() {
        true => {
            load_candidate_user_ids(
                &pool,
                chat_id,
                args.limit,
                args.max_messages,
                args.include_analyzed,
            )
            .await?
        }
        false => args.user_ids,
    };
    println!(
        "analyze new users: chat_id={} users={} max_messages={} include_analyzed={}",
        chat_id,
        user_ids.len(),
        args.max_messages,
        args.include_analyzed
    );

    let mut analyzed = 0usize;
    let mut failed = 0usize;
    for user_id in user_ids {
        match analyze_new_user_profile(&pool, chat_id, user_id).await {
            Ok(()) => analyzed += 1,
            Err(err) => {
                failed += 1;
                println!("failed user_id={user_id}: {err:#}");
            }
        }
    }

    println!("done: analyzed={analyzed} failed={failed}");
    Ok(())
}

async fn load_candidate_user_ids(
    pool: &sqlx::PgPool,
    chat_id: i64,
    limit: i64,
    max_messages: i64,
    include_analyzed: bool,
) -> anyhow::Result<Vec<i64>> {
    let rows = sqlx::query_as::<_, (i64,)>(
        r#"
        select cu.telegram_user_id
        from telegram_chat_users cu
        left join telegram_user_profiles p on p.telegram_user_id = cu.telegram_user_id
        left join telegram_new_user_profile_audits a
          on a.chat_id = cu.chat_id and a.telegram_user_id = cu.telegram_user_id
        where cu.chat_id = $1
          and cu.message_count <= $2
          and not coalesce(p.is_bot, false)
          and ($3 or a.telegram_user_id is null)
        order by
            cu.is_spammer desc,
            cu.first_seen_at desc nulls last,
            cu.telegram_user_id desc
        limit $4
        "#,
    )
    .bind(chat_id)
    .bind(max_messages)
    .bind(include_analyzed)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(user_id,)| user_id).collect())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut chat_id = None;
    let mut user_ids = Vec::new();
    let mut limit = 200i64;
    let mut max_messages = 5i64;
    let mut include_analyzed = false;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chat-id" => {
                chat_id = Some(
                    args.next()
                        .context("--chat-id requires value")?
                        .parse()
                        .context("invalid --chat-id")?,
                );
            }
            "--user-id" => user_ids.push(
                args.next()
                    .context("--user-id requires value")?
                    .parse()
                    .context("invalid --user-id")?,
            ),
            "--limit" => {
                limit = args
                    .next()
                    .context("--limit requires value")?
                    .parse()
                    .context("invalid --limit")?;
            }
            "--max-messages" => {
                max_messages = args
                    .next()
                    .context("--max-messages requires value")?
                    .parse()
                    .context("invalid --max-messages")?;
            }
            "--include-analyzed" => include_analyzed = true,
            "-h" | "--help" => {
                println!(
                    "Usage: analyze_new_users [--chat-id -100...] [--user-id ID]... [--limit 200] [--max-messages 5] [--include-analyzed]"
                );
                std::process::exit(0);
            }
            _ => bail!("unknown option: {arg}"),
        }
    }

    Ok(Args {
        chat_id,
        user_ids,
        limit,
        max_messages,
        include_analyzed,
    })
}
