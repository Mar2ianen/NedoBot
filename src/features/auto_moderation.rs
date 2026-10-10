use teloxide::{
    prelude::*,
    types::{ChatId, MessageId, UserId},
};

use serde_json::Value;
use teloxide_antispam::policy::{AutoAction, AutoPolicy, decide_auto_action};

use crate::{
    config::Config,
    features::{
        ingest::{is_managed_chat, managed_chat_allows},
        labels::{LabelSource, SpamLabel, record_spam},
        new_user_audit::repo::NewUserAuditJob,
    },
};

/// Окно свежести для enforcement: банить задним числом по старым аудитам
/// нельзя, replay пересчитывает историю новым кодом.
const ENFORCE_FRESHNESS: chrono::Duration = chrono::Duration::hours(24);
const MAX_DELETE_MESSAGES: i64 = 10;
const AUTO_BAN_EVIDENCE_LABELS: &[&str] = &[
    "shared_spammer_identity",
    "lols_spammer_identity",
    "tree_personal_channel_adult_funnel",
    "tree_personal_channel_invite_funnel",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnforcementDecision {
    pub action: AutoAction,
    pub safety_gate: &'static str,
    pub auto_ban_evidence: Vec<String>,
}

/// Применяет лестницу автомодерации к свежему materialize-аудиту.
///
/// На review-пороге первое сообщение удаляется только при включённой доставке
/// карточек. Бан-порог требует отдельного подтверждённого сигнала; одного
/// score недостаточно. Бан удаляет недавние сообщения и пишет System-метку.
/// Всё идемпотентно: повторный прогон пропускается по `is_spammer` и
/// существующей System-метке, повторное удаление — по `deleted_by_bot_at`.
///
/// При `enforce_dry_run` только пишется warn-лог, Telegram API и записи
/// не трогаются.
pub async fn maybe_enforce_audit(
    bot: &Bot,
    pool: &sqlx::PgPool,
    config: &Config,
    job: &NewUserAuditJob,
    score: i32,
) -> anyhow::Result<()> {
    let moderation = &config.community.moderation;
    if !moderation.enforce_enabled {
        return Ok(());
    }
    if !is_managed_chat(config, job.chat_id)
        || !managed_chat_allows(config, job.chat_id, |chat| chat.moderation)
    {
        return Ok(());
    }
    let policy = AutoPolicy {
        review_threshold: job.review_threshold,
        ban_threshold: moderation.enforce_ban_threshold,
    };
    let proposed_action = decide_auto_action(score, &policy);
    if proposed_action == AutoAction::None {
        return Ok(());
    }
    let user: Option<(bool, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "select is_spammer, first_seen_at from telegram_chat_users where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_optional(pool)
    .await?;
    let Some((is_spammer, first_seen_at)) = user else {
        return Ok(());
    };
    if is_spammer {
        return Ok(());
    }
    let fresh = first_seen_at
        .is_some_and(|seen| chrono::Utc::now().signed_duration_since(seen) <= ENFORCE_FRESHNESS);
    if !fresh {
        tracing::info!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            "auto moderation skipped stale audit"
        );
        return Ok(());
    }
    let system_labeled: bool = sqlx::query_scalar(
        "select exists (select 1 from spam_label_events where chat_id = $1 and telegram_user_id = $2 and label = 'spam' and source = 'system')",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_one(pool)
    .await?;
    if system_labeled {
        return Ok(());
    }
    let decision = decide_enforcement_for_audit(
        pool,
        job.chat_id,
        job.telegram_user_id,
        proposed_action,
        moderation.review_delivery_enabled,
    )
    .await?;
    let EnforcementDecision {
        action,
        safety_gate,
        auto_ban_evidence,
    } = decision;
    if action == AutoAction::None {
        tracing::warn!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            proposed_action = ?proposed_action,
            safety_gate,
            "auto moderation action suppressed by safety gate"
        );
        return Ok(());
    }
    if moderation.enforce_dry_run {
        tracing::warn!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            ban_threshold = policy.ban_threshold,
            proposed_action = ?proposed_action,
            action = ?action,
            safety_gate,
            auto_ban_evidence = ?auto_ban_evidence,
            "auto moderation dry-run: Telegram API not called"
        );
        return Ok(());
    }
    match action {
        AutoAction::None => Ok(()),
        AutoAction::DeleteMessages => delete_first_message(bot, pool, job).await,
        AutoAction::Ban => {
            ban_spammer(bot, pool, config, job, score, &policy, &auto_ban_evidence).await
        }
    }
}

