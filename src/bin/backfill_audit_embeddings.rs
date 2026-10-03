use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use tg_ai_bot_teloxide::features::memory::embedding::pgvector_literal;

/// Заполняет пустой корпус `first_message_embedding` для будущих
/// `spam_similarity`-проверок. История скоринга не пересчитывается:
/// эмбеддинги обслуживают только новые аудиты.
///
/// Требуется только `DATABASE_URL` и `RAG_EMBEDDING_*`; полный `Config`
/// с Telegram/LLM-секретами не нужен, polling не запускается.
#[derive(Debug, Serialize)]
struct EmbedBatchRequest<'a> {
    inputs: &'a [&'a str],
    truncate: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum EmbedBatchResponse {
    Batch(Vec<Vec<f32>>),
    // TEI иногда отвечает одиночным вектором на batch из одного входа.
    #[allow(dead_code)]
    Single(Vec<f32>),
}

#[derive(Debug)]
struct Args {
    chat_id: Option<i64>,
    limit: i64,
    only_spammers: bool,
    batch_size: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = parse_args()?;
    let database_url = required_env("DATABASE_URL")?;
    let embedding_url = required_env("RAG_EMBEDDING_URL")?;
    let embedding_model = absent_or("RAG_EMBEDDING_MODEL", "cointegrated/rubert-tiny2");
    let embedding_timeout_sec = optional_u64("RAG_EMBEDDING_TIMEOUT_SEC", 10)?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .context("backfill embedding database connection")?;
    let rows = load_pending(&pool, &args).await?;
    println!(
        "backfill audit embeddings: pending={} chat_id={:?} only_spammers={} model={}",
        rows.len(),
        args.chat_id,
        args.only_spammers,
        embedding_model,
    );
    if rows.is_empty() {
        return Ok(());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(embedding_timeout_sec))
        .build()
        .context("backfill embedding http client")?;
    let mut stored = 0usize;
    for chunk in rows.chunks(args.batch_size) {
        let texts: Vec<&str> = chunk.iter().map(|row| row.text.as_str()).collect();
        let embeddings = embed_batch(&client, &embedding_url, &texts).await?;
        for (row, embedding) in chunk.iter().zip(embeddings.iter()) {
            let literal = pgvector_literal(embedding)?;
            sqlx::query(
                "update telegram_new_user_profile_audits set first_message_embedding = $3::vector, first_message_embedding_model = $4 where chat_id = $1 and telegram_user_id = $2 and first_message_embedding is null",
            )
            .bind(row.chat_id)
            .bind(row.telegram_user_id)
            .bind(&literal)
            .bind(&embedding_model)
            .execute(&pool)
            .await?;
            stored += 1;
        }
        println!("backfill audit embeddings: stored={stored}");
    }
    println!("backfill audit embeddings: done stored={stored}");
    Ok(())
}

struct PendingRow {
    chat_id: i64,
    telegram_user_id: i64,
    text: String,
}

async fn load_pending(pool: &PgPool, args: &Args) -> anyhow::Result<Vec<PendingRow>> {
    let rows = sqlx::query(
        r#"
        select a.chat_id, a.telegram_user_id, a.first_message_text
        from telegram_new_user_profile_audits a
        left join telegram_chat_users u
          on u.chat_id = a.chat_id and u.telegram_user_id = a.telegram_user_id
        where a.first_message_embedding is null
          and nullif(trim(coalesce(a.first_message_text, '')), '') is not null
          and ($1::bigint is null or a.chat_id = $1)
          and (not $3 or coalesce(u.is_spammer, false))
        order by coalesce(u.is_spammer, false) desc, a.analyzed_at desc
        limit $2
        "#,
    )
    .bind(args.chat_id)
    .bind(args.limit)
    .bind(args.only_spammers)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let text: Option<String> = row.get("first_message_text");
            Some(PendingRow {
                chat_id: row.get("chat_id"),
                telegram_user_id: row.get("telegram_user_id"),
                text: text?,
            })
        })
        .collect())
}

async fn embed_batch(
    client: &reqwest::Client,
    embedding_url: &str,
    texts: &[&str],
) -> anyhow::Result<Vec<Vec<f32>>> {
    let response = client
        .post(format!("{}/embed", embedding_url.trim_end_matches('/')))
        .json(&EmbedBatchRequest {
            inputs: texts,
            truncate: true,
        })
        .send()
        .await
        .context("backfill embedding request")?
        .error_for_status()
        .context("backfill embedding status")?
        .json::<EmbedBatchResponse>()
        .await
        .context("backfill embedding body")?;
    match response {
        EmbedBatchResponse::Batch(rows) if rows.len() == texts.len() => Ok(rows),
        EmbedBatchResponse::Batch(rows) => anyhow::bail!(
            "embedding service returned {} rows for {} inputs",
            rows.len(),
            texts.len()
        ),
        EmbedBatchResponse::Single(_) => {
            anyhow::bail!("embedding service returned one row for batch input")
        }
    }
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        chat_id: None,
        limit: 500,
        only_spammers: true,
        batch_size: 16,
    };
    let mut raw = std::env::args().skip(1).peekable();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--chat-id" => {
                let value = raw.next().context("--chat-id requires a value")?;
                args.chat_id = Some(value.parse().context("--chat-id must be an integer")?);
            }
            "--limit" => {
                let value = raw.next().context("--limit requires a value")?;
                args.limit = value.parse().context("--limit must be an integer")?;
            }
            "--all" => args.only_spammers = false,
            "--batch-size" => {
                let value = raw.next().context("--batch-size requires a value")?;
                args.batch_size = value.parse().context("--batch-size must be an integer")?;
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    if args.limit <= 0 {
        anyhow::bail!("--limit must be positive");
    }
    if args.batch_size == 0 {
        anyhow::bail!("--batch-size must be positive");
    }
    Ok(args)
}

fn required_env(key: &str) -> anyhow::Result<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .with_context(|| format!("{key} must be set"))
}

fn absent_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn optional_u64(key: &str, default: u64) -> anyhow::Result<u64> {
    match std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        None => Ok(default),
        Some(value) => value
            .parse()
            .with_context(|| format!("{key} must be a positive integer")),
    }
}
