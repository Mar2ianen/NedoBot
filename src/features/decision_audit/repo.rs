use super::client::{DecisionAssessment, Failure};
use crate::config::Config;
use crate::features::{
    jobs::policy::{ANALYSIS_RETRY, EXTERNAL_REQUEST_LEASE},
    new_user_audit::local_cascade::hash_bytes,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{FromRow, PgPool};

pub const MAX_ATTEMPTS: i32 = 6;
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnqueueResult {
    Queued,
    Disabled,
    Duplicate,
    BacklogFull,
    ForeignInstance,
}

#[derive(Debug, Clone, FromRow)]
pub struct DecisionJob {
    pub id: i64,
    pub instance_id: String,
    pub chat_id: i64,
    pub message_id: i32,
    pub user_id: i64,
    pub snapshot_hash: String,
    pub source_hash: String,
    pub source_edited_at: Option<DateTime<Utc>>,
    pub label_event_id: i64,
    pub question_version: String,
    pub normalization_version: String,
    pub policy_version: String,
    pub model: String,
    pub input_json: Value,
    pub avatar_file_id: Option<String>,
    pub avatar_unique_id: Option<String>,
    pub attempts: i32,
    pub fence: i64,
}

pub fn enabled_for_chat(config: &Config, chat_id: i64) -> bool {
    config.community.moderation.enabled
        && config.community.moderation.decision.enabled
        && config
            .community
            .moderation
            .decision
            .external_processing_allowed
        && config.chat_allows(chat_id, |chat| chat.ingest && chat.moderation)
}

pub async fn enqueue(
    pool: &PgPool,
    config: &Config,
    job: &DecisionJob,
) -> anyhow::Result<EnqueueResult> {
    if !enabled_for_chat(config, job.chat_id) {
        return Ok(EnqueueResult::Disabled);
    }
    if job.instance_id != config.community.instance.id {
        return Ok(EnqueueResult::ForeignInstance);
    }
    let mut tx = pool.begin().await?;
    // Один bounded backlog на instance, включая параллельный ingest.
    sqlx::query("select pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("decision-enqueue:{}", job.instance_id))
        .execute(&mut *tx)
        .await?;
    let pending: i64 = sqlx::query_scalar("select count(*) from moderation_decision_jobs where instance_id=$1 and status in ('pending','retry_wait','processing')")
        .bind(&job.instance_id).fetch_one(&mut *tx).await?;
    if pending >= config.community.moderation.decision.max_pending_jobs {
        return Ok(EnqueueResult::BacklogFull);
    }
    let inserted = sqlx::query("insert into moderation_decision_jobs (instance_id,chat_id,message_id,user_id,snapshot_hash,source_hash,source_edited_at,label_event_id,question_version,normalization_version,policy_version,model,input_json,avatar_file_id,avatar_unique_id) values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) on conflict do nothing")
        .bind(&job.instance_id).bind(job.chat_id).bind(job.message_id).bind(job.user_id)
        .bind(&job.snapshot_hash).bind(&job.source_hash).bind(job.source_edited_at).bind(job.label_event_id)
        .bind(&job.question_version).bind(&job.normalization_version).bind(&job.policy_version).bind(&job.model)
        .bind(&job.input_json).bind(&job.avatar_file_id).bind(&job.avatar_unique_id).execute(&mut *tx).await?.rows_affected() > 0;
    tx.commit().await?;
    Ok(if inserted {
        EnqueueResult::Queued
    } else {
        EnqueueResult::Duplicate
    })
}

/// Чужие scope/версии не могут быть взяты текущим worker. Quota считает попытки, не удачные ответы.
pub async fn claim(pool: &PgPool, config: &Config) -> anyhow::Result<Option<DecisionJob>> {
    if !config.community.moderation.decision.enabled
        || !config
            .community
            .moderation
            .decision
            .external_processing_allowed
        || !config.community.moderation.enabled
    {
        return Ok(None);
    }
    let chats: Vec<i64> = config
        .community
        .chats
        .values()
        .filter(|c| enabled_for_chat(config, c.id))
        .map(|c| c.id)
        .collect();
    let mut tx = pool.begin().await?;
    sqlx::query("update moderation_decision_jobs set status='stale',completed_at=now(),lease_expires_at=null,error_kind='obsolete_version' where instance_id=$1 and status in ('pending','retry_wait','processing') and (not(chat_id=any($2)) or model<>$3 or question_version<>$4 or policy_version<>$5 or input_json->>'configuration_hash' is distinct from $6 or normalization_version<>$7 or created_at<now()-interval '24 hours')")
        .bind(&config.community.instance.id).bind(&chats).bind(&config.community.moderation.decision.model)
        .bind(hash_bytes(super::client::QUESTIONS.as_bytes())).bind(super::client::POLICY_VERSION)
        .bind(super::service::configuration_hash(config)).bind(teloxide_antispam::preprocess::PREPROCESSING_VERSION)
        .execute(&mut *tx).await?;
    let job: Option<DecisionJob> = sqlx::query_as(
        r#"
        select * from moderation_decision_jobs j where instance_id=$1 and chat_id=any($2)
        and model=$3 and question_version=$4 and policy_version=$5
        and input_json->>'configuration_hash'=$6 and normalization_version=$7
        and ((status in ('pending','retry_wait') and next_attempt_at<=now())
          or (status='processing' and lease_expires_at<=now()))
        order by next_attempt_at,id for update skip locked limit 1"#,
    )
    .bind(&config.community.instance.id)
    .bind(chats)
    .bind(&config.community.moderation.decision.model)
    .bind(hash_bytes(super::client::QUESTIONS.as_bytes()))
    .bind(super::client::POLICY_VERSION)
    .bind(super::service::configuration_hash(config))
    .bind(teloxide_antispam::preprocess::PREPROCESSING_VERSION)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(mut job) = job else {
        return Ok(None);
    };
    if job.attempts >= MAX_ATTEMPTS || !current_in_tx(&mut tx, &job).await? {
        sqlx::query("update moderation_decision_jobs set status=$2,error_kind=$3,completed_at=now(),lease_expires_at=null where id=$1")
            .bind(job.id).bind(if job.attempts >= MAX_ATTEMPTS {"failed"}else{"stale"})
            .bind(if job.attempts >= MAX_ATTEMPTS {"retry_exhausted"}else{"expired_or_changed"}).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(None);
    }
    let available: Option<i32> = sqlx::query_scalar(r#"insert into moderation_decision_budget (instance_id,window_start,calls)
        values ($1,date_trunc('hour',now()),1) on conflict (instance_id,window_start)
        do update set calls=moderation_decision_budget.calls+1 where moderation_decision_budget.calls<$2 returning calls"#)
        .bind(&job.instance_id).bind(config.community.moderation.decision.max_calls_per_hour).fetch_optional(&mut *tx).await?;
    if available.is_none() {
        sqlx::query("update moderation_decision_jobs set status='retry_wait',next_attempt_at=date_trunc('hour',now())+interval '1 hour',lease_expires_at=null,error_kind='budget' where id=$1")
            .bind(job.id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(None);
    }
    job.attempts += 1;
    job.fence += 1;
    sqlx::query("update moderation_decision_jobs set status='processing',attempts=$2,fence=$3,lease_expires_at=now()+make_interval(secs=>$4::double precision),error_kind=null where id=$1")
        .bind(job.id).bind(job.attempts).bind(job.fence).bind(EXTERNAL_REQUEST_LEASE.seconds() as f64).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(job))
}

async fn current_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    job: &DecisionJob,
) -> anyhow::Result<bool> {
    let current: Option<(String,Option<DateTime<Utc>>,i64,bool)> = sqlx::query_as(r#"
        select coalesce(m.text,''),m.edited_at,coalesce(l.id,0),coalesce(l.label='not_spam',false)
        from telegram_messages m
        join telegram_chat_users u on u.chat_id=m.chat_id and u.telegram_user_id=m.user_id
        left join lateral (
          select id,label from spam_label_events where chat_id=m.chat_id and telegram_user_id=m.user_id
          order by created_at desc,id desc limit 1) l on true
        where m.chat_id=$1 and m.message_id=$2 and m.user_id=$3 and m.source_channel_id is null
          and m.created_at>now()-interval '24 hours'
          and u.message_count between 1 and $5
          and u.first_seen_at between now()-make_interval(secs=>$6::double precision) and now()
          and not exists(select 1 from telegram_chat_member_snapshots s where s.chat_id=m.chat_id and s.telegram_user_id=m.user_id and s.is_admin)
          and not exists(select 1 from telegram_user_profiles p where p.telegram_user_id=m.user_id and p.is_bot)
          and ($4::text is null or exists(select 1 from telegram_user_profiles p where p.telegram_user_id=m.user_id and p.profile_photo_file_unique_id=$4))
        for share of m"#)
        .bind(job.chat_id).bind(job.message_id).bind(job.user_id).bind(&job.avatar_unique_id)
        .bind(crate::features::new_user_audit::diagnostics::newcomer_limits().0)
        .bind(crate::features::new_user_audit::diagnostics::newcomer_limits().1 as f64)
        .fetch_optional(&mut **tx).await?;
    Ok(current.is_some_and(|(text, edit, label, normal)| {
        hash_bytes(text.as_bytes()) == job.source_hash
            && edit == job.source_edited_at
            && label == job.label_event_id
            && !normal
    }))
}

/// CAS и повторная проверка источника/ручной метки; никакой записи в audit/review/enforcement.
pub async fn complete(
    pool: &PgPool,
    job: &DecisionJob,
    assessment: &DecisionAssessment,
    latency_ms: u64,
) -> anyhow::Result<bool> {
    assessment
        .validate()
        .map_err(|_| anyhow::anyhow!("invalid decision assessment"))?;
    let mut tx = pool.begin().await?;
    let locked: Option<i64> = sqlx::query_scalar("select id from moderation_decision_jobs where id=$1 and instance_id=$2 and fence=$3 and status='processing' and lease_expires_at>now() for update")
        .bind(job.id).bind(&job.instance_id).bind(job.fence).fetch_optional(&mut *tx).await?;
    if locked.is_none() {
        return Ok(false);
    }
    let current = current_in_tx(&mut tx, job).await?;
    let verdict = serde_json::to_value(assessment.verdict())?;
    sqlx::query("update moderation_decision_jobs set status=$2,assessment_json=$3,shadow_verdict=$4,api_latency_ms=$5,completed_at=now(),lease_expires_at=null,error_kind=$6 where id=$1")
        .bind(job.id).bind(if current {"succeeded"} else {"stale"}).bind(serde_json::to_value(assessment)?)
        .bind(verdict.as_str()).bind(latency_ms as i64).bind(if current {None} else {Some("source_or_label_changed")}).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(current)
}

pub async fn fail(pool: &PgPool, job: &DecisionJob, error: Failure) -> anyhow::Result<bool> {
    let delay = error
        .retryable()
        .then(|| ANALYSIS_RETRY.delay_seconds(job.attempts, None))
        .flatten();
    let changed=sqlx::query("update moderation_decision_jobs set status=$4,error_kind=$5,next_attempt_at=now()+make_interval(secs=>$6::double precision),lease_expires_at=null,completed_at=case when $4='failed' then now() else null end where id=$1 and instance_id=$2 and fence=$3 and status='processing' and lease_expires_at>now()")
        .bind(job.id).bind(&job.instance_id).bind(job.fence).bind(if delay.is_some(){"retry_wait"}else{"failed"})
        .bind(error.kind()).bind(delay.unwrap_or_default() as f64).execute(pool).await?.rows_affected()>0;
    Ok(changed)
}
