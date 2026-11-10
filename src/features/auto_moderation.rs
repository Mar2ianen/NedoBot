use teloxide::{
    prelude::*,
    types::{ChatId, ChatMemberKind, MessageId, UserId},
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
pub(crate) const ENFORCE_FRESHNESS: chrono::Duration = chrono::Duration::hours(24);
const MAX_DELETE_MESSAGES: i64 = 10;
const AUTO_BAN_EVIDENCE_LABELS: &[&str] = &[
    "local_spammer_identity",
    "shared_spammer_identity",
    "lols_spammer_identity",
    "tree_personal_channel_adult_funnel",
    "tree_personal_channel_invite_funnel",
];
const DETERMINISTIC_AUTO_BAN_LABEL: &str = "deterministic_campaign_fast_path";
const DETERMINISTIC_REVIEW_LABEL: &str = "deterministic_campaign_review_candidate";

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
/// Разметка усиливает риск, но не доказывает исполнение действия. Повторный
/// прогон сверяет текущий статус в Telegram; удаление — `deleted_by_bot_at`.
///
/// Капча — независимая review-ступень: для настроенного community она
/// заменяет удаление/бан и ограничивает участника до успешного ответа.
pub async fn maybe_enforce_audit(
    bot: &Bot,
    pool: &sqlx::PgPool,
    config: &Config,
    job: &NewUserAuditJob,
    score: i32,
) -> anyhow::Result<()> {
    if crate::features::new_user_audit::local_cascade::blocks_enforcement(
        job.assessment_json.as_ref(),
    ) {
        return Ok(());
    }
    let moderation = &config.community.moderation;
    if !moderation.enforce_enabled && !moderation.captcha_enabled {
        return Ok(());
    }
    if !is_managed_chat(config, job.chat_id)
        || !managed_chat_allows(config, job.chat_id, |chat| chat.moderation)
    {
        return Ok(());
    }
    if latest_label_is_not_spam(pool, job.chat_id, job.telegram_user_id).await? {
        tracing::info!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            "auto moderation skipped because latest manual label is not_spam"
        );
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
    let fresh = first_seen_at
        .is_some_and(|seen| chrono::Utc::now().signed_duration_since(seen) <= ENFORCE_FRESHNESS);
    // Размеченный участник может снова писать через недели после вступления.
    // Исторического replay недостаточно: нужен свежий апдейт в текущем аудите.
    let recent_known_spammer = is_spammer && has_recent_audited_activity(pool, job).await?;
    if !fresh && !recent_known_spammer {
        tracing::info!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            "auto moderation skipped stale audit"
        );
        return Ok(());
    }
    let member = bot
        .get_chat_member(ChatId(job.chat_id), UserId(job.telegram_user_id as u64))
        .await?;
    let eligible = match member.kind {
        ChatMemberKind::Member(_) => true,
        ChatMemberKind::Restricted(restricted) => restricted.is_member,
        _ => false,
    };
    if !eligible {
        tracing::info!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            "auto moderation skipped ineligible Telegram membership"
        );
        return Ok(());
    }
    let deterministic_campaign = has_deterministic_campaign(&job.input_json);
    if moderation.captcha_enabled
        && score >= job.review_threshold
        && score >= moderation.captcha_threshold
        && !deterministic_campaign
    {
        if moderation.captcha_dry_run {
            tracing::warn!(
                chat_id = job.chat_id,
                user_id = job.telegram_user_id,
                score,
                captcha_threshold = moderation.captcha_threshold,
                "risk captcha dry-run: Telegram API not called"
            );
            return Ok(());
        }
        if latest_label_is_not_spam(pool, job.chat_id, job.telegram_user_id).await? {
            tracing::info!(
                chat_id = job.chat_id,
                user_id = job.telegram_user_id,
                "risk captcha skipped because latest manual label is not_spam"
            );
            return Ok(());
        }
        let result = crate::features::risk_captcha::apply_risk_captcha(
            bot,
            pool,
            job,
            score,
            moderation.captcha_ttl_sec,
        )
        .await?;
        tracing::warn!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            score,
            outcome = ?result,
            "risk captcha applied"
        );
        return Ok(());
    }
    if !moderation.enforce_enabled {
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
            if latest_label_is_not_spam(pool, job.chat_id, job.telegram_user_id).await? {
                tracing::info!(
                    chat_id = job.chat_id,
                    user_id = job.telegram_user_id,
                    "auto ban skipped because latest manual label is not_spam"
                );
                return Ok(());
            }
            ban_spammer(bot, pool, config, job, score, &policy, &auto_ban_evidence).await
        }
    }
}

