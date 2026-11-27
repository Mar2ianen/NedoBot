use std::{str::FromStr, time::Duration};

use anyhow::Context;
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

/// Зеркалит LOLS spammers-full banlist в `lols_spam_users` инкрементальным diff-ом.
///
/// LOLS покрывает нашу форму спама на порядок лучше CAS (11/11 подтверждённых
/// спамеров против 0/8), поэтому зеркало локальное: lookup в baseline без
/// сетевой зависимости в момент аудита. Требуется только `DATABASE_URL`;
/// полный список (~10.8M user_id) качается не чаще раза в час (cron).
///
/// Синхронизация не переписывает таблицу целиком: новые id вставляются,
/// исчезнувшие удаляются anti-join-ом, а `last_seen_at` обновляется bounded
/// батчем. Temp-таблица анализируется перед diff-ом, иначе планировщик видит
/// пустую статистику и выбирает nested loop на миллионах строк. Сессионные
/// `statement_timeout` и `lock_timeout` не дают зависшему запросу пережить
/// клиента и копить блокировки следующих запусков.
const DEFAULT_BANLIST_URL: &str = "https://lols.bot/spam/banlist.txt";

/// Session-level advisory lock: одновременно работает только один sync.
const SYNC_ADVISORY_LOCK_KEY: i64 = 0x6c6f_6c73_5f73_796e;
const SYNC_STATEMENT_TIMEOUT_MS: u64 = 30 * 60 * 1000;
const SYNC_LOCK_TIMEOUT_MS: u64 = 30 * 1000;
const INSERT_CHUNK_SIZE: usize = 50_000;
/// Полный upsert каждые 15 минут раздувал heap и index; держим `last_seen_at`
/// приблизительным, обновляя не больше 100k самых старых строк за запуск.
const LAST_SEEN_REFRESH_BATCH: i64 = 100_000;
/// Ниже этого размера зеркала резкое сокращение banlist считаем сбоем загрузки.
const MIN_EXISTING_ROWS_FOR_SHRINK_GUARD: i64 = 1_000;

