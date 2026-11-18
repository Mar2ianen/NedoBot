use sqlx::{PgPool, Row, postgres::PgPoolOptions, query, query_scalar};
use tg_ai_bot_teloxide::private_corpus::extract_private_records;

async fn fixture_database(
    url: &str,
    instance: &str,
    chat_id: i64,
    own_url: &str,
    foreign_url: &str,
) -> String {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(url)
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TEMP TABLE instance_scope (scope_key boolean, instance_id text, discussion_chat_id bigint);
         CREATE TEMP TABLE telegram_messages (message_id bigint, chat_id bigint, created_at timestamptz, raw_text jsonb, entities jsonb, has_links boolean);
         CREATE TEMP TABLE qr_evidence (message_id bigint, chat_id bigint, created_at timestamptz, target_sha256 text, host text, display_hint text, privacy_sensitive boolean);"
    ).execute(&pool).await.unwrap();
    query("INSERT INTO instance_scope VALUES (true, $1, $2)")
        .bind(instance)
        .bind(chat_id)
        .execute(&pool)
        .await
        .unwrap();
    for (id, row_chat, text) in [
        (1_i64, chat_id, own_url),
        (
            2_i64,
            if chat_id == -101 { -202 } else { -101 },
            foreign_url,
        ),
    ] {
        query("INSERT INTO telegram_messages VALUES ($1,$2,'2026-01-01T00:00:00Z',$3,'[]',true)")
            .bind(id)
            .bind(row_chat)
            .bind(serde_json::json!(text))
            .execute(&pool)
            .await
            .unwrap();
    }
    query("INSERT INTO qr_evidence VALUES (3,$1,'2026-01-02T00:00:00Z',$2,'example.org','example.org/fixture',true)")
        .bind(chat_id).bind("a".repeat(64)).execute(&pool).await.unwrap();
    let message_lines = scoped_message_lines(&pool).await;
    let qr_lines = scoped_qr_lines(&pool).await;
    drop(pool);
    let (records, _) =
        extract_private_records(instance, message_lines.as_bytes(), qr_lines.as_bytes()).unwrap();
    serde_json::to_string(&records).unwrap()
}

async fn scoped_message_lines(pool: &PgPool) -> String {
    let rows: Vec<String> = query_scalar(
        "SELECT jsonb_build_object('message_id',m.message_id,'created_at',m.created_at,'text',m.raw_text,'caption',null,'entities',m.entities,'caption_entities',jsonb_build_array(),'rich_message',null)::text
         FROM telegram_messages m JOIN instance_scope s ON s.scope_key AND m.chat_id=s.discussion_chat_id AND s.instance_id IN ('synthetic_a','synthetic_b') ORDER BY m.message_id"
    ).fetch_all(pool).await.unwrap();
    rows.join("\n") + "\n"
}

async fn scoped_qr_lines(pool: &PgPool) -> String {
    let rows: Vec<serde_json::Value> = query(
        "SELECT jsonb_build_object('message_id',q.message_id,'created_at',q.created_at,'target_sha256',q.target_sha256,'host',q.host,'display_hint',q.display_hint,'privacy_sensitive',q.privacy_sensitive) AS row
         FROM qr_evidence q JOIN instance_scope s ON s.scope_key AND q.chat_id=s.discussion_chat_id"
    ).fetch_all(pool).await.unwrap().into_iter().map(|r| r.get("row")).collect();
    rows.into_iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[tokio::test]
#[ignore = "requires two disposable synthetic PostgreSQL databases"]
async fn two_synthetic_databases_extract_only_their_scoped_messages_and_qr_hashes() {
    let a = std::env::var("TEST_PRIVATE_HISTORY_DATABASE_URL_A").expect("synthetic DB A URL");
    let b = std::env::var("TEST_PRIVATE_HISTORY_DATABASE_URL_B").expect("synthetic DB B URL");
    let a_records = fixture_database(
        &a,
        "synthetic_a",
        -101,
        "https://example.org/a",
        "https://example.org/foreign_b",
    )
    .await;
    let b_records = fixture_database(
        &b,
        "synthetic_b",
        -202,
        "https://example.org/b",
        "https://example.org/foreign_a",
    )
    .await;
    assert!(a_records.contains("https://example.org/a"));
    assert!(!a_records.contains("https://example.org/foreign_b"));
    assert!(b_records.contains("https://example.org/b"));
    assert!(!b_records.contains("https://example.org/foreign_a"));
    assert!(!a_records.contains("https://example.org/b"));
    assert!(!b_records.contains("https://example.org/a"));
    for serialized in [&a_records, &b_records] {
        assert!(serialized.contains("qr_hash_only"));
        assert!(serialized.contains("\"canonical_url_identity\":null"));
        assert!(serialized.contains("\"qr_url_recoverable\":false"));
    }
}
