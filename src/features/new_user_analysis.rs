use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use std::future::Future;
use tokio::sync::Semaphore;

use crate::{
    config::Config,
    db::membership::MembershipContext,
    features::new_user_audit::{
        deterministic,
        prompt::PROMPT_VERSION,
        repo::{
            NewUserAuditJobParams, NewUserAuditStatus, enqueue_new_user_audit_job_in_transaction,
            has_materialized_audit_for_first_message, latest_audit_link_message_id,
            latest_audit_trigger_fingerprint, record_new_user_audit_snapshot_in_transaction,
            refresh_in_flight_audit_snapshot_in_transaction,
            reopen_obsolete_materialization_in_transaction,
            supersede_unneeded_new_user_audit_jobs_in_transaction,
            sync_spam_review_from_audit_in_transaction,
        },
    },
};
use teloxide_antispam::signals::{
    NewUserAnalysisConfig, NewUserFeatures, RiskAnalysis, SpamClass, TextTexture, analyze_risk,
    char_count_i32, has_profile_photo, id_bucket, max_pairwise_message_similarity, message_style,
    message_style_persona, only_channel_post_comments, only_replies_or_comments,
    personal_channel_has_external_link, personal_channel_has_invite_link,
    telegram_id_risk_coefficient, telegram_id_spam_probability, username_stats,
};
mod audit_fingerprint;
mod audit_materializer;
mod audit_planner;
mod audit_snapshot;
pub(crate) mod profile_context;
use profile_context::looks_like_feminine_first_name;

use audit_fingerprint::{
    FingerprintCalculator, has_long_first_comment, load_latest_link_evidence,
    load_latest_long_message_evidence,
};
#[cfg(test)]
use audit_fingerprint::{
    audit_trigger_fingerprint, hex_digest, link_target_fingerprint,
    project_unified_user_audit_material_revision, project_unified_user_audit_snapshot,
};
pub(crate) use audit_fingerprint::{bounded_audit_text, bounded_audit_text_with_limit};
#[cfg(test)]
use audit_materializer::best_effort_profile_photo_dc;
use audit_materializer::save_audit_in_transaction;
#[cfg(test)]
use audit_planner::should_enqueue_new_user_audit_job;
#[cfg(test)]
use audit_planner::should_enqueue_new_user_audit_job_with_link;
use audit_planner::{
    AuditPlanner, analysis_config_from_runtime, deterministic_campaign_from_features,
    record_campaign_snapshot,
};
use audit_snapshot::SnapshotLoader;

/// Сохраняет baseline, ревизию снимка и unified-audit job одной транзакцией.
/// Между baseline и job нет окна, в котором materializer может увидеть
/// несогласованное состояние.
pub async fn enqueue_new_user_audit_for_profile_refresh(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
) -> anyhow::Result<()> {
    enqueue_new_user_audit_for_profile_refresh_with_url_feed(
        pool,
        runtime_config,
        chat_id,
        telegram_user_id,
        None,
    )
    .await
}

