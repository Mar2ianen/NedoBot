use teloxide::{
    prelude::*,
    types::{ChatId, MessageId, UserId},
};

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

/// Применяет лестницу автомодерации к свежему materialize-аудиту.
///
/// Ступени: review-порог — удалить первое сообщение (карточка идёт обычным
/// путём, улики уже в аудите); ban-порог — бан плюс удаление недавних
/// сообщений плюс System-метка в корпус. Всё идемпотентно: повторный прогон
/// пропускается по `is_spammer` и существующей System-метке.
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
    let action = decide_auto_action(score, &policy);
    if action == AutoAction::None {
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
    if moderation.enforce_dry_run {
        tracing::warn!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            ban_threshold = policy.ban_threshold,
            action = ?action,
            "auto moderation dry-run: Telegram API not called"
        );
        return Ok(());
    }
    match action {
        AutoAction::None => Ok(()),
        AutoAction::DeleteMessages => delete_first_message(bot, pool, job).await,
        AutoAction::Ban => ban_spammer(bot, pool, config, job, score, &policy).await,
    }
}

async fn delete_first_message(
    bot: &Bot,
    pool: &sqlx::PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<()> {
    let message_id: Option<i32> = sqlx::query_scalar(
        "select first_message_id from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let Some(message_id) = message_id else {
        return Ok(());
    };
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
    }
    Ok(())
}

async fn ban_spammer(
    bot: &Bot,
    pool: &sqlx::PgPool,
    _config: &Config,
    job: &NewUserAuditJob,
    score: i32,
    policy: &AutoPolicy,
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
                "deleted_messages": deleted,
            }),
            operator_id: None,
        },
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