/// Loads persisted audit signals and applies the evidence/review delivery gates.
/// The durable JSON is covered by PostgreSQL integration tests.
pub async fn decide_enforcement_for_audit(
    pool: &sqlx::PgPool,
    chat_id: i64,
    telegram_user_id: i64,
    proposed_action: AutoAction,
    review_delivery_enabled: bool,
) -> anyhow::Result<EnforcementDecision> {
    let auto_ban_evidence = if proposed_action == AutoAction::Ban {
        let signals: Value = sqlx::query_scalar(
            "select risk_signal_breakdown from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
        )
        .bind(chat_id)
        .bind(telegram_user_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default();
        qualifying_auto_ban_evidence(&signals)
    } else {
        Vec::new()
    };
    let (action, safety_gate) = effective_action(
        proposed_action,
        review_delivery_enabled,
        !auto_ban_evidence.is_empty(),
    );
    Ok(EnforcementDecision {
        action,
        safety_gate,
        auto_ban_evidence,
    })
}

fn qualifying_auto_ban_evidence(signals: &Value) -> Vec<String> {
    signals
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|signal| {
            let label = signal.get("label")?.as_str()?;
            let qualifies = if label == "known_spam_campaign_match" {
                signal
                    .get("template_matches")
                    .and_then(Value::as_i64)
                    .is_some_and(|count| count > 0)
            } else {
                AUTO_BAN_EVIDENCE_LABELS.contains(&label)
            };
            qualifies.then(|| label.to_owned())
        })
        .collect()
}

fn effective_action(
    proposed_action: AutoAction,
    review_delivery_enabled: bool,
    has_auto_ban_evidence: bool,
) -> (AutoAction, &'static str) {
    match proposed_action {
        AutoAction::None => (AutoAction::None, "below_review_threshold"),
        AutoAction::DeleteMessages if review_delivery_enabled => {
            (AutoAction::DeleteMessages, "review_delivery_enabled")
        }
        AutoAction::DeleteMessages => (AutoAction::None, "review_delivery_disabled"),
        AutoAction::Ban if has_auto_ban_evidence => (AutoAction::Ban, "confirmed_ban_evidence"),
        AutoAction::Ban if review_delivery_enabled => (
            AutoAction::DeleteMessages,
            "ban_evidence_missing_review_fallback",
        ),
        AutoAction::Ban => (AutoAction::None, "ban_evidence_missing_and_review_disabled"),
    }
}

async fn delete_first_message(
    bot: &Bot,
    pool: &sqlx::PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<()> {
    let row: Option<(i32, bool)> = sqlx::query_as(
        "select m.message_id, m.deleted_by_bot_at is not null as deleted from telegram_messages m join telegram_new_user_profile_audits a on a.chat_id = m.chat_id and a.first_message_id = m.message_id and a.telegram_user_id = m.user_id where a.chat_id = $1 and a.telegram_user_id = $2",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_optional(pool)
    .await?;
    let Some((message_id, already_deleted)) = row else {
        return Ok(());
    };
    if already_deleted {
        return Ok(());
    }
    if let Err(error) = bot
        .delete_message(ChatId(job.chat_id), MessageId(message_id))
        .await
    {
        tracing::warn!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            message_id,
            %error,
            "auto moderation failed to delete first message"
        );
        return Ok(());
    }
    crate::db::telegram::mark_message_deleted_by_bot(
        pool,
        job.chat_id,
        message_id,
        None,
        Some("auto_moderation_delete"),
    )
    .await?;
    Ok(())
}

async fn ban_spammer(
    bot: &Bot,
    pool: &sqlx::PgPool,
    _config: &Config,
    job: &NewUserAuditJob,
    score: i32,
    policy: &AutoPolicy,
    auto_ban_evidence: &[String],
) -> anyhow::Result<()> {
    let message_ids: Vec<i32> = sqlx::query_scalar(
        "select message_id from telegram_messages where chat_id = $1 and user_id = $2 and source_channel_id is null order by message_id desc limit $3",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(MAX_DELETE_MESSAGES)
    .fetch_all(pool)
    .await?;
    if let Err(error) = bot
        .ban_chat_member(ChatId(job.chat_id), UserId(job.telegram_user_id as u64))
        .await
    {
        tracing::warn!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            %error,
            "auto moderation failed to ban user"
        );
        return Ok(());
    }
    let mut deleted = 0;
    for message_id in message_ids {
        if bot
            .delete_message(ChatId(job.chat_id), MessageId(message_id))
            .await
            .is_ok()
        {
            deleted += 1;
        }
    }
    record_spam(
        pool,
        job.chat_id,
        job.telegram_user_id,
        &SpamLabel {
            subtype: "auto_enforcement".to_string(),
            source: LabelSource::System,
            reason: format!(
                "Auto-ban at score {score} (review {}, ban {})",
                policy.review_threshold, policy.ban_threshold
            ),
            evidence: serde_json::json!({
                    "score": score,
                "review_threshold": policy.review_threshold,
                "ban_threshold": policy.ban_threshold,
                "auto_ban_evidence": auto_ban_evidence,
                "deleted_messages": deleted,
            }),
            operator_id: None,
        },
        false,
    )
    .await?;
    tracing::warn!(
        chat_id = job.chat_id,
        user_id = job.telegram_user_id,
        score,
        deleted,
        "auto moderation banned user"
    );
    Ok(())
}