fn is_not_spam_label(latest_label: Option<&str>) -> bool {
    latest_label == Some("not_spam")
}

async fn latest_label_is_not_spam(
    pool: &sqlx::PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<bool> {
    let latest_label: Option<String> = sqlx::query_scalar(
        "select label from spam_label_events where chat_id = $1 and telegram_user_id = $2 order by created_at desc, id desc limit 1",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(is_not_spam_label(latest_label.as_deref()))
}

async fn has_recent_audited_activity(
    pool: &sqlx::PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<bool> {
    Ok(sqlx::query_scalar(
        r#"select exists (
             select 1 from telegram_messages m
             join telegram_new_user_profile_audits a
               on a.chat_id = m.chat_id and a.telegram_user_id = m.user_id
             where a.chat_id = $1 and a.telegram_user_id = $2
               and a.unified_audit_snapshot_hash = $3
               and m.message_id = a.last_message_id
               and m.source_channel_id is null and m.deleted_by_bot_at is null
               and case when jsonb_typeof(m.raw_json -> 'date') = 'number'
                 then (m.raw_json ->> 'date')::numeric >=
                      extract(epoch from now()) - $4::bigint
                 else m.created_at >= now() - ($4::bigint * interval '1 second')
               end
           )"#,
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .bind(&job.snapshot_hash)
    .bind(ENFORCE_FRESHNESS.num_seconds())
    .fetch_one(pool)
    .await?)
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
    let signals = if proposed_action != AutoAction::None {
        let signals: Value = sqlx::query_scalar(
            "select risk_signal_breakdown from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
        )
        .bind(chat_id)
        .bind(telegram_user_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or_default();
        signals
    } else {
        Value::Array(Vec::new())
    };
    let (action, safety_gate, auto_ban_evidence) =
        enforcement_for_signals(proposed_action, review_delivery_enabled, &signals);
    Ok(EnforcementDecision {
        action,
        safety_gate,
        auto_ban_evidence,
    })
}

pub(crate) fn qualifying_auto_ban_evidence(signals: &Value) -> Vec<String> {
    signals
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|signal| {
            let label = signal.get("label")?.as_str()?;
            let qualifies = if label == DETERMINISTIC_AUTO_BAN_LABEL {
                signal.get("auto_ban_eligible").and_then(Value::as_bool) == Some(true)
            } else if label == "known_spam_campaign_match" {
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

fn has_review_only_campaign_signal(signals: &Value) -> bool {
    signals.as_array().into_iter().flatten().any(|signal| {
        signal.get("label").and_then(Value::as_str) == Some(DETERMINISTIC_REVIEW_LABEL)
    })
}

fn enforcement_for_signals(
    proposed_action: AutoAction,
    review_delivery_enabled: bool,
    signals: &Value,
) -> (AutoAction, &'static str, Vec<String>) {
    let auto_ban_evidence = qualifying_auto_ban_evidence(signals);
    let has_local_review = signals.as_array().into_iter().flatten().any(|signal| {
        signal.get("label").and_then(Value::as_str)
            == Some(crate::features::new_user_audit::local_cascade::REVIEW_LABEL)
    });
    let (action, safety_gate) = if has_local_review && auto_ban_evidence.is_empty() {
        (AutoAction::None, "local_cascade_review_only")
    } else if has_review_only_campaign_signal(signals) && auto_ban_evidence.is_empty() {
        (AutoAction::None, "deterministic_campaign_review_only")
    } else {
        effective_action(
            proposed_action,
            review_delivery_enabled,
            !auto_ban_evidence.is_empty(),
        )
    };
    (action, safety_gate, auto_ban_evidence)
}

fn has_deterministic_campaign(input_json: &Value) -> bool {
    input_json
        .get("deterministic_campaign_rule")
        .and_then(Value::as_str)
        .is_some()
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

pub(crate) async fn delete_first_message(
    bot: &Bot,
    pool: &sqlx::PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<()> {
    delete_first_message_for_user(bot, pool, job.chat_id, job.telegram_user_id).await
}

pub(crate) async fn delete_first_message_for_user(
    bot: &Bot,
    pool: &sqlx::PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<()> {
    if latest_label_is_not_spam(pool, chat_id, user_id).await? {
        tracing::info!(
            chat_id,
            user_id,
            "automatic message deletion skipped because latest manual label is not_spam"
        );
        return Ok(());
    }
    let row: Option<(i32, bool)> = sqlx::query_as(
        "select m.message_id, m.deleted_by_bot_at is not null as deleted from telegram_messages m join telegram_new_user_profile_audits a on a.chat_id = m.chat_id and a.first_message_id = m.message_id and a.telegram_user_id = m.user_id where a.chat_id = $1 and a.telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some((message_id, already_deleted)) = row else {
        return Ok(());
    };
    if already_deleted {
        return Ok(());
    }
    if let Err(error) = bot
        .delete_message(ChatId(chat_id), MessageId(message_id))
        .await
    {
        tracing::warn!(
            chat_id,
            user_id,
            message_id,
            %error,
            "auto moderation failed to delete first message"
        );
        return Ok(());
    }
    crate::db::telegram::mark_message_deleted_by_bot(
        pool,
        chat_id,
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
    // Recheck after DB reads, immediately before the irreversible Telegram call.
    if latest_label_is_not_spam(pool, job.chat_id, job.telegram_user_id).await? {
        tracing::info!(
            chat_id = job.chat_id,
            user_id = job.telegram_user_id,
            "auto ban skipped because latest manual label is not_spam"
        );
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deterministic_campaign_is_confirmed_auto_ban_evidence() {
        let evidence = qualifying_auto_ban_evidence(&json!([
            {"label": "deterministic_campaign_fast_path", "auto_ban_eligible": true, "rule_version": "new-user-features-v3"}
        ]));
        assert_eq!(evidence, ["deterministic_campaign_fast_path"]);
        assert_eq!(
            effective_action(AutoAction::Ban, false, !evidence.is_empty()),
            (AutoAction::Ban, "confirmed_ban_evidence")
        );
    }

    #[test]
    fn unverified_and_legacy_deterministic_campaigns_are_not_ban_evidence() {
        for signal in [
            json!({"label": "deterministic_campaign_fast_path", "auto_ban_eligible": false}),
            json!({"label": "deterministic_campaign_fast_path"}),
            json!({"label": "deterministic_campaign_review_candidate"}),
        ] {
            assert!(qualifying_auto_ban_evidence(&json!([signal])).is_empty());
        }
    }

    #[test]
    fn review_only_campaign_suppresses_automatic_deletion_and_ban() {
        let signals = json!([{"label": "deterministic_campaign_review_candidate"}]);
        for proposed in [AutoAction::DeleteMessages, AutoAction::Ban] {
            let (action, gate, evidence) = enforcement_for_signals(proposed, true, &signals);
            assert_eq!(action, AutoAction::None);
            assert_eq!(gate, "deterministic_campaign_review_only");
            assert!(evidence.is_empty());
        }
    }

    #[test]
    fn local_review_signal_cannot_authorize_actions_but_preserves_independent_evidence() {
        let mut signals = json!([{"label":crate::features::new_user_audit::local_cascade::REVIEW_LABEL,"auto_ban_eligible":false}]);
        for proposed in [AutoAction::DeleteMessages, AutoAction::Ban] {
            let (action, gate, evidence) = enforcement_for_signals(proposed, true, &signals);
            assert_eq!(action, AutoAction::None);
            assert_eq!(gate, "local_cascade_review_only");
            assert!(evidence.is_empty());
        }
        signals
            .as_array_mut()
            .unwrap()
            .push(json!({"label":"local_spammer_identity"}));
        let (action, gate, _) = enforcement_for_signals(AutoAction::Ban, true, &signals);
        assert_eq!(action, AutoAction::Ban);
        assert_eq!(gate, "confirmed_ban_evidence");
    }

    #[test]
    fn review_only_campaign_does_not_suppress_independent_ban_evidence() {
        let signals = json!([
            {"label": "deterministic_campaign_review_candidate"},
            {"label": "local_spammer_identity"}
        ]);
        let (action, gate, evidence) = enforcement_for_signals(AutoAction::Ban, false, &signals);
        assert_eq!(action, AutoAction::Ban);
        assert_eq!(gate, "confirmed_ban_evidence");
        assert_eq!(evidence, ["local_spammer_identity"]);
    }

    #[test]
    fn latest_manual_not_spam_label_blocks_automatic_enforcement() {
        assert!(is_not_spam_label(Some("not_spam")));
        assert!(!is_not_spam_label(Some("spam")));
        assert!(!is_not_spam_label(None));
    }

    #[test]
    fn deterministic_campaign_does_not_enter_captcha_path() {
        assert!(has_deterministic_campaign(&json!({
            "deterministic_campaign_rule": "fresh_money_work_private_contact"
        })));
        assert!(!has_deterministic_campaign(&json!({
            "deterministic_campaign_rule": null
        })));
    }
}
