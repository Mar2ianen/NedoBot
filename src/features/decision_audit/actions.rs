//! Durable handoff: validated Decision → общие метрики → существующая ephemeral captcha.
use super::{
    client::{DecisionAssessment, POLICY_VERSION, QUESTIONS},
    contacts,
    repo::{self, DecisionJob},
    scoring,
};
use crate::{
    config::Config,
    config_file::DecisionMode,
    features::{
        jobs::policy::ANALYSIS_RETRY, new_user_audit::local_cascade::hash_bytes, risk_captcha,
    },
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use teloxide::Bot;
// Несколько Telegram-вызовов и durable setup; длиннее single provider request lease.
const ACTION_LEASE_SECONDS: i64 = 300;
const MAX_ACTION_ATTEMPTS: i32 = 6;
#[derive(FromRow)]
struct Action {
    #[sqlx(flatten)]
    job: DecisionJob,
    action_fence: i64,
    action_attempts: i32,
    message_at: Option<DateTime<Utc>>,
    assessment_json: Option<Value>,
}
async fn claim(pool: &PgPool, c: &Config) -> anyhow::Result<Option<Action>> {
    if c.community.moderation.decision.mode != DecisionMode::ScoreCaptcha
        || !c.community.moderation.decision.enabled
        || !c.community.moderation.captcha_enabled
        || !c.community.moderation.enabled
    {
        return Ok(None);
    }
    let chats: Vec<i64> = c
        .community
        .chats
        .values()
        .filter(|v| repo::enabled_for_chat(c, v.id))
        .map(|v| v.id)
        .collect();
    let mut tx = pool.begin().await?;
    // Baseline/profile audit может завершиться позже Decision. Повторяем только
    // ранее подпороговый положительный результат, если общая метрика пересекла порог.
    sqlx::query(r#"update moderation_decision_jobs j set action_status='pending',action_next_attempt_at=now()
        from telegram_new_user_profile_audits a,telegram_messages m
        where j.instance_id=$1 and j.chat_id=any($2) and j.status='succeeded'
          and j.action_status='done' and j.action_error_kind='below_threshold_or_abstain'
          and j.shadow_verdict='review_candidate' and a.chat_id=j.chat_id and a.telegram_user_id=j.user_id
          and a.risk_decision_job_id=j.id and a.risk_score >= $3
          and coalesce((j.metrics_json->>'combined_score')::integer,0)<$3
          and m.chat_id=j.chat_id and m.message_id=j.message_id and m.user_id=j.user_id
          and m.created_at>now()-make_interval(secs=>$4::double precision)"#)
        .bind(&c.community.instance.id).bind(&chats).bind(c.community.moderation.captcha_threshold)
        .bind(c.community.moderation.decision.captcha_max_message_age_sec as f64).execute(&mut *tx).await?;
    sqlx::query("update moderation_decision_jobs set action_status='skipped',action_error_kind='obsolete_policy',action_lease_expires_at=null where instance_id=$1 and action_status in ('pending','retry_wait','processing') and (not(chat_id=any($2)) or policy_version<>$3 or question_version<>$4 or input_json->>'configuration_hash' is distinct from $5)")
        .bind(&c.community.instance.id).bind(&chats).bind(POLICY_VERSION).bind(hash_bytes(QUESTIONS.as_bytes())).bind(super::service::configuration_hash(c)).execute(&mut *tx).await?;
    let id:Option<i64>=sqlx::query_scalar(r#"select id from moderation_decision_jobs where instance_id=$1 and chat_id=any($2) and status='succeeded'
        and ((action_status in ('pending','retry_wait') and action_next_attempt_at<=now()) or(action_status='processing' and action_lease_expires_at<=now()))
        order by action_next_attempt_at,id for update skip locked limit 1"#)
        .bind(&c.community.instance.id).bind(chats).fetch_optional(&mut *tx).await?;
    let Some(id) = id else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("update moderation_decision_jobs set action_status='processing',action_attempts=action_attempts+1,action_fence=action_fence+1,action_lease_expires_at=now()+make_interval(secs=>$2::double precision) where id=$1")
        .bind(id).bind(ACTION_LEASE_SECONDS as f64).execute(&mut *tx).await?;
    let action=sqlx::query_as::<_,Action>("select j.*,m.created_at as message_at from moderation_decision_jobs j left join telegram_messages m on m.chat_id=j.chat_id and m.message_id=j.message_id and m.user_id=j.user_id where j.id=$1")
        .bind(id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(action))
}
async fn finish(pool: &PgPool, a: &Action, status: &str, reason: &str) -> anyhow::Result<()> {
    sqlx::query("update moderation_decision_jobs set action_status=$3,action_error_kind=$4,action_lease_expires_at=null where id=$1 and action_fence=$2 and action_status='processing'")
        .bind(a.job.id).bind(a.action_fence).bind(status).bind(reason).execute(pool).await?;
    Ok(())
}
async fn retry(pool: &PgPool, a: &Action) -> anyhow::Result<()> {
    sqlx::query("update moderation_decision_jobs set action_status=$3,action_error_kind='action_unavailable',action_next_attempt_at=now()+make_interval(secs=>$4::double precision),action_lease_expires_at=null where id=$1 and action_fence=$2 and action_status='processing'")
        .bind(a.job.id).bind(a.action_fence).bind(if a.action_attempts>=MAX_ACTION_ATTEMPTS{"failed"}else{"retry_wait"})
        .bind(ANALYSIS_RETRY.delay_seconds(a.action_attempts,None).unwrap_or(0) as f64).execute(pool).await?;
    Ok(())
}
async fn materialize(
    pool: &PgPool,
    c: &Config,
    a: &Action,
    score: &scoring::Contribution,
) -> anyhow::Result<Option<i32>> {
    let mut tx = pool.begin().await?;
    let owned:bool=sqlx::query_scalar("select exists(select 1 from moderation_decision_jobs where id=$1 and instance_id=$2 and action_fence=$3 and action_status='processing' and action_lease_expires_at>now())")
        .bind(a.job.id).bind(&a.job.instance_id).bind(a.action_fence).fetch_one(&mut *tx).await?;
    if !owned || !repo::current_in_tx(&mut tx, &a.job).await? {
        tx.rollback().await?;
        return Ok(None);
    }
    let row:Option<i32>=sqlx::query_scalar(r#"insert into telegram_new_user_profile_audits
        (chat_id,telegram_user_id,risk_decision_job_id,risk_decision_message_id,risk_decision_score,risk_decision_signals,risk_contact_score,risk_contact_signals)
        values ($1,$2,$3,$4,$5,$6,$7,$8) on conflict(chat_id,telegram_user_id) do update set
        risk_decision_job_id=excluded.risk_decision_job_id,risk_decision_message_id=excluded.risk_decision_message_id,
        risk_decision_score=excluded.risk_decision_score,risk_decision_signals=excluded.risk_decision_signals,
        risk_contact_score=excluded.risk_contact_score,risk_contact_signals=excluded.risk_contact_signals
        where (telegram_new_user_profile_audits.risk_decision_message_id is null or
         (telegram_new_user_profile_audits.risk_decision_message_id,coalesce(telegram_new_user_profile_audits.risk_decision_job_id,0)) <= (excluded.risk_decision_message_id,excluded.risk_decision_job_id))
         and not coalesce((select label='not_spam' from spam_label_events where chat_id=$1 and telegram_user_id=$2 order by created_at desc,id desc limit 1),false)
        returning risk_score"#)
        .bind(a.job.chat_id).bind(a.job.user_id).bind(a.job.id).bind(a.job.message_id).bind(score.decision_score).bind(score.decision_signals()).bind(score.contact_score).bind(score.contact_signals()).fetch_optional(&mut *tx).await?;
    sqlx::query("update moderation_decision_jobs set metrics_json=$3 where id=$1 and action_fence=$2")
        .bind(a.job.id).bind(a.action_fence).bind(serde_json::json!({"contribution":score,"combined_score":row,"configuration_hash":super::service::configuration_hash(c)})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(row)
}
async fn apply(bot: &Bot, pool: &PgPool, c: &Config, a: &Action) -> anyhow::Result<()> {
    if a.action_attempts > MAX_ACTION_ATTEMPTS {
        return finish(pool, a, "failed", "retry_exhausted").await;
    }
    if !repo::source_is_current(pool, &a.job).await? {
        return finish(pool, a, "skipped", "source_changed").await;
    }
    let assessment: DecisionAssessment = serde_json::from_value(
        a.assessment_json
            .clone()
            .ok_or_else(|| anyhow::anyhow!("missing stored decision"))?,
    )?;
    assessment
        .validate()
        .map_err(|_| anyhow::anyhow!("invalid stored decision"))?;
    let source:Option<(String,Value)>=sqlx::query_as("select coalesce(text,''),coalesce(raw_json->'entities',raw_json->'caption_entities','[]'::jsonb) from telegram_messages where chat_id=$1 and message_id=$2 and user_id=$3")
        .bind(a.job.chat_id).bind(a.job.message_id).bind(a.job.user_id).fetch_optional(pool).await?;
    let Some((text, entities)) = source else {
        return finish(pool, a, "skipped", "source_missing").await;
    };
    let contacts = contacts::extract(pool, a.job.chat_id, a.job.user_id, &text, &entities).await?;
    let contribution =
        scoring::contribution(&assessment, &contacts, &c.community.moderation.decision);
    let Some(total) = materialize(pool, c, a, &contribution).await? else {
        return finish(pool, a, "skipped", "stale_metrics").await;
    };
    if !contribution.review_candidate || total < c.community.moderation.captcha_threshold {
        return finish(pool, a, "done", "below_threshold_or_abstain").await;
    }
    if a.message_at.is_none_or(|t| {
        !(0..=c.community.moderation.decision.captcha_max_message_age_sec)
            .contains(&Utc::now().signed_duration_since(t).num_seconds())
    }) {
        return finish(pool, a, "skipped", "message_too_old").await;
    }
    if c.community.moderation.captcha_dry_run {
        return finish(pool, a, "done", "captcha_dry_run").await;
    }
    if !repo::source_is_current(pool, &a.job).await? {
        return finish(pool, a, "skipped", "source_changed").await;
    }
    let authorized=sqlx::query("update moderation_decision_jobs set metrics_json=jsonb_set(metrics_json,'{captcha_authorized}','true'::jsonb) where id=$1 and action_fence=$2 and action_status='processing' and action_lease_expires_at>now()")
        .bind(a.job.id).bind(a.action_fence).execute(pool).await?;
    if authorized.rows_affected() != 1 {
        return Ok(());
    }
    let outcome = risk_captcha::apply_decision_captcha(
        bot,
        pool,
        &a.job,
        total,
        c.community.moderation.captcha_ttl_sec,
        c.community.moderation.decision.ban_on_failure,
    )
    .await?;
    tracing::info!(
        job_id = a.job.id,
        score = total,
        ?outcome,
        "Decision captcha handoff completed"
    );
    finish(
        pool,
        a,
        "done",
        match outcome {
            risk_captcha::CaptchaOutcome::Issued => "issued",
            risk_captcha::CaptchaOutcome::Ineligible => "ineligible",
            _ => "existing_challenge",
        },
    )
    .await
}
pub async fn process_next(bot: &Bot, pool: &PgPool, c: &Config) -> anyhow::Result<bool> {
    let Some(a) = claim(pool, c).await? else {
        return Ok(false);
    };
    if apply(bot, pool, c, &a).await.is_err() {
        retry(pool, &a).await?;
        tracing::warn!(
            job_id = a.job.id,
            "Decision captcha handoff scheduled for bounded retry"
        );
    }
    Ok(true)
}
