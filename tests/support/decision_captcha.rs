use tg_ai_bot_teloxide::{
    community::ChatRegistry,
    config::Config,
    features::{
        decision_audit::{
            actions,
            client::{Answer, DecisionAssessment},
            contacts, repo,
            service::observe_message,
        },
        labels::record_not_spam,
    },
};
fn config() -> Config {
    let mut c = Config::from_env().unwrap();
    let chat = c.community.chats.get_mut("main").unwrap();
    chat.id = CHAT_ID;
    chat.ingest = true;
    chat.moderation = true;
    c.community.instance.id = "decision_captcha_fixture".into();
    c.chat_registry = ChatRegistry::new(&c.community).unwrap();
    let m = &mut c.community.moderation;
    m.enabled = true;
    m.captcha_enabled = true;
    m.captcha_dry_run = false;
    m.captcha_threshold = 85;
    m.captcha_ttl_sec = 600;
    m.decision.enabled = true;
    m.decision.external_processing_allowed = true;
    m.decision.mode = serde_json::from_str("\"score_captcha\"").unwrap();
    m.decision.initial_texts = 10;
    m.decision.ban_on_failure = true;
    c
}
fn positive() -> DecisionAssessment {
    let mut a: DecisionAssessment =
        serde_json::from_str(include_str!("../fixtures/decision_response.json")).unwrap();
    for key in [
        "unsolicited_promotion",
        "work_solicitation",
        "private_contact_funnel",
    ] {
        a.answers.insert(
            key.into(),
            Answer::Noul {
                noul: 0.98,
                confidence: 0.98,
            },
        );
    }
    if let Answer::Choice {
        choice,
        probabilities,
        confidence,
    } = a.answers.get_mut("spam_campaign_type").unwrap()
    {
        *choice = "work_dm_bait".into();
        *confidence = 0.98;
        for (k, p) in probabilities {
            *p = if k == "work_dm_bait" { 0.98 } else { 0.005 };
        }
    }
    if let Answer::Score {
        score,
        probabilities,
        confidence,
        ..
    } = a.answers.get_mut("contextual_relevance").unwrap()
    {
        *score = 0.05;
        *confidence = 0.95;
        for (k, p) in probabilities {
            *p = match k.as_str() {
                "0" => 0.95,
                "1" => 0.05,
                _ => 0.0,
            };
        }
    }
    a.validate().unwrap();
    a
}
async fn reset(pool: &PgPool, stub: &TelegramStub) {
    query("delete from telegram_risk_captcha_challenges where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_new_user_profile_audits where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    query("delete from moderation_decision_jobs where instance_id='decision_captcha_fixture'")
        .execute(pool)
        .await
        .unwrap();
    query("delete from moderation_decision_budget where instance_id='decision_captcha_fixture'")
        .execute(pool)
        .await
        .unwrap();
    query("delete from spam_label_events where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_messages where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    query("insert into telegram_chat_users(chat_id,telegram_user_id,first_seen_at,message_count) values($1,$2,now(),2) on conflict(chat_id,telegram_user_id) do update set first_seen_at=now(),message_count=2").bind(CHAT_ID).bind(USER_ID).execute(pool).await.unwrap();
    query("insert into telegram_messages(chat_id,message_id,user_id,text,created_at,raw_json) values($1,500,$2,'Плачу за отзывы, 10к за 10 отзывов. Пиши @Nikol_bt56',now(),'{\"entities\":[{\"type\":\"mention\",\"offset\":38,\"length\":11}]}'),($1,501,$2,'Обычный соседний ответ',now(),'{}')")
        .bind(CHAT_ID).bind(USER_ID).execute(pool).await.unwrap();
    // Entity offset вычисляется по UTF-16, чтобы fixture не зависела от подсчёта кириллицы.
    let text: String =
        query_scalar("select text from telegram_messages where chat_id=$1 and message_id=500")
            .bind(CHAT_ID)
            .fetch_one(pool)
            .await
            .unwrap();
    let offset = text[..text.find('@').unwrap()].encode_utf16().count();
    query("update telegram_messages set raw_json=$2 where chat_id=$1 and message_id=500")
        .bind(CHAT_ID)
        .bind(json!({"entities":[{"type":"mention","offset":offset,"length":11}]}))
        .execute(pool)
        .await
        .unwrap();
    stub.stateful.store(true, Ordering::SeqCst);
    stub.non_member.store(false, Ordering::SeqCst);
    stub.restricted.store(false, Ordering::SeqCst);
    stub.banned.store(false, Ordering::SeqCst);
    stub.omit_ephemeral_id.store(false, Ordering::SeqCst);
    stub.fail_send.store(false, Ordering::SeqCst);
    stub.calls.lock().unwrap().clear();
}
async fn completed(pool: &PgPool, c: &Config, a: &DecisionAssessment) -> repo::DecisionJob {
    observe_message(pool, c, CHAT_ID, 500).await.unwrap();
    let j = repo::claim(pool, c).await.unwrap().unwrap();
    assert!(repo::complete(pool, &j, a, 1).await.unwrap());
    j
}
async fn challenge(pool: &PgPool) -> (Uuid, i16) {
    sqlx::query_as(
        "select id,correct_option from telegram_risk_captcha_challenges where chat_id=$1",
    )
    .bind(CHAT_ID)
    .fetch_one(pool)
    .await
    .unwrap()
}
async fn status(pool: &PgPool) -> String {
    query_scalar("select status from telegram_risk_captcha_challenges where chat_id=$1")
        .bind(CHAT_ID)
        .fetch_one(pool)
        .await
        .unwrap()
}
async fn expire(pool: &PgPool, bot: &Bot) {
    query("update telegram_risk_captcha_challenges set expires_at=now()-interval '1 second' where chat_id=$1").bind(CHAT_ID).execute(pool).await.unwrap();
    assert!(process_next_expired(bot, pool).await.unwrap());
}
pub async fn assert_policy(pool: &PgPool, bot: &Bot, stub: &TelegramStub) {
    assert_contacts(pool).await;
    let mut c = config();
    let a = positive();
    reset(pool, stub).await;
    let f = contacts::extract(
        pool,
        CHAT_ID,
        USER_ID,
        "@Nikol_bt56",
        &json!([{"type":"mention","offset":0,"length":11}]),
    )
    .await
    .unwrap();
    assert_eq!(f.unseen_usernames, 1);
    // Shadow сохраняет результат, не вызывает Telegram.
    c.community.moderation.decision.mode = serde_json::from_str("\"shadow\"").unwrap();
    completed(pool, &c, &a).await;
    assert!(!actions::process_next(bot, pool, &c).await.unwrap());
    assert!(stub.calls.lock().unwrap().is_empty());
    reset(pool, stub).await;
    c.community.moderation.decision.mode = serde_json::from_str("\"score_captcha\"").unwrap();
    c.community.moderation.captcha_dry_run = true;
    completed(pool, &c, &a).await;
    assert!(actions::process_next(bot, pool, &c).await.unwrap());
    assert!(stub.calls.lock().unwrap().is_empty());
    // Реальный handoff: карточка reply отправляется раньше удаления только отмеченного текста.
    reset(pool, stub).await;
    c.community.moderation.captcha_dry_run = false;
    let j = completed(pool, &c, &a).await;
    query("update moderation_decision_jobs set action_status='processing',action_fence=5,action_lease_expires_at=now()-interval '1 second' where id=$1").bind(j.id).execute(pool).await.unwrap();
    assert!(actions::process_next(bot, pool, &c).await.unwrap());
    assert_eq!(status(pool).await, "pending");
    assert_eq!(send_count(stub), 1);
    {
        let calls = stub.calls.lock().unwrap();
        let sent = calls.iter().position(|(m, _)| m == "sendmessage").unwrap();
        let deleted = calls
            .iter()
            .position(|(m, _)| m == "deletemessages")
            .unwrap();
        assert!(sent < deleted);
        assert_eq!(calls[sent].1["reply_parameters"]["message_id"], 500);
        assert_eq!(
            calls[sent].1["ephemeral_message_parameters"]["receiver_user_id"],
            USER_ID
        );
    }
    assert_eq!(delete_batches(stub), vec![vec![500]]);
    let normal:bool=query_scalar("select deleted_by_bot_at is null from telegram_messages where chat_id=$1 and message_id=501").bind(CHAT_ID).fetch_one(pool).await.unwrap();
    assert!(normal);
    assert!(!actions::process_next(bot, pool, &c).await.unwrap());
    assert_eq!(send_count(stub), 1);
    query("update telegram_new_user_profile_audits set risk_baseline_score=2,risk_first_message_score=20,risk_score=22 where chat_id=$1").bind(CHAT_ID).execute(pool).await.unwrap();
    let score:(i32,i32,i32)=sqlx::query_as("select risk_score,risk_decision_score,risk_contact_score from telegram_new_user_profile_audits where chat_id=$1").bind(CHAT_ID).fetch_one(pool).await.unwrap();
    assert_eq!(score.0, 2 + score.1 + score.2);
    assert!(score.0 >= 85);
    assert!(
        repo::captcha_source_is_current(pool, j.id, CHAT_ID, USER_ID)
            .await
            .unwrap()
    );
    assert!(
        !repo::captcha_source_is_current(pool, j.id, CHAT_ID - 1, USER_ID)
            .await
            .unwrap()
    );
    let (id, option) = challenge(pool).await;
    assert_eq!(
        handle_callback(
            bot,
            pool,
            Some(CHAT_ID),
            USER_ID + 1,
            &format!("rc:{id}:{option}")
        )
        .await
        .unwrap(),
        CaptchaCallbackOutcome::NotForThisUser
    );
    handle_callback(
        bot,
        pool,
        Some(CHAT_ID),
        USER_ID,
        &format!("rc:{id}:{option}"),
    )
    .await
    .unwrap();
    assert_eq!(status(pool).await, "passed");
    assert!(!stub.restricted.load(Ordering::SeqCst));
    assert!(!stub.banned.load(Ordering::SeqCst));
    // Общая baseline-оценка, завершившаяся позже модели, тоже активирует handoff.
    reset(pool, stub).await;
    query("update telegram_messages set raw_json='{}' where chat_id=$1 and message_id=500")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
    let mut moderate = a.clone();
    moderate.answers.insert(
        "unsolicited_promotion".into(),
        Answer::Noul {
            noul: 0.88,
            confidence: 0.88,
        },
    );
    moderate.answers.insert(
        "work_solicitation".into(),
        Answer::Noul {
            noul: 0.5,
            confidence: 0.5,
        },
    );
    completed(pool, &c, &moderate).await;
    actions::process_next(bot, pool, &c).await.unwrap();
    assert_eq!(send_count(stub), 0);
    query("update telegram_new_user_profile_audits set risk_baseline_score=10,risk_score=10 where chat_id=$1").bind(CHAT_ID).execute(pool).await.unwrap();
    assert!(actions::process_next(bot, pool, &c).await.unwrap());
    assert_eq!(send_count(stub), 1);
    reset(pool, stub).await;
    query("update telegram_messages set created_at=now()-interval '11 minutes' where chat_id=$1 and message_id=500").bind(CHAT_ID).execute(pool).await.unwrap();
    completed(pool, &c, &a).await;
    actions::process_next(bot, pool, &c).await.unwrap();
    assert_eq!(send_count(stub), 0);
    // Истечение выданной капчи блокирует, не выполняет unban.
    reset(pool, stub).await;
    completed(pool, &c, &a).await;
    actions::process_next(bot, pool, &c).await.unwrap();
    let (id, option) = challenge(pool).await;
    for _ in 0..3 {
        handle_callback(
            bot,
            pool,
            Some(CHAT_ID),
            USER_ID,
            &format!("rc:{id}:{}", (option + 1) % 4),
        )
        .await
        .unwrap();
    }
    expire(pool, bot).await;
    assert!(stub.banned.load(Ordering::SeqCst));
    assert!(
        !stub
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "unbanchatmember")
    );
    // Комментатор вне группы получает ephemeral reply без предварительного mute.
    reset(pool, stub).await;
    stub.non_member.store(true, Ordering::SeqCst);
    completed(pool, &c, &a).await;
    actions::process_next(bot, pool, &c).await.unwrap();
    assert_eq!(send_count(stub), 1);
    assert!(
        !stub
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(m, _)| m == "restrictchatmember")
    );
    expire(pool, bot).await;
    assert!(stub.banned.load(Ordering::SeqCst));
    // Ручной normal отзывает доставленную капчу и снимает принадлежащий ей mute.
    reset(pool, stub).await;
    completed(pool, &c, &a).await;
    actions::process_next(bot, pool, &c).await.unwrap();
    record_not_spam(pool, CHAT_ID, USER_ID, "fixture", &json!({}), None)
        .await
        .unwrap();
    assert!(process_next_setup(bot, pool, 600).await.unwrap());
    assert_eq!(status(pool).await, "overridden");
    assert!(!stub.restricted.load(Ordering::SeqCst));
    let score:i32=query_scalar("select risk_decision_score+risk_contact_score from telegram_new_user_profile_audits where chat_id=$1").bind(CHAT_ID).fetch_one(pool).await.unwrap();
    assert_eq!(score, 0);
    reset(pool, stub).await;
    completed(pool, &c, &a).await;
    query("update telegram_messages set text='Новый текст',edited_at=now() where chat_id=$1 and message_id=500").bind(CHAT_ID).execute(pool).await.unwrap();
    actions::process_next(bot, pool, &c).await.unwrap();
    assert!(stub.calls.lock().unwrap().is_empty());
    reset(pool, stub).await;
    completed(pool, &c, &a).await;
    c.community.moderation.decision.score_weight = 89;
    actions::process_next(bot, pool, &c).await.unwrap();
    assert!(stub.calls.lock().unwrap().is_empty());
    c.community.moderation.decision.score_weight = 90;
    // Edit/delete до доставки закрываются без внешнего действия; после доставки отзывают mute.
    for delete in [false, true] {
        reset(pool, stub).await;
        completed(pool, &c, &a).await;
        actions::process_next(bot, pool, &c).await.unwrap();
        if delete {
            query("delete from telegram_messages where chat_id=$1 and message_id=500")
                .bind(CHAT_ID)
                .execute(pool)
                .await
                .unwrap();
        } else {
            query("update telegram_messages set text='Исправленный нормальный ответ',edited_at=now() where chat_id=$1 and message_id=500").bind(CHAT_ID).execute(pool).await.unwrap();
        }
        assert!(process_next_setup(bot, pool, 600).await.unwrap());
        assert_eq!(status(pool).await, "overridden");
        assert!(!stub.restricted.load(Ordering::SeqCst));
        assert!(!stub.banned.load(Ordering::SeqCst));
    }
    reset(pool, stub).await;
    completed(pool, &c, &a).await;
    record_not_spam(pool, CHAT_ID, USER_ID, "fixture", &json!({}), None)
        .await
        .unwrap();
    actions::process_next(bot, pool, &c).await.unwrap();
    assert!(stub.calls.lock().unwrap().is_empty());
    reset(pool, stub).await;
    let negative: DecisionAssessment =
        serde_json::from_str(include_str!("../fixtures/decision_response.json")).unwrap();
    completed(pool, &c, &negative).await;
    actions::process_next(bot, pool, &c).await.unwrap();
    assert!(stub.calls.lock().unwrap().is_empty());
    reset(pool, stub).await;
    query("delete from telegram_chat_users where chat_id=$1")
        .bind(CHAT_ID)
        .execute(pool)
        .await
        .unwrap();
}

