use chrono::{Duration, Utc};
use sqlx::PgPool;
use std::sync::atomic::{AtomicI32, Ordering};
use tg_ai_bot_teloxide::features::manual_moderation::{
    ActionPreparation, UndoClaim, add_warning, claim_undo_action, create_batch, latest_batch,
    list_actions, mark_action_failed, mark_action_succeeded, prepare_action, revoke_warnings,
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
            None,
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
