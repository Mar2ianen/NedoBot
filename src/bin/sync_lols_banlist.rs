use std::time::Duration;

use anyhow::Context;

/// Зеркалит LOLS spammers-full banlist в `lols_spam_users` через temp swap.
///
/// LOLS покрывает нашу форму спама на порядок лучше CAS (11/11 подтверждённых
/// спамеров против 0/8), поэтому зеркало локальное: lookup в baseline без
/// сетевой зависимости в момент аудита. Требуется только `DATABASE_URL`;
/// полный список (~40МБ, ~3.6M user_id) качается не чаще раза в час (cron).
const DEFAULT_BANLIST_URL: &str = "https://lols.bot/spam/banlist.txt";

#[derive(Debug)]
struct Args {
    banlist_url: String,
    timeout_sec: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = parse_args()?;
    let database_url = std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .context("DATABASE_URL must be set")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(args.timeout_sec))
        .build()
        .context("lols mirror http client")?;
    let body = client
        .get(&args.banlist_url)
        .send()
        .await
        .context("lols banlist download")?
        .error_for_status()
        .context("lols banlist status")?
        .text()
        .await
        .context("lols banlist body")?;
    let mut user_ids: Vec<i64> = body
        .lines()
        .filter_map(|line| line.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .collect();
    user_ids.sort_unstable();
    user_ids.dedup();
    println!(
        "lols mirror: downloaded {} user ids from {}",
        user_ids.len(),
        args.banlist_url
    );
    if user_ids.is_empty() {
        anyhow::bail!("lols banlist is empty, refusing to swap the mirror");
    }
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .context("lols mirror database connection")?;
    let mut tx = pool.begin().await?;
    sqlx::query("create temporary table lols_spam_users_next (telegram_user_id bigint primary key) on commit drop")
        .execute(&mut *tx)
        .await?;
    // COPY через unnest батчами: 3.6M строк одним INSERT невозможны.
    for chunk in user_ids.chunks(50_000) {
        sqlx::query("insert into lols_spam_users_next (telegram_user_id) select * from unnest($1::bigint[]) on conflict do nothing")
            .bind(chunk)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        r#"
        insert into lols_spam_users (telegram_user_id, first_seen_at, last_seen_at)
        select telegram_user_id, now(), now() from lols_spam_users_next
        on conflict (telegram_user_id) do update set last_seen_at = now()
        "#,
    )
    .execute(&mut *tx)
    .await?;
    let removed = sqlx::query(
        "delete from lols_spam_users where telegram_user_id not in (select telegram_user_id from lols_spam_users_next)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    let total: (i64,) = sqlx::query_as("select count(*) from lols_spam_users")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    println!("lols mirror: total={} removed={}", total.0, removed);
    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        banlist_url: DEFAULT_BANLIST_URL.to_string(),
        timeout_sec: 300,
    };
    let mut raw = std::env::args().skip(1).peekable();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--banlist-url" => {
                args.banlist_url = raw.next().context("--banlist-url requires a value")?;
            }
            "--timeout-sec" => {
                let value: u64 = raw
                    .next()
                    .context("--timeout-sec requires a value")?
                    .parse()?;
                args.timeout_sec = value;
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    if args.timeout_sec == 0 {
        anyhow::bail!("--timeout-sec must be positive");
    }
    Ok(args)
}
