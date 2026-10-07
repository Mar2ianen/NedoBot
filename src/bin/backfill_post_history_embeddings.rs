#[path = "support/embedding_backfill.rs"]
mod embedding_backfill;

use anyhow::Context;
use embedding_backfill::embed_text_batch_at;
use sqlx::{PgPool, Row};
use tg_ai_bot_teloxide::features::memory::embedding::{
    EMBEDDINGGEMMA2_MODEL_ID, EMBEDDINGGEMMA2_RAG_DOCUMENT_PREFIX, pgvector_literal,
};

#[derive(Debug)]
struct Args {
    limit: i64,
    batch_size: usize,
}

struct PendingRow {
    id: i64,
    text: String,
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
    let timeout_sec = optional_u64("RAG_EMBEDDING_TIMEOUT_SEC", 60)?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
        .context("post history embedding backfill database connection")?;
    let rows = load_pending(&pool, &args).await?;
    println!(
        "backfill post-history EmbeddingGemma 2: pending={} model={}",
        rows.len(),
        embedding_model
    );
    let mut stored = 0usize;
    for chunk in rows.chunks(args.batch_size) {
        let texts = chunk
            .iter()
            .map(|row| format!("{EMBEDDINGGEMMA2_RAG_DOCUMENT_PREFIX}{}", row.text))
            .collect::<Vec<_>>();
        let inputs = texts.iter().map(String::as_str).collect::<Vec<_>>();
        let embeddings = embed_text_batch_at(&embedding_url, timeout_sec, &inputs).await?;
        for (row, embedding) in chunk.iter().zip(embeddings.iter()) {
            let literal = pgvector_literal(embedding)?;
            let result = sqlx::query(
                "update post_history_entries set embedding_gemma2 = $2::vector, embedding_gemma2_model = $3 where id = $1 and status = 'ready' and embedding_gemma2 is null",
            )
            .bind(row.id)
            .bind(&literal)
            .bind(&embedding_model)
            .execute(&pool)
            .await?;
            stored += usize::try_from(result.rows_affected()).unwrap_or_default();
        }
        println!("backfill post-history EmbeddingGemma 2: stored={stored}");
    }
    println!("backfill post-history EmbeddingGemma 2: done stored={stored}");
    Ok(())
}

async fn load_pending(pool: &PgPool, args: &Args) -> anyhow::Result<Vec<PendingRow>> {
    let rows = sqlx::query(
        r#"
        select id, summary, entities, used_angle, external_fact
        from post_history_entries
        where status = 'ready' and embedding_gemma2 is null
        order by created_at, id
        limit $1
        "#,
    )
    .bind(args.limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let summary: String = row.get("summary");
            let entities: Vec<String> = row.get("entities");
            let used_angle: Option<String> = row.get("used_angle");
            let external_fact: Option<String> = row.get("external_fact");
            let mut parts = vec![summary];
            if !entities.is_empty() {
                parts.push(entities.join(", "));
            }
            parts.extend(used_angle);
            parts.extend(external_fact);
            PendingRow {
                id: row.get("id"),
                text: parts.join("\n"),
            }
        })
        .collect())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        limit: 1000,
        batch_size: 8,
    };
    let mut raw = std::env::args().skip(1).peekable();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--limit" => {
                args.limit = raw
                    .next()
                    .context("--limit requires a value")?
                    .parse()
                    .context("--limit must be an integer")?;
            }
            "--batch-size" => {
                args.batch_size = raw
                    .next()
                    .context("--batch-size requires a value")?
                    .parse()
                    .context("--batch-size must be an integer")?;
            }
            other => anyhow::bail!("unknown argument: {other}"),
        }
    }
    if args.limit <= 0 {
        anyhow::bail!("--limit must be positive");
    }
    if !(1..=16).contains(&args.batch_size) {
        anyhow::bail!("--batch-size must be between 1 and 16");
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
    match std::env::var(key) {
        Ok(value) => value
            .parse()
            .with_context(|| format!("{key} must be a positive integer")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error).with_context(|| format!("failed to read {key}")),
    }
}