#[derive(Debug)]
struct Args {
    banlist_url: String,
    timeout_sec: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SyncStats {
    inserted: u64,
    removed: u64,
    refreshed: u64,
    total: i64,
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
    let user_ids = parse_banlist(&body);
    println!(
        "lols mirror: downloaded {} user ids from {}",
        user_ids.len(),
        args.banlist_url
    );
    if user_ids.is_empty() {
        anyhow::bail!("lols banlist is empty, refusing to swap the mirror");
    }
    let pool = connect(&database_url).await?;
    if !try_acquire_sync_lock(&pool).await? {
        println!("lols mirror: another sync is already running, skipping");
        return Ok(());
    }
    let stats = sync_mirror(&pool, &user_ids).await?;
    println!(
        "lols mirror: inserted={} removed={} refreshed={} total={}",
        stats.inserted, stats.removed, stats.refreshed, stats.total
    );
    Ok(())
}

fn parse_banlist(body: &str) -> Vec<i64> {
    let mut user_ids: Vec<i64> = body
        .lines()
        .filter_map(|line| line.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .collect();
    user_ids.sort_unstable();
    user_ids.dedup();
    user_ids
}

fn banlist_shrank(existing: i64, incoming: usize) -> bool {
    existing >= MIN_EXISTING_ROWS_FOR_SHRINK_GUARD && (incoming as i64) * 2 < existing
}

async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    // Temp-таблица живёт в сессии, поэтому строго одно соединение: пул на два
    // может разнести create/insert/diff по разным бэкендам.
    let options = PgConnectOptions::from_str(database_url)
        .context("invalid DATABASE_URL")?
        .application_name("sync_lols_banlist");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .context("lols mirror database connection")?;
    for (name, value) in [
        ("statement_timeout", SYNC_STATEMENT_TIMEOUT_MS.to_string()),
        ("lock_timeout", SYNC_LOCK_TIMEOUT_MS.to_string()),
    ] {
        sqlx::query("select set_config($1, $2, false)")
            .bind(name)
            .bind(value)
            .execute(&pool)
            .await
            .with_context(|| format!("set lols sync {name}"))?;
    }
    Ok(pool)
}

async fn try_acquire_sync_lock(pool: &PgPool) -> anyhow::Result<bool> {
    let acquired: bool = sqlx::query_scalar("select pg_try_advisory_lock($1)")
        .bind(SYNC_ADVISORY_LOCK_KEY)
        .fetch_one(pool)
        .await
        .context("acquire lols sync advisory lock")?;
    Ok(acquired)
}

async fn sync_mirror(pool: &PgPool, user_ids: &[i64]) -> anyhow::Result<SyncStats> {
    let existing: i64 = sqlx::query_scalar("select count(*) from lols_spam_users")
        .fetch_one(pool)
        .await
        .context("count lols mirror rows")?;
    if banlist_shrank(existing, user_ids.len()) {
        anyhow::bail!(
            "lols banlist shrank from {existing} to {} entries, refusing to replace the mirror",
            user_ids.len()
        );
    }

    let mut tx = pool
        .begin()
        .await
        .context("begin lols mirror transaction")?;
    sqlx::query(
        "create temporary table lols_spam_users_next (telegram_user_id bigint primary key) on commit drop",
    )
    .execute(&mut *tx)
    .await
    .context("create lols staging table")?;
    // COPY через unnest батчами: 10.8M строк одним INSERT не проходят.
    for chunk in user_ids.chunks(INSERT_CHUNK_SIZE) {
        sqlx::query(
            "insert into lols_spam_users_next (telegram_user_id) select * from unnest($1::bigint[])",
        )
        .bind(chunk)
        .execute(&mut *tx)
        .await
        .context("fill lols staging table")?;
    }
    // Без ANALYZE планировщик видит пустую temp-таблицу и выбирает nested loop
    // для anti-join: именно это превращало DELETE в многосуточный запрос.
    sqlx::query("analyze lols_spam_users_next")
        .execute(&mut *tx)
        .await
        .context("analyze lols staging table")?;
    let inserted = sqlx::query(
        r#"
        insert into lols_spam_users (telegram_user_id, first_seen_at, last_seen_at)
        select telegram_user_id, now(), now() from lols_spam_users_next
        on conflict (telegram_user_id) do nothing
        "#,
    )
    .execute(&mut *tx)
    .await
    .context("insert new lols mirror rows")?
    .rows_affected();
    let removed = sqlx::query(
        r#"
        delete from lols_spam_users l
        where not exists (
            select 1 from lols_spam_users_next n
            where n.telegram_user_id = l.telegram_user_id
        )
        "#,
    )
    .execute(&mut *tx)
    .await
    .context("delete stale lols mirror rows")?
    .rows_affected();
    let refreshed = sqlx::query(
        r#"
        update lols_spam_users
        set last_seen_at = now()
        where telegram_user_id in (
            select telegram_user_id
            from lols_spam_users
            where last_seen_at < now() - interval '24 hours'
            order by last_seen_at
            limit $1
        )
        "#,
    )
    .bind(LAST_SEEN_REFRESH_BATCH)
    .execute(&mut *tx)
    .await
    .context("refresh lols last_seen_at")?
    .rows_affected();
    let total: i64 = sqlx::query_scalar("select count(*) from lols_spam_users")
        .fetch_one(&mut *tx)
        .await
        .context("count lols mirror rows after sync")?;
    tx.commit()
        .await
        .context("commit lols mirror transaction")?;
    Ok(SyncStats {
        inserted,
        removed,
        refreshed,
        total,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_banlist_keeps_positive_sorted_unique_ids() {
        let parsed = parse_banlist(" 42\n-1\nabc\n42\n7\n0\n\n9\n");
        assert_eq!(parsed, vec![7, 9, 42]);
    }

    #[test]
    fn shrink_guard_rejects_only_large_truncated_banlists() {
        assert!(!banlist_shrank(0, 0));
        assert!(!banlist_shrank(999, 1));
        assert!(banlist_shrank(1_000, 499));
        assert!(!banlist_shrank(1_000, 500));
        assert!(!banlist_shrank(10_855_995, 10_855_000));
        assert!(banlist_shrank(10_855_995, 1_000_000));
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL and applied migrations"]
    async fn sync_is_incremental_idempotent_and_guarded() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
        let pool = connect(&database_url).await.unwrap();
        sqlx::query("truncate lols_spam_users")
            .execute(&pool)
            .await
            .unwrap();

        let first = sync_mirror(&pool, &[10, 20, 30]).await.unwrap();
        assert_eq!(
            first,
            SyncStats {
                inserted: 3,
                removed: 0,
                refreshed: 0,
                total: 3,
            }
        );
        let first_seen: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "select first_seen_at from lols_spam_users where telegram_user_id = 20",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let second = sync_mirror(&pool, &[20, 30, 40]).await.unwrap();
        assert_eq!(
            second,
            SyncStats {
                inserted: 1,
                removed: 1,
                refreshed: 0,
                total: 3,
            }
        );
        let ids: Vec<i64> =
            sqlx::query_scalar("select telegram_user_id from lols_spam_users order by 1")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(ids, vec![20, 30, 40]);
        let preserved: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "select first_seen_at from lols_spam_users where telegram_user_id = 20",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preserved, first_seen);

        let repeated = sync_mirror(&pool, &[20, 30, 40]).await.unwrap();
        assert_eq!(
            repeated,
            SyncStats {
                inserted: 0,
                removed: 0,
                refreshed: 0,
                total: 3,
            }
        );

        sqlx::query("truncate lols_spam_users")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "insert into lols_spam_users (telegram_user_id) select generate_series(1, 1000)",
        )
        .execute(&pool)
        .await
        .unwrap();
        let error = sync_mirror(&pool, &[1, 2]).await.unwrap_err();
        assert!(error.to_string().contains("shrank"), "{error}");
        let total: i64 = sqlx::query_scalar("select count(*) from lols_spam_users")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(total, 1_000);

        sqlx::query("truncate lols_spam_users")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires TEST_DATABASE_URL and applied migrations"]
    async fn advisory_lock_blocks_a_second_sync_session() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must be set by scripts/test.sh");
        let first = connect(&database_url).await.unwrap();
        let second = connect(&database_url).await.unwrap();
        assert!(try_acquire_sync_lock(&first).await.unwrap());
        assert!(!try_acquire_sync_lock(&second).await.unwrap());
    }
}
