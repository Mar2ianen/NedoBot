//! Batch adjudication writer for review queues.
//!
//! Reads a local TSV (no header): `telegram_user_id \t verdict \t subtype \t reason`.
//! `verdict` is `spam` or `ham`. Every row goes through the same writer the
//! review buttons use (`record_spam` / `record_not_spam`), so user flags,
//! message stamps and label events stay consistent. Raw SQL batches are
//! banned for this: they desync `is_spammer` from `spam_label_events`.
//!
//! The file may contain private message texts; never commit it.
//!
//! ```sh
//! cargo run --bin apply_review_labels -- --chat-id -1001932061163 \
//!   --operator-id 5939287960 --input /tmp/opencode/label_decisions.tsv --dry-run
//! ```

use std::path::PathBuf;

use sqlx::PgPool;

use tg_ai_bot_teloxide::config::Config;
use tg_ai_bot_teloxide::db::build_pool;
use tg_ai_bot_teloxide::features::labels::{LabelSource, SpamLabel, record_not_spam, record_spam};

struct Decision {
    user_id: i64,
    spam: bool,
    subtype: String,
    reason: String,
}

fn parse_tsv(path: &PathBuf) -> anyhow::Result<Vec<Decision>> {
    let mut decisions = Vec::new();
    for (index, line) in std::fs::read_to_string(path)?.lines().enumerate() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split('\t');
        let (user_id, verdict, subtype, reason) =
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some(user_id), Some(verdict), Some(subtype), Some(reason)) => {
                    (user_id, verdict, subtype, reason)
                }
                _ => anyhow::bail!(
                    "line {}: want uid \\t verdict \\t subtype \\t reason",
                    index + 1
                ),
            };
        if parts.next().is_some() {
            anyhow::bail!("line {}: too many columns", index + 1);
        }
        let spam = match verdict.trim() {
            "spam" => true,
            "ham" => false,
            other => anyhow::bail!(
                "line {}: verdict must be spam|ham, got {other:?}",
                index + 1
            ),
        };
        decisions.push(Decision {
            user_id: user_id
                .trim()
                .parse()
                .map_err(|_| anyhow::anyhow!("line {}: bad user id", index + 1))?,
            spam,
            subtype: subtype.trim().to_string(),
            reason: reason.trim().to_string(),
        });
    }
    if decisions.is_empty() {
        anyhow::bail!("no decisions in input");
    }
    Ok(decisions)
}

async fn apply(
    pool: &PgPool,
    chat_id: i64,
    operator_id: i64,
    decisions: &[Decision],
    avatar_embeddings_enabled: bool,
) -> anyhow::Result<()> {
    for decision in decisions {
        if decision.spam {
            record_spam(
                pool,
                chat_id,
                decision.user_id,
                &SpamLabel {
                    subtype: if decision.subtype.is_empty() {
                        "owner_review_queue".to_string()
                    } else {
                        decision.subtype.clone()
                    },
                    source: LabelSource::OwnerManual,
                    reason: decision.reason.clone(),
                    evidence: serde_json::json!({"queue": "label_review.tsv"}),
                    operator_id: Some(operator_id),
                },
                avatar_embeddings_enabled,
            )
            .await?;
        } else {
            record_not_spam(
                pool,
                chat_id,
                decision.user_id,
                &decision.reason,
                &serde_json::json!({"queue": "label_review.tsv"}),
                Some(operator_id),
            )
            .await?;
        }
        println!(
            "ok {} -> {}",
            decision.user_id,
            if decision.spam { "spam" } else { "ham" }
        );
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let mut args = std::env::args().skip(1);
    let mut chat_id: Option<i64> = None;
    let mut operator_id: Option<i64> = None;
    let mut input: Option<PathBuf> = None;
    let mut dry_run = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--chat-id" => {
                chat_id = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--chat-id needs a value"))?
                        .parse()?,
                )
            }
            "--operator-id" => {
                operator_id = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--operator-id needs a value"))?
                        .parse()?,
                )
            }
            "--input" => {
                input = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--input needs a value"))?
                        .into(),
                )
            }
            "--dry-run" => dry_run = true,
            other => anyhow::bail!("unknown arg {other:?}"),
        }
    }
    let (chat_id, operator_id, input) = match (chat_id, operator_id, input) {
        (Some(chat_id), Some(operator_id), Some(input)) => (chat_id, operator_id, input),
        _ => anyhow::bail!("need --chat-id, --operator-id and --input"),
    };
    let decisions = parse_tsv(&input)?;
    let spam = decisions.iter().filter(|decision| decision.spam).count();
    println!(
        "parsed {} decisions ({} spam, {} ham), dry_run={dry_run}",
        decisions.len(),
        spam,
        decisions.len() - spam
    );
    if dry_run {
        return Ok(());
    }
    let config = Config::from_env()?;
    let pool = build_pool().await?;
    apply(
        &pool,
        chat_id,
        operator_id,
        &decisions,
        config.avatar_embeddings_enabled,
    )
    .await
}
