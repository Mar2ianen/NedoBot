use std::{sync::Arc, time::Duration};

use sqlx::{PgPool, query, query_scalar};
use teloxide::types::User;
use tg_ai_bot_teloxide::{
    db::telegram::upsert_user_profile,
    features::url_relations::{TelegramUsernameResolution, resolve_telegram_username},
};
use tokio::sync::Barrier;

fn synthetic_user(id: i64, username: &str) -> User {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "is_bot": false,
        "first_name": "Synthetic",
        "username": username
    }))
    .expect("synthetic Telegram profile must deserialize")
}

pub async fn assert_current_alias_resolution(pool: &PgPool) {
    let suffix = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after unix epoch")
        .as_nanos()
        % 900_000_000) as i64;

    for iteration in 0..32_i64 {
        let user_id = 9_100_000_000_000 + suffix * 32 + iteration;
        let username_a = format!("a{suffix:08x}{iteration:02x}");
        let username_b = format!("b{suffix:08x}{iteration:02x}");
        let barrier = Arc::new(Barrier::new(3));
        let user_a = synthetic_user(user_id, &username_a);
        let user_b = synthetic_user(user_id, &username_b);
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);
        let pool_a = pool.clone();
        let pool_b = pool.clone();
        let a = tokio::spawn(async move {
            barrier_a.wait().await;
            upsert_user_profile(&pool_a, &user_a).await
        });
        let b = tokio::spawn(async move {
            barrier_b.wait().await;
            upsert_user_profile(&pool_b, &user_b).await
        });
        barrier.wait().await;
        tokio::time::timeout(Duration::from_secs(10), async {
            a.await.expect("first concurrent profile task must join")?;
            b.await.expect("second concurrent profile task must join")?;
            anyhow::Ok(())
        })
        .await
        .expect("concurrent initial profile upserts must not hang")
        .expect("both synthetic profile upserts must commit");

        let profile_username: String = query_scalar(
            "select username from public.telegram_user_profiles where telegram_user_id = $1",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("one synthetic profile row must be present");
        let resolution_a = resolve_telegram_username(pool, &username_a)
            .await
            .expect("first synthetic alias must resolve");
        let resolution_b = resolve_telegram_username(pool, &username_b)
            .await
            .expect("second synthetic alias must resolve");
        let (active_username, inactive_resolution) = if profile_username == username_a {
            (username_a.as_str(), resolution_b)
        } else {
            assert_eq!(profile_username, username_b);
            (username_b.as_str(), resolution_a)
        };
        assert!(matches!(
            resolve_telegram_username(pool, active_username).await,
            Ok(TelegramUsernameResolution::Unique(peer)) if peer.peer_id == user_id
        ));
        assert_eq!(
            inactive_resolution,
            TelegramUsernameResolution::Unknown,
            "only the last serialized username may remain active"
        );
        query("delete from public.telegram_username_aliases where telegram_peer_id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .expect("synthetic alias fixture must be removed");
        query("delete from public.telegram_user_profiles where telegram_user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .expect("synthetic profile fixture must be removed");
    }

    let owner_a = 9_200_000_000_001 + suffix;
    let owner_b = owner_a + 1;
    let old_username = format!("old{suffix:08x}");
    let new_username = format!("new{suffix:08x}");
    let other_username = format!("other{suffix:08x}");
    upsert_user_profile(pool, &synthetic_user(owner_a, &old_username))
        .await
        .expect("first owner must be created");
    upsert_user_profile(pool, &synthetic_user(owner_b, &other_username))
        .await
        .expect("second owner must be created");
    upsert_user_profile(pool, &synthetic_user(owner_a, &new_username))
        .await
        .expect("first owner must release its prior alias");
    upsert_user_profile(pool, &synthetic_user(owner_b, &old_username))
        .await
        .expect("second owner must acquire the released alias");
    assert!(matches!(
        resolve_telegram_username(pool, &old_username).await,
        Ok(TelegramUsernameResolution::Unique(peer)) if peer.peer_id == owner_b
    ));
    query("delete from public.telegram_username_aliases where telegram_peer_id = any($1)")
        .bind([owner_a, owner_b])
        .execute(pool)
        .await
        .expect("synthetic alias reassignment records must be removed");
    query("delete from public.telegram_user_profiles where telegram_user_id = any($1)")
        .bind([owner_a, owner_b])
        .execute(pool)
        .await
        .expect("synthetic alias reassignment fixtures must be removed");
}
