use sqlx::{PgPool, query};
use tg_ai_bot_teloxide::features::url_relations::resolve_recent_username_aliases;

pub async fn assert_current_alias_resolution(pool: &PgPool) {
    let suffix = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after unix epoch")
        .as_nanos()
        % 900_000_000) as i64;
    let owner_a = 9_100_000_000_000 + suffix * 2;
    let owner_b = owner_a + 1;
    let alias_a = format!("a{:08x}bot", suffix as u32);
    let alias_b = format!("b{:08x}agent", suffix as u32);
    let stale = format!("s{:08x}", suffix as u32);

    query("delete from telegram_user_profiles where telegram_user_id = any($1)")
        .bind(vec![owner_a, owner_b])
        .execute(pool)
        .await
        .expect("synthetic alias fixtures must start clean");

    query(
        "insert into telegram_user_profiles (telegram_user_id, username, is_bot, updated_at) values ($1, $2, false, now()), ($3, $4, true, now())",
    )
    .bind(owner_a)
    .bind(&alias_a)
    .bind(owner_b)
    .bind(&alias_b)
    .execute(pool)
    .await
    .expect("two owners must receive separate current aliases");

    let resolved = resolve_recent_username_aliases(pool, &[alias_a.clone(), alias_b.clone()])
        .await
        .expect("fresh aliases must be queryable");
    assert_eq!(resolved.get(&alias_a), Some(&(owner_a, false)));
    assert_eq!(resolved.get(&alias_b), Some(&(owner_b, true)));

    // The first owner gives up alias A and the second owner acquires it.
    query("update telegram_user_profiles set username = $2, updated_at = now() where telegram_user_id = $1")
        .bind(owner_a)
        .bind(&stale)
        .execute(pool)
        .await
        .expect("first owner's current username must change");
    query("update telegram_user_profiles set username = $2, updated_at = now() where telegram_user_id = $1")
        .bind(owner_b)
        .bind(&alias_a)
        .execute(pool)
        .await
        .expect("second owner must acquire the reassigned alias");

    let reassigned = resolve_recent_username_aliases(pool, std::slice::from_ref(&alias_a))
        .await
        .expect("reassigned alias must be queryable");
    assert_eq!(reassigned.get(&alias_a), Some(&(owner_b, true)));
    assert_ne!(reassigned.get(&alias_a).map(|value| value.0), Some(owner_a));

    // When the only observation for an alias becomes stale, it must not fall
    // back to the former owner or be promoted to a known external target.
    query("update telegram_user_profiles set username = $2, updated_at = now() - interval '25 hours' where telegram_user_id = $1")
        .bind(owner_b)
        .bind(&alias_a)
        .execute(pool)
        .await
        .expect("current profile observation must age out");
    let expired = resolve_recent_username_aliases(pool, std::slice::from_ref(&alias_a))
        .await
        .expect("stale alias lookup must be queryable");
    assert!(!expired.contains_key(&alias_a));

    // Two fresh snapshots claiming the same alias are ambiguous even when
    // one owner is a bot and the other is not.
    query("update telegram_user_profiles set username = $2, updated_at = now() where telegram_user_id = $1")
        .bind(owner_a)
        .bind(&alias_b)
        .execute(pool)
        .await
        .expect("first duplicate alias fixture must be fresh");
    query("update telegram_user_profiles set username = $2, updated_at = now() where telegram_user_id = $1")
        .bind(owner_b)
        .bind(&alias_b)
        .execute(pool)
        .await
        .expect("second duplicate alias fixture must be fresh");
    let duplicate = resolve_recent_username_aliases(pool, std::slice::from_ref(&alias_b))
        .await
        .expect("duplicate alias lookup must be queryable");
    assert!(!duplicate.contains_key(&alias_b));

    query("delete from telegram_user_profiles where telegram_user_id = any($1)")
        .bind(vec![owner_a, owner_b])
        .execute(pool)
        .await
        .expect("synthetic alias fixtures must be removed");
}
