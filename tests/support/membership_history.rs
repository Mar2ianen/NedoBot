use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use sqlx::{PgPool, query, query_as, query_scalar};
use teloxide::types::{ChatMember, ChatMemberUpdated, Message};
use tg_ai_bot_teloxide::db::{
    membership::{MembershipContext, load_membership_context, observe_service_message},
    telegram::{save_chat_member_event, save_chat_member_snapshot},
};

const CHAT: i64 = -1_009_111_100_001;
const USER: i64 = 9_111_100_001;

fn member(user: i64, status: &str, present: bool) -> ChatMember {
    let mut v = json!({"status":status,"user":{"id":user,"is_bot":false,"first_name":"Fixture"}});
    if status == "restricted" {
        v["is_member"] = json!(present);
        v["until_date"] = json!(0);
        for k in [
            "can_send_messages",
            "can_send_audios",
            "can_send_documents",
            "can_send_photos",
            "can_send_videos",
            "can_send_video_notes",
            "can_send_voice_notes",
            "can_send_other_messages",
            "can_add_web_page_previews",
            "can_send_polls",
            "can_change_info",
            "can_invite_users",
            "can_pin_messages",
            "can_manage_topics",
            "can_react_to_messages",
            "can_edit_tag",
        ] {
            v[k] = json!(false);
        }
    }
    if status == "kicked" {
        v["until_date"] = json!(0);
    }
    serde_json::from_value(v).unwrap()
}

async fn event(
    pool: &PgPool,
    user: i64,
    old: &str,
    old_present: bool,
    new: &str,
    new_present: bool,
    at: DateTime<Utc>,
) {
    let e:ChatMemberUpdated=serde_json::from_value(json!({
        "chat":{"id":CHAT,"type":"supergroup","title":"Fixture"},
        "from":{"id":USER+99,"is_bot":false,"first_name":"Fixture"},"date":at.timestamp(),
        "old_chat_member":member(user,old,old_present),"new_chat_member":member(user,new,new_present)
    })).unwrap();
    save_chat_member_event(pool, &e).await.unwrap();
}

async fn context(pool: &PgPool, chat: i64, user: i64) -> MembershipContext {
    let (status,present):(Option<String>,Option<bool>)=query_as("select member_status,is_present from telegram_chat_users where chat_id=$1 and telegram_user_id=$2")
        .bind(chat).bind(user).fetch_one(pool).await.unwrap();
    load_membership_context(pool, chat, user, status.as_deref(), present)
        .await
        .unwrap()
}

