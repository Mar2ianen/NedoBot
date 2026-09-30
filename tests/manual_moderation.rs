use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::sync::atomic::{AtomicI32, Ordering};
use tg_ai_bot_teloxide::features::manual_moderation::{
    ActionPreparation, BatchClaim, UndoClaim, WarningSelection, add_warning, add_warnings_batch,
    claim_batch, claim_restriction_revokes_batch, claim_undo_action, create_batch,
    create_batch_with_request, finish_batch, latest_batch, list_actions, mark_action_failed,
    mark_action_succeeded, prepare_action, prepare_actions_batch, recover_expired_batch_actions,
    revoke_warnings, start_prepared_action,
    types::{BatchRequestSnapshot, CommandKind},
};

const CHAT_ID: i64 = -1001932061163;
static MESSAGE_ID: AtomicI32 = AtomicI32::new(1_400_000_000);

#[tokio::test]
#[ignore = "run against the disposable local test database"]
async fn warning_threshold_escalates_once_and_failed_replacement_keeps_existing_restriction() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to the disposable test database");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("test PostgreSQL must be reachable");
    let user_id = Utc::now().timestamp_micros() + 2_000_000_000;
    let actor_id = user_id + 1;
    let mut batch_ids = Vec::new();

    for warning_number in 1..=3 {
        let batch = create_batch(
            &pool,
            CHAT_ID,
            actor_id,
            MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
            "warn",
        )
        .await
        .expect("warning batch must be recorded");
        batch_ids.push(batch.id);
        let warning = add_warning(
            &pool,
            batch.id,
            CHAT_ID,
            user_id,
            actor_id,
            std::time::Duration::from_secs(30 * 24 * 60 * 60),
            None,
        )
        .await
        .expect("warning must be appended");
        assert_eq!(warning.active_count, warning_number);
        assert_eq!(warning.should_escalate, warning_number == 3);

        if warning.should_escalate {
            let action_id = prepare_action(
                &pool,
                ActionPreparation {
                    batch_id: batch.id,
                    chat_id: CHAT_ID,
                    target_user_id: user_id,
                    actor_user_id: actor_id,
                    action: "auto_mute",
                    reason: Some("автоматически: три активных предупреждения"),
                    expires_at: Some(Utc::now() + Duration::days(5)),
                    automatic: true,
                },
            )
            .await
            .expect("automatic mute must be claimable")
            .expect("automatic mute is required exactly at threshold");
            mark_action_succeeded(&pool, action_id)
                .await
                .expect("automatic mute must become active");
        }
    }

    let fourth_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "warn",
    )
    .await
    .expect("fourth warning batch must be recorded");
    batch_ids.push(fourth_batch.id);
    let fourth = add_warning(
        &pool,
        fourth_batch.id,
        CHAT_ID,
        user_id,
        actor_id,
        std::time::Duration::from_secs(30 * 24 * 60 * 60),
        None,
    )
    .await
    .expect("fourth warning must be appended");
    assert_eq!(fourth.active_count, 4);
    assert!(
        !fourth.should_escalate,
        "an active five-day mute is not extended by later warns"
    );

    let restrictions = list_actions(&pool, CHAT_ID, Some(user_id), 20)
        .await
        .expect("moderation history must be queryable");
    assert_eq!(
        restrictions
            .iter()
            .filter(|action| action.action == "auto_mute" && action.status == "applied")
            .count(),
        1
    );

    let replacement_user_id = user_id + 10;
    let mute_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "mute",
    )
    .await
    .expect("mute batch must be recorded");
    batch_ids.push(mute_batch.id);
    let mute_action_id = prepare_action(
        &pool,
        ActionPreparation {
            batch_id: mute_batch.id,
            chat_id: CHAT_ID,
            target_user_id: replacement_user_id,
            actor_user_id: actor_id,
            action: "mute",
            reason: None,
            expires_at: Some(Utc::now() + Duration::days(1)),
            automatic: false,
        },
    )
    .await
    .expect("mute must be claimable")
    .expect("mute action must be created");
    mark_action_succeeded(&pool, mute_action_id)
        .await
        .expect("mute must become active");

    let ban_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "ban",
    )
    .await
    .expect("ban batch must be recorded");
    batch_ids.push(ban_batch.id);
    let ban_action_id = prepare_action(
        &pool,
        ActionPreparation {
            batch_id: ban_batch.id,
            chat_id: CHAT_ID,
            target_user_id: replacement_user_id,
            actor_user_id: actor_id,
            action: "ban",
            reason: None,
            expires_at: None,
            automatic: false,
        },
    )
    .await
    .expect("ban must be claimable");
    let Some(ban_action_id) = ban_action_id else {
        panic!("ban action must be created")
    };
    mark_action_failed(&pool, ban_action_id, false, "simulated API rejection")
        .await
        .expect("ban failure must be recorded");

    let replacement_history = list_actions(&pool, CHAT_ID, Some(replacement_user_id), 20)
        .await
        .expect("replacement history must be queryable");
    assert!(
        replacement_history
            .iter()
            .any(|action| action.id == mute_action_id && action.status == "applied")
    );
    assert!(
        replacement_history
            .iter()
            .any(|action| action.id == ban_action_id && action.status == "failed")
    );

    let undo_user_id = user_id + 20;
    let warning_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "warn",
    )
    .await
    .expect("warning batch must be recorded");
    batch_ids.push(warning_batch.id);
    let warning = add_warning(
        &pool,
        warning_batch.id,
        CHAT_ID,
        undo_user_id,
        actor_id,
        std::time::Duration::from_secs(30 * 24 * 60 * 60),
        None,
    )
    .await
    .expect("warning must be appended");
    let unwarn_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "unwarn",
    )
    .await
    .expect("unwarn batch must be recorded");
    batch_ids.push(unwarn_batch.id);
    assert_eq!(
        revoke_warnings(
            &pool,
            unwarn_batch.id,
            CHAT_ID,
            undo_user_id,
            actor_id,
            WarningSelection::Latest,
            None,
        )
        .await
        .expect("warning must be revocable"),
        [warning.action_id]
    );
    assert_eq!(
        latest_batch(&pool, CHAT_ID, actor_id)
            .await
            .expect("latest batch must be queryable"),
        Some(unwarn_batch.id)
    );
    let undo_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "undo",
    )
    .await
    .expect("undo batch must be recorded");
    batch_ids.push(undo_batch.id);
    assert_eq!(
        claim_undo_action(
            &pool,
            undo_batch.id,
            unwarn_batch.id,
            warning.action_id,
            actor_id,
        )
        .await
        .expect("undo must restore the warning"),
        UndoClaim::WarningRestored
    );
    assert_eq!(
        claim_undo_action(
            &pool,
            undo_batch.id,
            unwarn_batch.id,
            warning.action_id,
            actor_id,
        )
        .await
        .expect("repeated undo must be safe"),
        UndoClaim::Conflict
    );

    cleanup(
        &pool,
        &[user_id, replacement_user_id, undo_user_id],
        &batch_ids,
    )
    .await;
}