pub async fn enqueue_new_user_audit_for_profile_refresh_with_url_feed(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
    url_feed: Option<std::sync::Arc<crate::features::url_feeds::FeedSnapshot>>,
) -> anyhow::Result<()> {
    materialize_profile_refresh_audit(pool, runtime_config, chat_id, telegram_user_id, url_feed)
        .await
        .map(|_| ())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProfileAuditOutcome {
    Materialized,
    NoSnapshot,
}

pub(crate) async fn materialize_profile_refresh_audit(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
    url_feed: Option<std::sync::Arc<crate::features::url_feeds::FeedSnapshot>>,
) -> anyhow::Result<ProfileAuditOutcome> {
    with_url_shadow_advisory_lock(
        pool,
        &runtime_config.community.instance.id,
        chat_id,
        telegram_user_id,
        enqueue_new_user_audit_for_profile_refresh_locked(
            pool,
            runtime_config,
            chat_id,
            telegram_user_id,
            url_feed,
        ),
    )
    .await
}

async fn enqueue_new_user_audit_for_profile_refresh_locked(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
    url_feed: Option<std::sync::Arc<crate::features::url_feeds::FeedSnapshot>>,
) -> anyhow::Result<ProfileAuditOutcome> {
    let Some(snapshot) = SnapshotLoader::load(
        pool,
        chat_id,
        telegram_user_id,
        &runtime_config.community.instance.id,
    )
    .await?
    else {
        return Ok(ProfileAuditOutcome::NoSnapshot);
    };
    let features = snapshot.features;
    let membership_context = snapshot.membership_context;
    let analysis_config = analysis_config_from_runtime(runtime_config);
    let is_old_active_user = features.message_count >= analysis_config.old_user_message_threshold;
    let risk = analyze_risk(
        &profile_context::features_for_baseline(&features),
        &analysis_config,
        is_old_active_user,
    );
    let previous_avatar_file_unique_id: Option<String> = sqlx::query_scalar(
        "select profile_photo_file_unique_id from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    let avatar_changed = previous_avatar_file_unique_id.as_deref()
        != features.profile_photo_file_unique_id.as_deref();
    let latest_link_evidence = load_latest_link_evidence(pool, chat_id, telegram_user_id).await?;
    let latest_long_message_evidence =
        load_latest_long_message_evidence(pool, chat_id, telegram_user_id).await?;
    let trigger_fingerprint = FingerprintCalculator::trigger(
        &features,
        latest_link_evidence.as_ref(),
        latest_long_message_evidence.as_ref(),
    );
    let previous_trigger_fingerprint =
        latest_audit_trigger_fingerprint(pool, chat_id, telegram_user_id).await?;
    let previous_link_message_id =
        latest_audit_link_message_id(pool, chat_id, telegram_user_id).await?;
    let link_evidence_changed = latest_link_evidence
        .as_ref()
        .is_some_and(|(message_id, _)| Some(*message_id) != previous_link_message_id);
    let fresh_content_changed =
        trigger_fingerprint
            .as_ref()
            .is_some_and(|fingerprint| match previous_trigger_fingerprint.as_ref() {
                Some((previous, status, has_assessment)) if previous == fingerprint => {
                    *status == NewUserAuditStatus::Succeeded && !has_assessment
                }
                _ => true,
            });
    let first_message_preview = bounded_audit_text(features.first_message_text.as_deref());
    let has_completed_first_message_audit = has_materialized_audit_for_first_message(
        pool,
        chat_id,
        telegram_user_id,
        first_message_preview.as_deref(),
    )
    .await?;
    let plan = AuditPlanner::plan(
        &features,
        &risk,
        is_old_active_user,
        avatar_changed,
        fresh_content_changed,
        link_evidence_changed,
        !has_completed_first_message_audit && !is_old_active_user,
        trigger_fingerprint.as_deref(),
        previous_trigger_fingerprint.as_ref(),
    );
    let run_audit_job = plan.run_audit_job;
    let same_fingerprint_in_flight = plan.same_fingerprint_in_flight;
    let mut input_json = FingerprintCalculator::snapshot(&features, &risk, &analysis_config);
    input_json["membership"]["context"] = serde_json::to_value(membership_context)?;
    let url_relationship_observation =
        crate::features::url_relations::recent_link_relationship_summary(
            pool,
            runtime_config,
            chat_id,
            telegram_user_id,
            url_feed.as_deref(),
        )
        .await?;
    let url_shadow = crate::features::url_shadow_projection::UrlShadowProjection::project(
        &url_relationship_observation,
        deterministic_campaign_from_features(&features).is_some()
            || features.personal_channel_has_adult_links,
    );
    // Always materialize the current URL state, including an empty/zero result.
    // Otherwise a previously positive observation would survive after a user
    // edits or removes their only link.
    let url_shadow_fingerprint = {
        let material = serde_json::to_vec(&(
            trigger_fingerprint.as_deref(),
            latest_link_evidence.as_ref(),
            url_relationship_observation["feed_snapshot"]["revision"].clone(),
            url_relationship_observation.clone(),
        ))
        .unwrap_or_default();
        FingerprintCalculator::digest(&material)
    };
    input_json["audit_trigger_fingerprint"] = trigger_fingerprint
        .map(Value::String)
        .unwrap_or(Value::Null);
    input_json["latest_link_message_id"] = latest_link_evidence
        .as_ref()
        .map(|(message_id, _)| Value::from(*message_id))
        .unwrap_or(Value::Null);
    input_json["latest_long_message_id"] = latest_long_message_evidence
        .as_ref()
        .map(|(message_id, _)| Value::from(*message_id))
        .unwrap_or(Value::Null);
    let deterministic_campaign = deterministic_campaign_from_features(&features);
    record_campaign_snapshot(&mut input_json, deterministic_campaign.as_ref());
    let mut material_revision =
        FingerprintCalculator::material_revision(&features, &analysis_config);
    material_revision["membership"]["context"] = serde_json::to_value(membership_context)?;
    let snapshot_hash = Sha256::digest(serde_json::to_vec(&material_revision)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    let params = NewUserAuditJobParams {
        chat_id,
        telegram_user_id,
        snapshot_hash: &snapshot_hash,
        prompt_version: PROMPT_VERSION,
        input_json: &input_json,
        avatar_file_id: features.profile_photo_file_id.as_deref(),
        avatar_file_unique_id: features.profile_photo_file_unique_id.as_deref(),
        review_threshold: analysis_config.review_threshold,
    };
    let mut tx = pool.begin().await?;
    record_url_shadow_observation_in_transaction(
        &mut tx,
        chat_id,
        telegram_user_id,
        &url_shadow_fingerprint,
        url_shadow.suggested_delta,
        &url_relationship_observation,
        &serde_json::to_value(&url_shadow)?,
    )
    .await?;
    // Authoritative transactions consistently lock job, then audit, then review.
    let preserved_in_flight = if run_audit_job || same_fingerprint_in_flight {
        let reused_in_flight =
            refresh_in_flight_audit_snapshot_in_transaction(&mut tx, params).await?;
        if run_audit_job && !reused_in_flight {
            enqueue_new_user_audit_job_in_transaction(&mut tx, params).await?;
        }
        reused_in_flight
    } else {
        reopen_obsolete_materialization_in_transaction(&mut tx, params).await?;
        supersede_unneeded_new_user_audit_jobs_in_transaction(&mut tx, chat_id, telegram_user_id)
            .await?;
        false
    };
    save_audit_in_transaction(&mut tx, &features, &risk, &analysis_config, avatar_changed).await?;
    if !run_audit_job || avatar_changed {
        sync_spam_review_from_audit_in_transaction(
            &mut tx,
            chat_id,
            telegram_user_id,
            analysis_config.review_threshold,
        )
        .await?;
    }
    record_new_user_audit_snapshot_in_transaction(&mut tx, params).await?;
    tx.commit().await?;

    tracing::info!(
        chat_id,
        telegram_user_id,
        risk_score = risk.score,
        risk_level = %risk.level,
        audit_job_queued = run_audit_job && !preserved_in_flight,
        "unified new user audit baseline and job saved"
    );
    Ok(ProfileAuditOutcome::Materialized)
}

const UNIFIED_AUDIT_TEXT_LIMIT: usize = 280;
const UNIFIED_AUDIT_RECENT_MESSAGES_LIMIT: usize = 5;
const NEW_USER_LLM_AUDIT_MIN_BASELINE_SCORE: i32 = 25;
const LONG_FIRST_COMMENT_MIN_CHARS: usize = 220;
// build_pool() allows five PostgreSQL connections. Keep one available for the
// work performed while session locks are held, while still allowing different
// users to refresh concurrently.
static URL_SHADOW_LOCK_SLOTS: Semaphore = Semaphore::const_new(4);

async fn record_url_shadow_observation_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    chat_id: i64,
    telegram_user_id: i64,
    source_fingerprint: &str,
    shadow_score: i16,
    relationships: &Value,
    analysis: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"insert into public.telegram_url_shadow_observations
               (chat_id, telegram_user_id, source_fingerprint, shadow_score, evidence)
           values ($1, $2, $3, $4, $5)
           on conflict (chat_id, telegram_user_id) do update set
               source_fingerprint = excluded.source_fingerprint,
               observed_at = now(), shadow_score = excluded.shadow_score,
               evidence = excluded.evidence"#,
    )
    .bind(chat_id)
    .bind(telegram_user_id)
    .bind(source_fingerprint)
    .bind(shadow_score)
    .bind(json!({"relationships": relationships, "analysis": analysis}))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn with_url_shadow_advisory_lock<T>(
    pool: &PgPool,
    instance_id: &str,
    chat_id: i64,
    telegram_user_id: i64,
    operation: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    let _slot = URL_SHADOW_LOCK_SLOTS.acquire().await?;
    let lock_sql =
        "select pg_advisory_lock(hashtextextended($1 || ':' || $2::text || ':' || $3::text, 0))";
    let mut connection = pool.acquire().await?;
    // Session advisory locks must never return to the pool while held. If the
    // caller future is cancelled, dropping this connection closes it and
    // PostgreSQL releases the lock.
    connection.close_on_drop();
    if let Err(error) = sqlx::query(lock_sql)
        .bind(instance_id)
        .bind(chat_id)
        .bind(telegram_user_id)
        .execute(&mut *connection)
        .await
    {
        let _ = connection.close().await;
        return Err(error.into());
    }

    let result = operation.await;
    let unlocked: Result<bool, sqlx::Error> = sqlx::query_scalar(
        "select pg_advisory_unlock(hashtextextended($1 || ':' || $2::text || ':' || $3::text, 0))",
    )
    .bind(instance_id)
    .bind(chat_id)
    .bind(telegram_user_id)
    .fetch_one(&mut *connection)
    .await;

    match (result, unlocked) {
        (Ok(value), Ok(true)) => Ok(value),
        (Err(error), Ok(true)) => Err(error),
        (Ok(_), Ok(false)) => anyhow::bail!("URL shadow advisory lock was not held at release"),
        (Err(error), Ok(false)) => {
            Err(error.context("URL shadow advisory lock was not held at release"))
        }
        (Ok(_), Err(error)) => {
            let _ = connection.close().await;
            Err(error.into())
        }
        (Err(error), Err(unlock_error)) => {
            let _ = connection.close().await;
            Err(error.context(format!("failed to release URL shadow lock: {unlock_error}")))
        }
    }
}

/// Recomputes only the URL shadow projection. It performs bounded PostgreSQL
/// reads and never enqueues or invokes the LLM audit.
pub async fn refresh_url_shadow_observation_from_links(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
    url_feed: Option<&crate::features::url_feeds::FeedSnapshot>,
) -> anyhow::Result<()> {
    with_url_shadow_advisory_lock(
        pool,
        &runtime_config.community.instance.id,
        chat_id,
        telegram_user_id,
        refresh_url_shadow_observation_from_links_locked(
            pool,
            runtime_config,
            chat_id,
            telegram_user_id,
            url_feed,
        ),
    )
    .await
}

async fn refresh_url_shadow_observation_from_links_locked(
    pool: &PgPool,
    runtime_config: &Config,
    chat_id: i64,
    telegram_user_id: i64,
    url_feed: Option<&crate::features::url_feeds::FeedSnapshot>,
) -> anyhow::Result<()> {
    let latest_link_evidence = load_latest_link_evidence(pool, chat_id, telegram_user_id).await?;
    let relationships = crate::features::url_relations::recent_link_relationship_summary(
        pool,
        runtime_config,
        chat_id,
        telegram_user_id,
        url_feed,
    )
    .await?;
    let promotional_content = SnapshotLoader::load(
        pool,
        chat_id,
        telegram_user_id,
        &runtime_config.community.instance.id,
    )
    .await?
    .is_some_and(|snapshot| {
        let features = snapshot.features;
        deterministic_campaign_from_features(&features).is_some()
            || features.personal_channel_has_adult_links
    });
    let analysis = crate::features::url_shadow_projection::UrlShadowProjection::project(
        &relationships,
        promotional_content,
    );
    let source_fingerprint = FingerprintCalculator::digest(&serde_json::to_vec(&(
        "url-shadow-links-v2",
        latest_link_evidence,
        relationships["feed_snapshot"]["revision"].clone(),
        relationships.clone(),
        promotional_content,
    ))?);
    let mut tx = pool.begin().await?;
    record_url_shadow_observation_in_transaction(
        &mut tx,
        chat_id,
        telegram_user_id,
        &source_fingerprint,
        analysis.suggested_delta,
        &relationships,
        &serde_json::to_value(&analysis)?,
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use teloxide_antispam::signals::analyze_new_or_low_activity_user;

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn url_shadow_lock_serializes_same_user_without_blocking_other_users() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        const CHAT_ID: i64 = -100_991_223_499;
        const USER_ID: i64 = 9_912_299;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let same_user_entered = Arc::new(AtomicBool::new(false));
        let other_user_entered = Arc::new(AtomicBool::new(false));

        let first_pool = pool.clone();
        let first_entered = entered.clone();
        let first_release = release.clone();
        let first = tokio::spawn(async move {
            with_url_shadow_advisory_lock(
                &first_pool,
                "url-shadow-lock-test",
                CHAT_ID,
                USER_ID,
                async move {
                    first_entered.notify_one();
                    first_release.notified().await;
                    Ok(())
                },
            )
            .await
        });
        entered.notified().await;

        let second_pool = pool.clone();
        let second_entered = same_user_entered.clone();
        let second = tokio::spawn(async move {
            with_url_shadow_advisory_lock(
                &second_pool,
                "url-shadow-lock-test",
                CHAT_ID,
                USER_ID,
                async move {
                    second_entered.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!same_user_entered.load(Ordering::SeqCst));

        let other_pool = pool.clone();
        let other_entered = other_user_entered.clone();
        with_url_shadow_advisory_lock(
            &other_pool,
            "url-shadow-lock-test",
            CHAT_ID,
            USER_ID + 1,
            async move {
                other_entered.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(other_user_entered.load(Ordering::SeqCst));

        release.notify_one();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert!(same_user_entered.load(Ordering::SeqCst));
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn cancelled_url_shadow_refresh_releases_its_session_lock() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let task_pool = pool.clone();
        let task_entered = entered.clone();
        let task = tokio::spawn(async move {
            with_url_shadow_advisory_lock(
                &task_pool,
                "url-shadow-cancel-test",
                -100_991_223_501,
                9_912_301,
                async move {
                    task_entered.notify_one();
                    std::future::pending::<anyhow::Result<()>>().await
                },
            )
            .await
        });

        entered.notified().await;
        task.abort();
        let _ = task.await;

        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            with_url_shadow_advisory_lock(
                &pool,
                "url-shadow-cancel-test",
                -100_991_223_501,
                9_912_301,
                async { Ok(()) },
            ),
        )
        .await
        .expect("cancelled refresh left its session advisory lock held")
        .unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn url_shadow_observations_are_separate_and_private() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        const CHAT_ID: i64 = -100_991_223_441;
        const USER_ID: i64 = 9_912_234;
        let fingerprint = "a".repeat(64);
        sqlx::query("delete from public.telegram_url_shadow_observations where chat_id = $1 and telegram_user_id = $2")
            .bind(CHAT_ID)
            .bind(USER_ID)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        record_url_shadow_observation_in_transaction(
            &mut tx,
            CHAT_ID,
            USER_ID,
            &fingerprint,
            16,
            &json!({"external_web_url_count": 1}),
            &json!({"signals": [{"label": "promotion_with_external_url"}]}),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        let stored: (i16, Value) = sqlx::query_as(
            "select shadow_score, evidence from public.telegram_url_shadow_observations where chat_id = $1 and telegram_user_id = $2 and source_fingerprint = $3",
        )
        .bind(CHAT_ID)
        .bind(USER_ID)
        .bind(&fingerprint)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(stored.0, 16);
        assert_eq!(stored.1["relationships"]["external_web_url_count"], 1);
        let can_read: bool = sqlx::query_scalar(
            "select has_table_privilege('nedobot_mcp_ro', 'public.telegram_url_shadow_observations', 'select')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!can_read);
        sqlx::query("delete from public.telegram_url_shadow_observations where chat_id = $1 and telegram_user_id = $2")
            .bind(CHAT_ID)
            .bind(USER_ID)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    #[tokio::test]
    #[ignore = "requires a disposable migrated PostgreSQL database"]
    async fn qr_only_evidence_refreshes_shadow_and_removed_links_clear_positive_state() {
        let database_url = std::env::var("TEST_DATABASE_URL")
            .expect("TEST_DATABASE_URL must point at a disposable test database");
        let pool = PgPool::connect(&database_url).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        const CHAT_ID: i64 = -100_991_223_442;
        const USER_ID: i64 = 9_912_235;
        const MESSAGE_ID: i32 = 9_123_001;
        const QR_URL: &str = "https://qr-threat.example/payload";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let feed = crate::features::url_feeds::FeedSnapshot::from_lines(
            "fixture",
            "CC0",
            "qr-test-revision",
            now,
            3600,
            crate::features::url_feeds::MatchScope::ExactUrl,
            QR_URL,
        )
        .unwrap();
        let mut target_hash = Sha256::new();
        target_hash.update(b"nedobot-url-evidence-v1\0url\0");
        target_hash.update(QR_URL);
        let target_hash = target_hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(feed.contains_evidence_hash(&target_hash, now));
        sqlx::query("delete from telegram_url_shadow_observations where chat_id = $1 and telegram_user_id = $2")
            .bind(CHAT_ID).bind(USER_ID).execute(&pool).await.unwrap();
        sqlx::query("delete from telegram_messages where chat_id = $1 and message_id = $2")
            .bind(CHAT_ID)
            .bind(MESSAGE_ID)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("insert into telegram_chat_users(chat_id,telegram_user_id,first_seen_at,message_count) values($1,$2,now(),1) on conflict do nothing")
            .bind(CHAT_ID).bind(USER_ID).execute(&pool).await.unwrap();
        sqlx::query("insert into telegram_messages(chat_id,message_id,user_id,text,raw_json,created_at) values($1,$2,$3,null,'{}',now())")
            .bind(CHAT_ID).bind(MESSAGE_ID).bind(USER_ID).execute(&pool).await.unwrap();
        sqlx::query("insert into telegram_message_url_evidence(chat_id,message_id,parser_version,item_index,kind,source,target_sha256,host,display_hint,privacy_sensitive,qr_relation) values($1,$2,$3,0,'url','qr',$4,'example.invalid','https://example.invalid/…',true,'external')")
            .bind(CHAT_ID).bind(MESSAGE_ID).bind(crate::db::url_evidence::URL_EVIDENCE_PARSER_VERSION)
            .bind(&target_hash).execute(&pool).await.unwrap();
        sqlx::query("insert into telegram_message_url_evidence(chat_id,message_id,parser_version,item_index,kind,source,target_sha256,display_hint,privacy_sensitive,qr_relation) values($1,$2,$3,1,'url','qr',$4,'https://t.me/…',true,'affiliated')")
            .bind(CHAT_ID).bind(MESSAGE_ID).bind(crate::db::url_evidence::URL_EVIDENCE_PARSER_VERSION)
            .bind("0".repeat(64)).execute(&pool).await.unwrap();
        // The same canonical URL in text and QR is one target for scoring.
        sqlx::query("insert into telegram_message_url_evidence(chat_id,message_id,parser_version,item_index,kind,source,target_sha256,host,display_hint,privacy_sensitive) values($1,$2,$3,0,'url','visible_text',$4,'example.invalid','https://example.invalid/…',true)")
            .bind(CHAT_ID).bind(MESSAGE_ID).bind(crate::db::url_evidence::URL_EVIDENCE_PARSER_VERSION)
            .bind(target_hash).execute(&pool).await.unwrap();

        let mut config = Config::from_env().unwrap();
        config.community.instance.id = "url_shadow_fixture".into();
        let chat = config.community.chats.get_mut("main").unwrap();
        chat.id = CHAT_ID;
        chat.moderation = true;
        config.chat_registry = crate::community::ChatRegistry::new(&config.community).unwrap();
        let (refresh_a, refresh_b) = tokio::join!(
            refresh_url_shadow_observation_from_links(
                &pool,
                &config,
                CHAT_ID,
                USER_ID,
                Some(&feed)
            ),
            refresh_url_shadow_observation_from_links(
                &pool,
                &config,
                CHAT_ID,
                USER_ID,
                Some(&feed)
            )
        );
        refresh_a.unwrap();
        refresh_b.unwrap();
        let first: (i16, Value, String) = sqlx::query_as(
            "select shadow_score,evidence,source_fingerprint::text from telegram_url_shadow_observations where chat_id=$1 and telegram_user_id=$2",
        )
        .bind(CHAT_ID)
        .bind(USER_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(first.0, 24);
        assert_eq!(first.1["relationships"]["unique_external_url_count"], 1);
        assert_eq!(first.1["relationships"]["external_qr_url_count"], 1);
        assert!(
            load_latest_link_evidence(&pool, CHAT_ID, USER_ID)
                .await
                .unwrap()
                .is_some()
        );

        sqlx::query("delete from telegram_message_url_evidence where chat_id=$1 and message_id=$2")
            .bind(CHAT_ID)
            .bind(MESSAGE_ID)
            .execute(&pool)
            .await
            .unwrap();
        refresh_url_shadow_observation_from_links(&pool, &config, CHAT_ID, USER_ID, Some(&feed))
            .await
            .unwrap();
        let cleared: (i16, Value, String) = sqlx::query_as(
            "select shadow_score,evidence,source_fingerprint::text from telegram_url_shadow_observations where chat_id=$1 and telegram_user_id=$2",
        )
        .bind(CHAT_ID)
        .bind(USER_ID)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(cleared.0, 0);
        assert_eq!(cleared.1["relationships"]["external_qr_url_count"], 0);
        assert_ne!(first.2, cleared.2);
        assert!(
            load_latest_link_evidence(&pool, CHAT_ID, USER_ID)
                .await
                .unwrap()
                .is_none()
        );

        sqlx::query(
            "delete from telegram_url_shadow_observations where chat_id=$1 and telegram_user_id=$2",
        )
        .bind(CHAT_ID)
        .bind(USER_ID)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("delete from telegram_chat_users where chat_id=$1 and telegram_user_id=$2")
            .bind(CHAT_ID)
            .bind(USER_ID)
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
    }

    #[test]
    fn hidden_link_target_changes_fingerprint_without_leaking_target() {
        let first = link_target_fingerprint(Some("нажми сюда"), vec!["a".repeat(64)]);
        let same = link_target_fingerprint(Some("нажми сюда"), vec!["a".repeat(64)]);
        let edited = link_target_fingerprint(Some("нажми сюда"), vec!["b".repeat(64)]);

        assert_eq!(first, same);
        assert_ne!(first, edited);
        assert!(first.starts_with("url-evidence-v2:"));
        assert!(!first.contains("SYNTHETIC_SECRET"));
    }

    #[test]
    fn hidden_entity_target_edit_changes_audit_fingerprint_at_same_anchor() {
        fn message(target: &str) -> teloxide::types::Message {
            serde_json::from_value(json!({
                "message_id": 7,
                "date": 1_700_000_000,
                "chat": {"id": -1001, "type": "supergroup"},
                "from": {"id": 42, "is_bot": false, "first_name": "Test"},
                "text": "нажми сюда",
                "entities": [{
                    "type": "text_link", "offset": 0, "length": 10, "url": target
                }]
            }))
            .unwrap()
        }

        let first = message("https://t.me/+SYNTHETIC_SECRET_A");
        let edited = message("https://t.me/+SYNTHETIC_SECRET_B");
        let before = link_target_fingerprint(first.text(), vec![hex_digest(b"SYNTHETIC_SECRET_A")]);
        let after = link_target_fingerprint(edited.text(), vec![hex_digest(b"SYNTHETIC_SECRET_B")]);

        assert_ne!(before, after);
        assert!(!before.contains("SYNTHETIC_SECRET"));
        assert!(!after.contains("SYNTHETIC_SECRET"));
    }

    fn risk_with_score(score: i32) -> RiskAnalysis {
        RiskAnalysis {
            score,
            level: if score >= 70 { "high" } else { "low" }.to_string(),
            primary_class: None,
            class_scores: json!({}),
            labels: Vec::new(),
            reasons: Vec::new(),
            signals: json!([]),
        }
    }

    #[test]
    fn audit_gate_skips_mature_profile_refreshes_but_keeps_fresh_signals() {
        let config = NewUserAnalysisConfig::default();
        let ordinary = NewUserFeatures {
            message_count: 2,
            first_message_text: Some("Привет всем".to_string()),
            ..Default::default()
        };
        assert!(!should_enqueue_new_user_audit_job(
            &ordinary,
            &risk_with_score(12),
            false,
            false,
            false,
            false,
        ));
        assert!(should_enqueue_new_user_audit_job(
            &ordinary,
            &risk_with_score(25),
            false,
            false,
            false,
            false,
        ));
        assert!(should_enqueue_new_user_audit_job(
            &NewUserFeatures {
                message_count: 2,
                ..ordinary.clone()
            },
            &risk_with_score(12),
            false,
            false,
            false,
            true,
        ));
        assert!(should_enqueue_new_user_audit_job(
            &ordinary,
            &risk_with_score(12),
            false,
            true,
            false,
            false,
        ));

        let profile_photo = NewUserFeatures {
            profile_photo_count: Some(1),
            ..ordinary.clone()
        };
        assert!(should_enqueue_new_user_audit_job(
            &profile_photo,
            &risk_with_score(12),
            false,
            true,
            false,
            false,
        ));

        let mature = NewUserFeatures {
            message_count: config.old_user_message_threshold,
            profile_photo_count: Some(1),
            ..ordinary
        };
        assert!(!should_enqueue_new_user_audit_job(
            &mature,
            &risk_with_score(12),
            true,
            false,
            false,
            false,
        ));
        assert!(should_enqueue_new_user_audit_job(
            &mature,
            &risk_with_score(12),
            true,
            true,
            false,
            false,
        ));

        assert!(should_enqueue_new_user_audit_job(
            &mature,
            &risk_with_score(12),
            true,
            false,
            true,
            false,
        ));

        let mature_with_long_recent_text = NewUserFeatures {
            recent_message_texts: vec!["текст".repeat(LONG_FIRST_COMMENT_MIN_CHARS / 5 + 1)],
            ..mature.clone()
        };
        assert!(should_enqueue_new_user_audit_job(
            &mature_with_long_recent_text,
            &risk_with_score(12),
            true,
            false,
            true,
            false,
        ));
    }

    #[test]
    fn audit_gate_keeps_long_comments_and_deterministic_campaign_fast_paths() {
        for (length, expected) in [
            (LONG_FIRST_COMMENT_MIN_CHARS - 1, false),
            (LONG_FIRST_COMMENT_MIN_CHARS, true),
        ] {
            let features = NewUserFeatures {
                message_count: 2,
                first_message_text: Some("я".repeat(length)),
                ..Default::default()
            };
            assert_eq!(
                should_enqueue_new_user_audit_job(
                    &features,
                    &risk_with_score(12),
                    false,
                    false,
                    expected,
                    false,
                ),
                expected,
                "comment length {length}"
            );
        }

        let paid_dm_campaign = NewUserFeatures {
            message_count: 1,
            chat_age_sec: Some(60),
            first_message_text: Some("Кидай + в директ — с меня 400 ₽".to_string()),
            ..Default::default()
        };
        assert!(should_enqueue_new_user_audit_job(
            &paid_dm_campaign,
            &risk_with_score(12),
            false,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn trigger_fingerprint_uses_stable_message_ids_for_link_and_long_text_evidence() {
        let historical_long_message = NewUserFeatures {
            recent_message_texts: vec![
                "short one".into(),
                "short two".into(),
                "short three".into(),
                "short four".into(),
                "short five".into(),
                "old ".to_string() + &"x".repeat(LONG_FIRST_COMMENT_MIN_CHARS),
            ],
            ..Default::default()
        };
        assert_eq!(
            audit_trigger_fingerprint(&historical_long_message, None, None),
            None
        );

        let long_evidence = (20, Some("x".repeat(LONG_FIRST_COMMENT_MIN_CHARS)));
        let long_fingerprint =
            audit_trigger_fingerprint(&historical_long_message, None, Some(&long_evidence));
        assert_eq!(
            audit_trigger_fingerprint(&NewUserFeatures::default(), None, Some(&long_evidence)),
            long_fingerprint
        );

        let link_message = NewUserFeatures {
            recent_message_texts: vec![
                "Промо https://example.org/a".into(),
                "обычный комментарий".into(),
            ],
            ..Default::default()
        };
        let link_evidence = (19, Some("promo".into()));
        let original =
            audit_trigger_fingerprint(&link_message, Some(&link_evidence), None).unwrap();
        let after_ordinary_message = NewUserFeatures {
            recent_message_texts: vec![
                "ещё обычный комментарий".into(),
                link_message.recent_message_texts[0].clone(),
                link_message.recent_message_texts[1].clone(),
            ],
            link_count_24h: 1,
            ..Default::default()
        };
        assert_eq!(
            audit_trigger_fingerprint(&after_ordinary_message, Some(&link_evidence), None),
            Some(original)
        );

        let latest_link_evidence = (21, Some("visible anchor text".into()));
        let hidden_link = NewUserFeatures::default();
        let hidden_link_fingerprint =
            audit_trigger_fingerprint(&hidden_link, Some(&latest_link_evidence), None).unwrap();
        assert_eq!(
            audit_trigger_fingerprint(&hidden_link, Some(&latest_link_evidence), None),
            Some(hidden_link_fingerprint.clone())
        );
        assert_ne!(
            Some(hidden_link_fingerprint),
            audit_trigger_fingerprint(
                &hidden_link,
                Some(&(22, Some("visible anchor text".into()))),
                None,
            )
        );
    }

    #[test]
    fn mature_channel_link_evidence_triggers_only_when_its_fingerprint_changes() {
        let mature = NewUserFeatures {
            message_count: NewUserAnalysisConfig::default().old_user_message_threshold,
            personal_channel_chat_id: Some(-100_123),
            personal_channel_username: Some("example_channel".into()),
            personal_channel_last_message_id: Some(10),
            personal_channel_last_text: Some("Смотрите https://example.org".into()),
            ..Default::default()
        };
        let first_fingerprint = audit_trigger_fingerprint(&mature, None, None);
        assert!(first_fingerprint.is_some());
        assert!(should_enqueue_new_user_audit_job(
            &mature,
            &risk_with_score(12),
            true,
            false,
            true,
            false,
        ));
        assert!(!should_enqueue_new_user_audit_job(
            &mature,
            &risk_with_score(12),
            true,
            false,
            false,
            false,
        ));
        let changed = NewUserFeatures {
            personal_channel_last_message_id: Some(11),
            ..mature
        };
        assert_ne!(
            first_fingerprint,
            audit_trigger_fingerprint(&changed, None, None)
        );
    }

    #[test]
    fn a_new_short_link_triggers_audit_for_a_low_activity_user() {
        let features = NewUserFeatures {
            message_count: 4,
            ..Default::default()
        };
        assert!(should_enqueue_new_user_audit_job_with_link(
            &features,
            &risk_with_score(18),
            false,
            false,
            false,
            true,
            false,
        ));
        assert!(!should_enqueue_new_user_audit_job_with_link(
            &features,
            &risk_with_score(18),
            false,
            false,
            false,
            false,
            false,
        ));
    }

    #[test]
    fn latest_personal_channel_content_is_a_redacted_material_audit_input() {
        let features = NewUserFeatures {
            personal_channel_chat_id: Some(-100_000_000_001),
            personal_channel_last_text: Some(
                "Пишите в личку за VPN: https://example.org/promo".to_string(),
            ),
            recent_message_texts: vec!["Пишите в личку за VPN".to_string()],
            ..Default::default()
        };
        let config = NewUserAnalysisConfig::default();
        let risk = analyze_new_or_low_activity_user(&features, &config);
        let snapshot = project_unified_user_audit_snapshot(&features, &risk, &config);
        let revision = project_unified_user_audit_material_revision(&features, &config);

        assert_eq!(
            snapshot["personal_channel"]["recent_content_preview"],
            "Пишите в личку за VPN: [link]"
        );
        assert_eq!(
            revision["personal_channel"]["recent_content_preview"],
            snapshot["personal_channel"]["recent_content_preview"]
        );

        let changed_features = NewUserFeatures {
            personal_channel_last_text: Some("Обычный пост о книгах".to_string()),
            ..features.clone()
        };
        assert_ne!(
            revision,
            project_unified_user_audit_material_revision(&changed_features, &config)
        );

        let changed_message = NewUserFeatures {
            recent_message_texts: vec!["Есть вариант заработать, пиши в лс".to_string()],
            ..features.clone()
        };
        assert_ne!(
            revision,
            project_unified_user_audit_material_revision(&changed_message, &config)
        );
    }

    #[test]
    fn local_spam_label_changes_the_canonical_audit_revision() {
        let config = NewUserAnalysisConfig::default();
        let features = NewUserFeatures::default();
        let original = project_unified_user_audit_material_revision(&features, &config);
        let labeled = NewUserFeatures {
            local_spammer_identity: true,
            ..features.clone()
        };
        let revision = project_unified_user_audit_material_revision(&labeled, &config);
        assert_ne!(original, revision);
        assert_eq!(revision["profile"]["local_spammer_identity"], true);
        assert_eq!(
            original,
            project_unified_user_audit_material_revision(&features, &config)
        );
    }

    #[test]
    fn canonical_analysis_matches_golden_score_and_snapshot_revisions() {
        let features = NewUserFeatures {
            chat_id: -100_991_235,
            telegram_user_id: 42,
            username: Some("reader_42".to_string()),
            display_name: Some("Synthetic Reader".to_string()),
            first_message_text: Some("Добрый день https://t.me/example".to_string()),
            recent_message_texts: vec!["Добрый день https://t.me/example".to_string()],
            member_status: Some("member".to_string()),
            ..Default::default()
        };
        let config = NewUserAnalysisConfig::default();
        let risk = analyze_new_or_low_activity_user(&features, &config);
        let snapshot = project_unified_user_audit_snapshot(&features, &risk, &config);
        let revision = project_unified_user_audit_material_revision(&features, &config);
        let snapshot_hash = hex_digest(&serde_json::to_vec(&snapshot).unwrap());
        let revision_hash = hex_digest(&serde_json::to_vec(&revision).unwrap());

        assert_eq!(risk.score, 18);
        assert_eq!(
            snapshot_hash,
            "eb7a70a8a9dfa6af5d1d466004266634e983f0b8fed56565e79730d71c7d587a"
        );
        assert_eq!(
            revision_hash,
            "8e0f519a2d64ad8af10cbc7e1a07c939e4611753a2582abe3e1c2b698d6d2e11"
        );
        assert_eq!(snapshot["profile"]["username"], "reader_42");
        assert_eq!(
            snapshot["text"]["first_message_preview"],
            "Добрый день [link]"
        );
    }

    #[test]
    fn dc_parser_does_not_guess_without_verified_decoder() {
        let result = best_effort_profile_photo_dc(Some("AQADBAADb6sxG4x8AAEC"));
        assert!(result.dc_id.is_none());
        assert!(result.note.contains("dc_unavailable_from_bot_api"));
    }

    #[test]
    fn bounded_audit_text_redacts_links_and_normalizes_whitespace() {
        assert_eq!(
            bounded_audit_text(Some("  promo https://t.me/+secret\nnext  ")),
            Some("promo [link] next".to_string())
        );
        assert_eq!(
            bounded_audit_text(Some("(telegram.me/+secret)")),
            Some("[link]".to_string())
        );
    }

    #[test]
    fn bounded_audit_text_truncates_by_characters() {
        let input = "я".repeat(UNIFIED_AUDIT_TEXT_LIMIT + 1);
        let preview = bounded_audit_text(Some(&input)).expect("non-empty preview");

        assert_eq!(preview.chars().count(), UNIFIED_AUDIT_TEXT_LIMIT + 1);
        assert!(preview.ends_with('…'));
    }
}

#[cfg(test)]
mod work_review_compatibility_tests {
    use super::*;
    #[test]
    fn later_paid_engagement_offer_enqueues_without_legacy_campaign_authority() {
        let features = NewUserFeatures {
            first_message_text: Some("Всем привет".into()),
            recent_message_texts: vec!["плачу за отзывы, 10к за 10 отзывов💵😉".into()],
            message_count: 4,
            chat_age_sec: Some(3600),
            ..NewUserFeatures::default()
        };
        let campaign = deterministic_campaign_from_features(&features).unwrap();
        assert_eq!(
            campaign.kind,
            deterministic::CampaignKind::PaidEngagementOffer
        );
        assert!(!campaign.auto_ban_eligible);
        assert!(!campaign.can_short_circuit_audit());
        let mut snapshot = json!({});
        record_campaign_snapshot(&mut snapshot, Some(&campaign));
        assert_eq!(
            snapshot["work_contact_review"]["kind"],
            "paid_engagement_offer"
        );
        assert!(snapshot["deterministic_campaign_rule"].is_null());
        assert!(snapshot["deterministic_campaign_evidence"].is_null());
        let risk = analyze_risk(&features, &NewUserAnalysisConfig::default(), false);
        assert!(should_enqueue_new_user_audit_job(
            &features, &risk, false, false, false, false
        ));
    }

    #[test]
    fn weak_work_signal_does_not_replace_existing_llm_or_suppress_its_captcha() {
        let work =
            deterministic::classify(Some("Есть подработка, пиши мне"), 1, Some(0), false).unwrap();
        let mut snapshot = json!({});
        record_campaign_snapshot(&mut snapshot, Some(&work));
        assert_eq!(snapshot["work_contact_review"]["auto_ban_eligible"], false);
        assert!(snapshot["deterministic_campaign_rule"].is_null());
        assert!(snapshot["deterministic_campaign_evidence"].is_null());
        assert!(!work.can_short_circuit_audit());
        let old =
            deterministic::classify(Some("Нужны срочно деньги??? Пиши мне!"), 1, Some(0), false)
                .unwrap();
        record_campaign_snapshot(&mut snapshot, Some(&old));
        assert!(snapshot["deterministic_campaign_rule"].is_string());
    }
}