pub async fn assert_membership_history(pool: &PgPool) {
    let t = DateTime::from_timestamp(Utc::now().timestamp() - 100, 0).unwrap();
    // Restriction без членства и бан никогда не доказывают прошлое вступление.
    event(pool, USER, "left", false, "restricted", false, t).await;
    let (joined,proof):(Option<DateTime<Utc>>,Option<DateTime<Utc>>)=query_as("select first_joined_at,membership_first_confirmed_at from telegram_chat_users where chat_id=$1 and telegram_user_id=$2")
        .bind(CHAT).bind(USER).fetch_one(pool).await.unwrap();
    assert!(joined.is_none() && proof.is_none());
    assert_eq!(
        context(pool, CHAT, USER).await,
        MembershipContext::NonMemberHistoryUnknown
    );
    // is_member меняется без изменения строки status=restricted.
    event(
        pool,
        USER,
        "restricted",
        false,
        "restricted",
        true,
        t + Duration::seconds(1),
    )
    .await;
    let joined: DateTime<Utc> = query_scalar(
        "select first_joined_at from telegram_chat_users where chat_id=$1 and telegram_user_id=$2",
    )
    .bind(CHAT)
    .bind(USER)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(joined, t + Duration::seconds(1));
    event(
        pool,
        USER,
        "restricted",
        true,
        "restricted",
        false,
        t + Duration::seconds(2),
    )
    .await;
    assert_eq!(
        context(pool, CHAT, USER).await,
        MembershipContext::FormerMember
    );
    let left: DateTime<Utc> = query_scalar(
        "select last_left_at from telegram_chat_users where chat_id=$1 and telegram_user_id=$2",
    )
    .bind(CHAT)
    .bind(USER)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(left, t + Duration::seconds(2));
    event(pool, USER + 1, "kicked", false, "left", false, t).await;
    assert_eq!(
        context(pool, CHAT, USER + 1).await,
        MembershipContext::NonMemberHistoryUnknown
    );
    // Старое member -> left достаточно даже без полученного события вступления.
    event(pool, USER + 2, "member", true, "left", false, t).await;
    assert_eq!(
        context(pool, CHAT, USER + 2).await,
        MembershipContext::FormerMember
    );

    // Polls сохраняют положительное доказательство, но не выдумывают joined_at.
    save_chat_member_snapshot(pool, CHAT, &member(USER + 3, "member", true), t)
        .await
        .unwrap();
    save_chat_member_snapshot(
        pool,
        CHAT,
        &member(USER + 3, "left", false),
        t + Duration::seconds(2),
    )
    .await
    .unwrap();
    save_chat_member_snapshot(
        pool,
        CHAT,
        &member(USER + 3, "member", true),
        t - Duration::seconds(1),
    )
    .await
    .unwrap();
    assert_eq!(
        context(pool, CHAT, USER + 3).await,
        MembershipContext::FormerMember
    );
    let snapshot:(String,bool)=query_as("select status,is_present from telegram_chat_member_snapshots where chat_id=$1 and telegram_user_id=$2")
        .bind(CHAT).bind(USER+3).fetch_one(pool).await.unwrap();
    assert_eq!(snapshot, ("left".into(), false));
    let (first,last,joined):(DateTime<Utc>,DateTime<Utc>,Option<DateTime<Utc>>)=query_as("select membership_first_confirmed_at,membership_last_confirmed_at,first_joined_at from telegram_chat_users where chat_id=$1 and telegram_user_id=$2")
        .bind(CHAT).bind(USER+3).fetch_one(pool).await.unwrap();
    assert_eq!((first, last, joined), (t - Duration::seconds(1), t, None));
    save_chat_member_snapshot(pool, CHAT - 1, &member(USER + 3, "left", false), t)
        .await
        .unwrap();
    assert_eq!(
        context(pool, CHAT - 1, USER + 3).await,
        MembershipContext::NonMemberHistoryUnknown
    );

    event(
        pool,
        USER + 4,
        "left",
        false,
        "member",
        true,
        t + Duration::seconds(3),
    )
    .await;
    event(
        pool,
        USER + 4,
        "member",
        true,
        "left",
        false,
        t + Duration::seconds(1),
    )
    .await;
    assert_eq!(
        context(pool, CHAT, USER + 4).await,
        MembershipContext::CurrentMember
    );

    // Служебный join/leave подтверждает цель, а не автора служебного сообщения.
    for service in [
        json!({"new_chat_members":[{"id":USER+5,"is_bot":false,"first_name":"Joined"}]}),
        json!({"left_chat_member":{"id":USER+6,"is_bot":false,"first_name":"Left"}}),
    ] {
        let mut v = json!({"message_id":900,"date":t.timestamp(),"chat":{"id":CHAT,"type":"supergroup","title":"Fixture"},"from":{"id":USER+90,"is_bot":false,"first_name":"Actor"}});
        v.as_object_mut()
            .unwrap()
            .extend(service.as_object().unwrap().clone());
        let message: Message = serde_json::from_value(v).unwrap();
        observe_service_message(pool, &message).await.unwrap();
    }
    assert_eq!(
        load_membership_context(pool, CHAT, USER + 5, Some("left"), Some(false))
            .await
            .unwrap(),
        MembershipContext::FormerMember
    );
    assert_eq!(
        load_membership_context(pool, CHAT, USER + 6, Some("left"), Some(false))
            .await
            .unwrap(),
        MembershipContext::FormerMember
    );
    assert_eq!(
        load_membership_context(pool, CHAT, USER + 90, Some("left"), Some(false))
            .await
            .unwrap(),
        MembershipContext::NonMemberHistoryUnknown
    );
    let ordinary:Message=serde_json::from_value(json!({"message_id":901,"date":t.timestamp(),"chat":{"id":CHAT,"type":"supergroup","title":"Fixture"},"from":{"id":USER+7,"is_bot":false,"first_name":"Commenter"},"text":"Обычный комментарий без вступления"})).unwrap();
    observe_service_message(pool, &ordinary).await.unwrap();
    assert_eq!(
        load_membership_context(pool, CHAT, USER + 7, Some("left"), Some(false))
            .await
            .unwrap(),
        MembershipContext::NonMemberHistoryUnknown
    );
    assert_upgrade_backfill(pool, t).await;
    for sql in [
        "delete from telegram_chat_member_events where chat_id in ($1,$1-1)",
        "delete from telegram_chat_member_snapshots where chat_id in ($1,$1-1)",
        "delete from telegram_chat_users where chat_id in ($1,$1-1)",
    ] {
        query(sql).bind(CHAT).execute(pool).await.unwrap();
    }
}

async fn assert_upgrade_backfill(pool: &PgPool, t: DateTime<Utc>) {
    let mut tx = pool.begin().await.unwrap();
    sqlx::raw_sql("alter table telegram_chat_users drop column membership_first_confirmed_at, drop column membership_last_confirmed_at").execute(&mut *tx).await.unwrap();
    // Ложный legacy joined_at/first_seen не подтверждает членство.
    query("insert into telegram_chat_users(chat_id,telegram_user_id,first_joined_at,first_seen_at) values($1,$2,$3,$3)")
        .bind(CHAT).bind(USER+20).bind(t).execute(&mut *tx).await.unwrap();
    // Переносим только настоящие служебные поля; malformed JSON не ломает upgrade.
    let rows: Vec<(i32, Value)> = vec![
        (1, json!({"new_chat_members":[{"id":USER+21}]})),
        (2, json!({"left_chat_member":{"id":USER+22}})),
        (
            3,
            json!({"new_chat_members":[{"id":"9223372036854775808999"},{"id":"oops"}]}),
        ),
        (
            4,
            json!({"new_chat_members":"invalid","left_chat_member":{"id":"bad"}}),
        ),
    ];
    for (id, raw) in rows {
        query("insert into telegram_messages(chat_id,message_id,user_id,text,raw_json) values($1,$2,$3,'fixture',$4)")
            .bind(CHAT).bind(id).bind(USER+20).bind(raw).execute(&mut *tx).await.unwrap();
    }
    sqlx::raw_sql(include_str!(
        "../../migrations/20261111100000_confirmed_membership_observations.sql"
    ))
    .execute(&mut *tx)
    .await
    .unwrap();
    let proven:Vec<i64>=query_scalar("select telegram_user_id from telegram_chat_users where chat_id=$1 and membership_first_confirmed_at is not null")
        .bind(CHAT).fetch_all(&mut *tx).await.unwrap();
    assert!(
        proven.contains(&(USER + 2))
            && proven.contains(&(USER + 21))
            && proven.contains(&(USER + 22))
    );
    assert!(!proven.contains(&(USER + 1)) && !proven.contains(&(USER + 20)));
    tx.rollback().await.unwrap();
}
