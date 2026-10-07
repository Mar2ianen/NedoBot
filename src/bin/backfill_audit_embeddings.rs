#[path = "support/embedding_backfill.rs"]
mod embedding_backfill;

use anyhow::Context;
use embedding_backfill::embed_text_batch_at;
use sqlx::{PgPool, Row};
use tg_ai_bot_teloxide::features::memory::embedding::{
    EMBEDDINGGEMMA2_CLASSIFICATION_PREFIX, EMBEDDINGGEMMA2_MODEL_ID, pgvector_literal,
};

/// Заполняет новый корпус EmbeddingGemma 2 для будущих `spam_similarity`
/// проверок. История уже выполненного скоринга не пересчитывается.
///
/// Требуется только `DATABASE_URL` и `RAG_EMBEDDING_*`; полный `Config`
/// с Telegram/LLM-секретами не нужен, polling не запускается.
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
    let embedding_model = absent_or("RAG_EMBEDDING_MODEL", EMBEDDINGGEMMA2_MODEL_ID)?;
    if embedding_model != EMBEDDINGGEMMA2_MODEL_ID {
        anyhow::bail!("RAG_EMBEDDING_MODEL must match the pinned EmbeddingGemma 2 encoder");
    }
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
    let mut stored = 0usize;
    for chunk in rows.chunks(args.batch_size) {
        let texts: Vec<String> = chunk
            .iter()
            .map(|row| format!("{EMBEDDINGGEMMA2_CLASSIFICATION_PREFIX}{}", row.text))
            .collect();
        let inputs = texts.iter().map(String::as_str).collect::<Vec<_>>();
        let embeddings =
            embed_text_batch_at(&embedding_url, embedding_timeout_sec, &inputs).await?;
        for (row, embedding) in chunk.iter().zip(embeddings.iter()) {
            let literal = pgvector_literal(embedding)?;
            sqlx::query(
                "update telegram_new_user_profile_audits set first_message_embedding_gemma2 = $3::vector, first_message_embedding_gemma2_model = $4 where chat_id = $1 and telegram_user_id = $2 and first_message_embedding_gemma2 is null",
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
        where a.first_message_embedding_gemma2 is null
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

fn absent_or(key: &str, default: &str) -> anyhow::Result<String> {
    match std::env::var(key) {
        Ok(value) if value.trim().is_empty() => anyhow::bail!("{key} must not be empty"),
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Ok(default.to_string()),
        Err(error) => Err(error).with_context(|| format!("failed to read {key}")),
    }
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