async fn assert_contacts(pool: &PgPool) {
    let ids = [
        USER_ID + 101,
        USER_ID + 102,
        USER_ID + 103,
        USER_ID + 104,
        USER_ID + 105,
    ];
    query("insert into telegram_user_profiles(telegram_user_id,username,is_bot) values($1,'fixture_member',false),($1+1,'fixture_bot',true),($1+2,'fixture_foreign',false),($1+3,'fixture_duplicate',false),($1+4,'fixture_duplicate',false)").bind(ids[0]).execute(pool).await.unwrap();
    query("insert into telegram_chat_users(chat_id,telegram_user_id,message_count,first_seen_at) values($1,$2,1,now()),($1-1,$2+2,10,now())").bind(CHAT_ID).bind(ids[0]).execute(pool).await.unwrap();
    let check =
        |text: &str| json!([{"type":"mention","offset":0,"length":text.encode_utf16().count()}]);
    for (name, expected) in [
        ("@fixture_member", (1, 0, 0, 0)),
        ("@fixture_bot", (0, 1, 0, 0)),
        ("@fixture_foreign", (0, 0, 1, 0)),
        ("@fixture_duplicate", (0, 0, 0, 1)),
    ] {
        let f = contacts::extract(pool, CHAT_ID, USER_ID, name, &check(name))
            .await
            .unwrap();
        assert_eq!(
            (
                f.known_participants,
                f.known_bots,
                f.unknown_membership,
                f.ambiguous_usernames
            ),
            expected
        );
    }
    query("insert into telegram_chat_member_snapshots(chat_id,telegram_user_id,status,is_present,observed_at) values($1,$2,'left',false,now())").bind(CHAT_ID).bind(ids[0]).execute(pool).await.unwrap();
    let f = contacts::extract(
        pool,
        CHAT_ID,
        USER_ID,
        "@fixture_member",
        &check("@fixture_member"),
    )
    .await
    .unwrap();
    assert_eq!(f.last_observed_nonmembers, 1);
    assert!(f.external_candidate());
    let text_mention = json!([{"type":"text_mention","offset":0,"length":4,"user":{"id":ids[0],"is_bot":false,"first_name":"User"}}]);
    assert_eq!(
        contacts::extract(pool, CHAT_ID, USER_ID, "User", &text_mention)
            .await
            .unwrap()
            .last_observed_nonmembers,
        1
    );
    assert!(!contacts::extract(pool,CHAT_ID,USER_ID,"@fixture_foreign",&json!([{"type":"mention","offset":0,"length":16},{"type":"code","offset":0,"length":16}])).await.unwrap().external_candidate());
    query(
        "delete from telegram_chat_member_snapshots where chat_id=$1 and telegram_user_id=any($2)",
    )
    .bind(CHAT_ID)
    .bind(&ids[..])
    .execute(pool)
    .await
    .unwrap();
    query("delete from telegram_chat_users where chat_id in($1,$1-1) and telegram_user_id=any($2)")
        .bind(CHAT_ID)
        .bind(&ids[..])
        .execute(pool)
        .await
        .unwrap();
    query("delete from telegram_user_profiles where telegram_user_id=any($1)")
        .bind(&ids[..])
        .execute(pool)
        .await
        .unwrap();
}