async fn cleanup(pool: &PgPool, user_ids: &[i64], batch_ids: &[i64]) {
    sqlx::query("delete from manual_moderation_events where batch_id = any($1)")
        .bind(batch_ids)
        .execute(pool)
        .await
        .expect("test events must be removed");
    sqlx::query("delete from manual_moderation_actions where batch_id = any($1)")
        .bind(batch_ids)
        .execute(pool)
        .await
        .expect("test actions must be removed");
    sqlx::query("delete from manual_moderation_batches where id = any($1)")
        .bind(batch_ids)
        .execute(pool)
        .await
        .expect("test batches must be removed");
    let _ = user_ids;
}

#[tokio::test]
#[ignore = "run against the disposable local test database"]
async fn moderation_batch_db_preflight_is_atomic_and_duplicate_updates_resume_safely() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to the disposable test database");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("test PostgreSQL must be reachable");
    let user_id = Utc::now().timestamp_micros() + 3_000_000_000;
    let actor_id = user_id + 1;
    let first_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "mute",
    )
    .await
    .unwrap();
    let blocker_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "mute",
    )
    .await
    .unwrap();
    let warning_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "warn",
    )
    .await
    .unwrap();
    let recoverable_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "ban",
    )
    .await
    .unwrap();
    let mut batch_ids = vec![
        first_batch.id,
        blocker_batch.id,
        warning_batch.id,
        recoverable_batch.id,
    ];

    let blocker_id = prepare_action(
        &pool,
        ActionPreparation {
            batch_id: blocker_batch.id,
            chat_id: CHAT_ID,
            target_user_id: user_id + 1,
            actor_user_id: actor_id,
            action: "mute",
            reason: None,
            expires_at: Some(Utc::now() + Duration::days(1)),
            automatic: false,
        },
    )
    .await
    .unwrap()
    .unwrap();

    let batch_preflight = prepare_actions_batch(
        &pool,
        &[
            ActionPreparation {
                batch_id: first_batch.id,
                chat_id: CHAT_ID,
                target_user_id: user_id,
                actor_user_id: actor_id,
                action: "mute",
                reason: None,
                expires_at: Some(Utc::now() + Duration::days(1)),
                automatic: false,
            },
            ActionPreparation {
                batch_id: first_batch.id,
                chat_id: CHAT_ID,
                target_user_id: user_id + 1,
                actor_user_id: actor_id,
                action: "mute",
                reason: None,
                expires_at: Some(Utc::now() + Duration::days(1)),
                automatic: false,
            },
        ],
    )
    .await;
    assert!(
        batch_preflight.is_err(),
        "the conflicting second target rejects the full DB preflight"
    );
    let first_target_action_count: i64 = sqlx::query_scalar(
        "select count(*) from manual_moderation_actions where batch_id = $1 and target_user_id = $2",
    )
    .bind(first_batch.id)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        first_target_action_count, 0,
        "transaction rollback must leave no first-target intent"
    );
    assert!(
        add_warnings_batch(
            &pool,
            warning_batch.id,
            CHAT_ID,
            &[user_id, user_id + 1],
            actor_id,
            std::time::Duration::from_secs(30 * 24 * 60 * 60),
            None,
        )
        .await
        .is_err(),
        "a warning batch must reject every target before committing any warning"
    );
    let first_target_warning_count: i64 = sqlx::query_scalar(
        "select count(*) from manual_moderation_actions where batch_id = $1 and target_user_id = $2 and action = 'warn'",
    )
    .bind(warning_batch.id)
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(first_target_warning_count, 0);
    assert!(
        claim_restriction_revokes_batch(
            &pool,
            first_batch.id,
            CHAT_ID,
            &[user_id, user_id + 1],
            actor_id,
            "mute",
        )
        .await
        .is_err(),
        "unmute must claim every database target before any Telegram call"
    );

    mark_action_failed(&pool, blocker_id, false, "test cleanup")
        .await
        .unwrap();
    let request = BatchRequestSnapshot {
        kind: CommandKind::Ban,
        target_user_ids: vec![user_id + 2],
        duration_seconds: None,
        reason: Some("retry snapshot".to_string()),
        warn_id: None,
        warn_all: false,
        limit: 20,
    };
    let saved_message_id = MESSAGE_ID.fetch_add(1, Ordering::Relaxed);
    let saved =
        create_batch_with_request(&pool, CHAT_ID, actor_id, saved_message_id, "ban", &request)
            .await
            .unwrap();
    batch_ids.push(saved.id);
    let retry =
        create_batch_with_request(&pool, CHAT_ID, actor_id, saved_message_id, "ban", &request)
            .await;
    let retry = retry.unwrap();
    assert_eq!(retry.id, saved.id);
    assert_eq!(retry.request.as_ref(), Some(&request));

    assert_eq!(
        claim_batch(&pool, saved.id).await.unwrap(),
        BatchClaim::Claimed
    );
    let prepared = prepare_actions_batch(
        &pool,
        &[ActionPreparation {
            batch_id: saved.id,
            chat_id: CHAT_ID,
            target_user_id: user_id + 2,
            actor_user_id: actor_id,
            action: "ban",
            reason: Some("retry snapshot"),
            expires_at: None,
            automatic: false,
        }],
    )
    .await
    .unwrap()
    .pop()
    .unwrap()
    .unwrap();
    assert_eq!(prepared.status, "pending");
    assert!(start_prepared_action(&pool, prepared.id).await.unwrap());
    sqlx::query("update manual_moderation_actions set processing_lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(prepared.id)
        .execute(&pool)
        .await
        .unwrap();
    recover_expired_batch_actions(&pool, saved.id)
        .await
        .unwrap();
    let uncertain_status: String =
        sqlx::query_scalar("select status from manual_moderation_actions where id = $1")
            .bind(prepared.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(uncertain_status, "unknown");
    assert!(
        !start_prepared_action(&pool, prepared.id).await.unwrap(),
        "unknown Telegram outcomes must never be replayed"
    );
    sqlx::query("update manual_moderation_batches set processing_lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(saved.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        claim_batch(&pool, saved.id).await.unwrap(),
        BatchClaim::Claimed
    );
    assert_eq!(
        finish_batch(&pool, saved.id, "manual reconciliation required")
            .await
            .unwrap(),
        "unknown"
    );
    assert_eq!(
        claim_batch(&pool, saved.id).await.unwrap(),
        BatchClaim::Finished(Some("manual reconciliation required".to_string()))
    );

    assert_eq!(
        claim_batch(&pool, recoverable_batch.id).await.unwrap(),
        BatchClaim::Claimed
    );
    assert_eq!(
        claim_batch(&pool, recoverable_batch.id).await.unwrap(),
        BatchClaim::Busy
    );
    sqlx::query("update manual_moderation_batches set processing_lease_expires_at = now() - interval '1 second' where id = $1")
        .bind(recoverable_batch.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        claim_batch(&pool, recoverable_batch.id).await.unwrap(),
        BatchClaim::Claimed
    );

    cleanup(&pool, &[user_id, user_id + 1, user_id + 2], &batch_ids).await;
}

#[tokio::test]
#[ignore = "run against the disposable local test database"]
async fn unwarn_defaults_to_latest_and_only_explicit_all_revokes_every_warning() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to the disposable test database");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("test PostgreSQL must be reachable");
    let user_id = Utc::now().timestamp_micros() + 4_000_000_000;
    let actor_id = user_id + 1;
    let mut batch_ids = Vec::new();
    let mut warning_ids = Vec::new();
    for _ in 0..3 {
        let batch = create_batch(
            &pool,
            CHAT_ID,
            actor_id,
            MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
            "warn",
        )
        .await
        .unwrap();
        batch_ids.push(batch.id);
        let warning = add_warning(
            &pool,
            batch.id,
            CHAT_ID,
            user_id,
            actor_id,
            std::time::Duration::from_secs(30 * 24 * 60 * 60),
            None,
        )
        .await
        .unwrap();
        warning_ids.push(warning.action_id);
    }
    let unwarn_batch = create_batch(
        &pool,
        CHAT_ID,
        actor_id,
        MESSAGE_ID.fetch_add(1, Ordering::Relaxed),
        "unwarn",
    )
    .await
    .unwrap();
    batch_ids.push(unwarn_batch.id);

    assert_eq!(
        revoke_warnings(
            &pool,
            unwarn_batch.id,
            CHAT_ID,
            user_id,
            actor_id,
            WarningSelection::Latest,
            None
        )
        .await
        .unwrap(),
        [*warning_ids.last().unwrap()],
        "plain /unwarn revokes only the latest active warning"
    );
    let mut revoked_all = revoke_warnings(
        &pool,
        unwarn_batch.id,
        CHAT_ID,
        user_id,
        actor_id,
        WarningSelection::All,
        None,
    )
    .await
    .unwrap();
    revoked_all.sort_unstable();
    let mut expected_remaining = warning_ids[..2].to_vec();
    expected_remaining.sort_unstable();
    assert_eq!(
        revoked_all, expected_remaining,
        "all must be explicit to revoke the remaining warnings"
    );

    cleanup(&pool, &[user_id], &batch_ids).await;
}
