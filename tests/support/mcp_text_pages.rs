use std::collections::BTreeMap;

use serde_json::json;
use sqlx::{PgPool, postgres::PgPoolOptions, query};
use tg_ai_bot_teloxide::features::chat_read_api::{
    catalog::PublicCatalog,
    query::{self as read_query, TextReadRequest},
    types::ChatReadScope,
};

pub async fn assert_text_pages(pool: &PgPool, scope: &ChatReadScope, foreign_chat: i64, user: i64) {
    let directory = tempfile::tempdir().unwrap();
    let manifest = directory.path().join("manifest.toml");
    // Узкая reviewed text projection для query-теста; startup validation отдельно
    // проверяется полным manifest в существующих двухинстансовых сценариях.
    std::fs::write(
        &manifest,
        format!(
            r#"
version=2
source_schema="public"
public_schema="mcp_public"
[scope]
instance_id={:?}
discussion_chat_id={}
{}
[tables.telegram_messages]
description="Fixture"
primary_key=["chat_id","message_id"]
[tables.telegram_messages.columns.chat_id]
pg_type="bigint"
nullable=true
[tables.telegram_messages.columns.message_id]
pg_type="integer"
nullable=true
[tables.telegram_messages.columns.text]
pg_type="text"
nullable=true
"#,
            scope.instance_id(),
            scope.discussion_chat_id(),
            scope
                .source_channel_id()
                .map(|id| format!("source_channel_id={id}"))
                .unwrap_or_default()
        ),
    )
    .unwrap();
    let catalog = PublicCatalog::load(manifest.to_str().unwrap(), scope).unwrap();
    let text = "Длинная расшифровка 🦀\n".repeat(2000);
    query("insert into telegram_messages(chat_id,message_id,user_id,text) values($1,990001,$2,$3)")
        .bind(scope.discussion_chat_id())
        .bind(user)
        .bind(&text)
        .execute(pool)
        .await
        .unwrap();
    let readonly = PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                query("set role nedobot_mcp_ro").execute(connection).await?;
                Ok(())
            })
        })
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();
    let request = |chat_id, offset, revision| TextReadRequest {
        table: "telegram_messages".into(),
        column: "text".into(),
        key: BTreeMap::from([
            ("chat_id".into(), json!(chat_id)),
            ("message_id".into(), json!(990001)),
        ]),
        offset,
        limit: 3000,
        revision,
    };
    let mut offset = 0;
    let mut version = None;
    let mut recovered = String::new();
    loop {
        let page = read_query::read_text(
            &readonly,
            &catalog,
            request(scope.discussion_chat_id(), offset, version.clone()),
        )
        .await
        .unwrap();
        assert_eq!(page["found"], true);
        assert_eq!(page["total_chars"], text.chars().count());
        let fragment = page["text"].as_str().unwrap();
        assert_eq!(page["returned_chars"], fragment.chars().count());
        recovered.push_str(fragment);
        version = page["revision"].as_str().map(str::to_owned);
        if page["has_more"] == false {
            break;
        }
        offset = page["next_offset"].as_i64().unwrap();
    }
    assert_eq!(recovered, text);
    assert!(
        read_query::read_text(&readonly, &catalog, request(foreign_chat, 0, None))
            .await
            .is_err()
    );
    let mut private = request(scope.discussion_chat_id(), 0, None);
    private.table = "ask_conversation_memory".into();
    assert!(
        read_query::read_text(&readonly, &catalog, private)
            .await
            .is_err()
    );
    let mut incomplete = request(scope.discussion_chat_id(), 0, None);
    incomplete.key.remove("chat_id");
    assert!(
        read_query::read_text(&readonly, &catalog, incomplete)
            .await
            .is_err()
    );
    let mut nontext = request(scope.discussion_chat_id(), 0, None);
    nontext.column = "message_id".into();
    assert!(
        read_query::read_text(&readonly, &catalog, nontext)
            .await
            .is_err()
    );
    let mut invalid = request(scope.discussion_chat_id(), -1, None);
    assert!(
        read_query::read_text(&readonly, &catalog, invalid)
            .await
            .is_err()
    );
    invalid = request(scope.discussion_chat_id(), 0, None);
    invalid.limit = 12001;
    assert!(
        read_query::read_text(&readonly, &catalog, invalid)
            .await
            .is_err()
    );
    query("update telegram_messages set text='Изменённый текст' where chat_id=$1 and message_id=990001").bind(scope.discussion_chat_id()).execute(pool).await.unwrap();
    assert!(
        read_query::read_text(
            &readonly,
            &catalog,
            request(scope.discussion_chat_id(), 0, version)
        )
        .await
        .is_err()
    );
    readonly.close().await;
    query("delete from telegram_messages where chat_id=$1 and message_id=990001")
        .bind(scope.discussion_chat_id())
        .execute(pool)
        .await
        .unwrap();
}
