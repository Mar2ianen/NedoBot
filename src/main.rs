use teloxide::{
    dispatching::UpdateFilterExt,
    prelude::*,
    types::{
        ChatId, ChatMemberKind, ChatMemberUpdated, MessageReactionCountUpdated,
        MessageReactionUpdated, ParseMode,
    },
};

#[cfg(feature = "moderation")]
use teloxide::types::CallbackQuery;

use tg_ai_bot_teloxide::{config, db, features, llm, state, telegram};

use config::Config;
use db::telegram::{
    refresh_known_member_snapshots, save_chat_member_event, save_edited_telegram_message,
    save_message_reaction, save_message_reaction_count,
};
use db::{build_pool, migrate};
use features::chat_retrieval::process_next_embedding_batch;
#[cfg(feature = "auto-comment")]
use features::first_comment::pipeline::{maybe_comment_post, process_next_post_comment_job};
use features::ingest::{ingest_message, is_managed_chat, managed_chat_allows};
#[cfg(feature = "moderation")]
use features::jobs::policy::EXTERNAL_ANALYSIS_POLL;
use features::jobs::policy::POST_HISTORY_POLL;
#[cfg(feature = "voice")]
use features::jobs::policy::VOICE_TRANSCRIPTION_POLL;
use features::member_greetings::handle_chat_member_update;
use features::memory::service::process_next_history_entry;
#[cfg(feature = "moderation")]
use features::new_user_audit::service::process_next_new_user_audit_job;
#[cfg(feature = "moderation")]
use features::reports::{self, ReportActionResult};
use features::service_message_cleanup::maybe_delete_join_leave_message;
#[cfg(feature = "spam-sync")]
use features::spam_reputation::SpamReputationStore;
#[cfg(feature = "moderation")]
use features::spam_review::{
    apply_callback, callback_message_chat_id, is_chat_admin, parse_callback,
    process_next_review_delivery, review_delivery_enabled,
};
use features::spammer_avatar_embeddings::{
    enqueue_existing_spammer_avatars, process_next_spammer_avatar_embedding,
    worker_error_seconds as spammer_avatar_worker_error_seconds,
    worker_idle_seconds as spammer_avatar_worker_idle_seconds,
};
use features::user_profiles::enrichment::{
    ProfileRefreshEnqueueResult, ProfileRefreshQueue, spawn_profile_refresh_workers,
};
#[cfg(feature = "voice")]
use features::voice::pipeline::{maybe_transcribe_voice, process_next_voice_job};
use llm::genai_transport::GenAiTransport;
use state::AppState;
use telegram::command_handler::{handle_command, handle_reply_user_stats_command};
use telegram::commands::Command;

// Keep the production update tree shared with the route tests below. Tests
// replace only the terminal handlers, so they exercise the actual teloxide
// filters and branch precedence without touching PostgreSQL or Telegram.
macro_rules! update_handler_tree {
    ($command:expr, $message:expr, $reaction:expr, $reaction_count:expr, $edited:expr, $callback:expr, $member:expr) => {{
        let handler = dptree::entry()
            .branch(
                Update::filter_message()
                    .branch(
                        dptree::entry()
                            .filter_command::<Command>()
                            .endpoint($command),
                    )
                    .branch(dptree::endpoint($message)),
            )
            .branch(Update::filter_message_reaction_updated().endpoint($reaction))
            .branch(Update::filter_message_reaction_count_updated().endpoint($reaction_count))
            .branch(Update::filter_edited_message().endpoint($edited));
        #[cfg(feature = "moderation")]
        let handler = handler.branch(Update::filter_callback_query().endpoint($callback));
        handler.branch(Update::filter_chat_member().endpoint($member))
    }};
}

#[cfg(test)]
#[path = "../tests/support/quality/generators.rs"]
#[allow(dead_code)]
mod quality_generators;

#[cfg(test)]
mod update_route_tests {
    use std::ops::ControlFlow;

    use proptest::{prelude::*, test_runner::TestRunner};
    use serde_json::{Value, json};
    use teloxide::{
        prelude::*,
        types::{Me, MessageReactionCountUpdated, MessageReactionUpdated, Update},
    };

    use super::Command;
    use crate::quality_generators;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Route {
        Command,
        Message,
        Edited,
        Reaction,
        ReactionCount,
        #[cfg(feature = "moderation")]
        Callback,
        Member,
    }

    async fn command(_: Message, _: Command) -> ResponseResult<Route> {
        Ok(Route::Command)
    }
    async fn message(_: Message) -> ResponseResult<Route> {
        Ok(Route::Message)
    }
    async fn edited(_: Message) -> ResponseResult<Route> {
        Ok(Route::Edited)
    }
    async fn reaction(_: MessageReactionUpdated) -> ResponseResult<Route> {
        Ok(Route::Reaction)
    }
    async fn reaction_count(_: MessageReactionCountUpdated) -> ResponseResult<Route> {
        Ok(Route::ReactionCount)
    }
    #[cfg(feature = "moderation")]
    async fn callback(_: CallbackQuery) -> ResponseResult<Route> {
        Ok(Route::Callback)
    }
    async fn member(_: ChatMemberUpdated) -> ResponseResult<Route> {
        Ok(Route::Member)
    }

    fn decode_update(value: Value) -> Update {
        serde_json::from_value(value).expect("synthetic Telegram Update must match teloxide schema")
    }

    fn synthetic_bot_identity() -> Me {
        serde_json::from_value(json!({
            "id": 99,
            "is_bot": true,
            "first_name": "SyntheticBot",
            "username": "synthetic_bot",
            "can_join_groups": true,
            "can_read_all_group_messages": false,
            "supports_inline_queries": false,
            "has_main_web_app": false
        }))
        .expect("synthetic bot identity must match teloxide schema")
    }

    async fn dispatch(
        update: Update,
    ) -> ControlFlow<ResponseResult<Route>, dptree::di::DependencyMap> {
        update_handler_tree!(
            command,
            message,
            reaction,
            reaction_count,
            edited,
            callback,
            member
        )
        .dispatch(dptree::deps![update, synthetic_bot_identity()])
        .await
    }

    fn message_update(update_id: u32, message_id: i32, text: &str) -> Update {
        decode_update(json!({
            "update_id": update_id,
            "message": {
                "message_id": message_id,
                "date": 1_700_000_000,
                "chat": {"id": -100123, "type": "supergroup", "title": "Synthetic"},
                "from": {"id": 42, "is_bot": false, "first_name": "Synthetic"},
                "text": text
            }
        }))
    }

    #[tokio::test]
    async fn command_filter_has_precedence_and_unknown_command_is_ordinary_message() {
        let command = dispatch(message_update(1, 10, "/ping")).await;
        assert!(matches!(command, ControlFlow::Break(Ok(Route::Command))));

        let unknown = dispatch(message_update(2, 11, "/not_a_command arg")).await;
        assert!(matches!(unknown, ControlFlow::Break(Ok(Route::Message))));

        let plain = dispatch(message_update(3, 12, "hello")).await;
        assert!(matches!(plain, ControlFlow::Break(Ok(Route::Message))));
    }

    #[tokio::test]
    async fn edited_message_routes_to_edit_handler_without_falling_through_to_ingest() {
        let mut update = serde_json::to_value(message_update(4, 13, "edited text")).unwrap();
        let message = update.as_object_mut().unwrap().remove("message").unwrap();
        update["edited_message"] = message;
        let route = dispatch(decode_update(update)).await;
        assert!(matches!(route, ControlFlow::Break(Ok(Route::Edited))));
    }

    #[tokio::test]
    async fn callback_and_reaction_updates_reach_only_their_matching_routes() {
        #[cfg(feature = "moderation")]
        let callback = decode_update(json!({
            "update_id": 5,
            "callback_query": {
                "id": "synthetic-query",
                "from": {"id": 42, "is_bot": false, "first_name": "Synthetic"},
                "chat_instance": "synthetic-chat-instance",
                "data": "unsupported:test"
            }
        }));
        #[cfg(feature = "moderation")]
        assert!(matches!(
            dispatch(callback).await,
            ControlFlow::Break(Ok(Route::Callback))
        ));

        let reaction = decode_update(json!({
            "update_id": 6,
            "message_reaction": {
                "chat": {"id": -100123, "type": "supergroup", "title": "Synthetic"},
                "message_id": 14,
                "user": {"id": 42, "is_bot": false, "first_name": "Synthetic"},
                "date": 1_700_000_000,
                "old_reaction": [],
                "new_reaction": []
            }
        }));
        assert!(matches!(
            dispatch(reaction).await,
            ControlFlow::Break(Ok(Route::Reaction))
        ));

        let reaction_count = decode_update(json!({
            "update_id": 7,
            "message_reaction_count": {
                "chat": {"id": -100123, "type": "supergroup", "title": "Synthetic"},
                "message_id": 14,
                "date": 1_700_000_000,
                "reactions": []
            }
        }));
        assert!(matches!(
            dispatch(reaction_count).await,
            ControlFlow::Break(Ok(Route::ReactionCount))
        ));

        let actor = json!({"id": 42, "is_bot": false, "first_name": "Synthetic"});
        let member = decode_update(json!({
            "update_id": 8,
            "chat_member": {
                "chat": {"id": -100123, "type": "supergroup", "title": "Synthetic"},
                "from": actor,
                "date": 1_700_000_000,
                "old_chat_member": {"status": "left", "user": actor},
                "new_chat_member": {"status": "left", "user": actor}
            }
        }));
        assert!(matches!(
            dispatch(member).await,
            ControlFlow::Break(Ok(Route::Member))
        ));
    }

    #[cfg(feature = "moderation")]
    #[tokio::test]
    async fn unsupported_callback_is_acknowledged_once_through_local_telegram_fake() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0, "Telegram fake closed before receiving a request");
                request.extend_from_slice(&buffer[..count]);
                let request_text = String::from_utf8_lossy(&request);
                let Some((headers, body)) = request_text.split_once("\r\n\r\n") else {
                    continue;
                };
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or_default();
                if body.len() >= content_length {
                    break;
                }
            }
            let response_body = r#"{"ok":true,"result":true}"#;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        response_body.len(),
                        response_body
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });

        let api_url = format!("http://{address}/").parse().unwrap();
        let bot = Bot::new("synthetic-token").set_api_url(api_url);
        super::answer_unsupported_callback(
            &bot,
            teloxide::types::CallbackQueryId("synthetic-query-id".to_owned()),
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        assert_eq!(
            request.matches("AnswerCallbackQuery").count(),
            1,
            "request was: {request}"
        );
        assert!(request.contains("synthetic-query-id"));
    }

    #[test]
    fn structurally_valid_generated_messages_preserve_command_precedence() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let seed = std::env::var("NEDOBOT_PROPTEST_SEED")
            .map(|value| {
                value
                    .parse::<u64>()
                    .expect("NEDOBOT_PROPTEST_SEED must be an unsigned integer")
            })
            .unwrap_or(0x5445_4c45_4752_414d);
        let sha = std::env::var("GITHUB_SHA")
            .or_else(|_| std::env::var("NEDOBOT_TEST_SHA"))
            .unwrap_or_else(|_| "local".to_owned());
        let mut runner = TestRunner::new(proptest::test_runner::Config {
            cases: 192,
            rng_seed: proptest::test_runner::RngSeed::Fixed(seed),
            failure_persistence: None,
            ..Default::default()
        });
        let strategy = quality_generators::schema_valid_update();
        runner
            .run(&strategy, |case| {
                let mut ordinary_json = case.to_json();
                ordinary_json["message"]["text"] = json!(format!("synthetic {}", case.update_id));
                ordinary_json["message"]
                    .as_object_mut()
                    .unwrap()
                    .remove("entities");
                let ordinary = decode_update(ordinary_json);
                let mut command_json = case.to_json();
                command_json["message"]["text"] = json!("/ping");
                command_json["message"]
                    .as_object_mut()
                    .unwrap()
                    .remove("entities");
                let command = decode_update(command_json);

                runtime.block_on(async {
                    prop_assert!(matches!(
                        dispatch(ordinary).await,
                        ControlFlow::Break(Ok(Route::Message))
                    ));
                    prop_assert!(matches!(
                        dispatch(command).await,
                        ControlFlow::Break(Ok(Route::Command))
                    ));
                    Ok(())
                })
            })
            .unwrap_or_else(|error| {
                panic!(
                    "case_id=telegram-route-command-precedence seed={seed} sha={} replay=sh scripts/replay_quality_case.sh telegram-route-command-precedence {seed}\n{error}",
                    sha
                )
            });
    }

    #[tokio::test]
    async fn unsupported_update_kind_has_no_route_or_effect() {
        let update = decode_update(
            json!({"update_id": 7, "poll": {"id":"p", "question":"q", "options":[]}}),
        );
        assert!(matches!(dispatch(update).await, ControlFlow::Continue(_)));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let explicit_env_file = std::env::var("ENV_FILE").ok();
    let env_file = explicit_env_file.as_deref().unwrap_or(".env");
    if explicit_env_file.is_some() {
        dotenvy::from_filename(env_file)
            .map_err(|error| anyhow::anyhow!("failed to load {env_file}: {error}"))?;
    } else {
        dotenvy::dotenv().ok();
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,teloxide=info".into()),
        )
        .init();

    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    #[cfg(feature = "ask")]
    if config.ask_python_sandbox_enabled {
        features::ask::python_sandbox::AskPythonSandbox::validate_runtime(&config).await?;
    }
    tracing::info!(
        instance_id = %config.community.instance.id,
        instance_name = %config.community.instance.display_name,
        risk_profile = ?config.community.moderation.risk_profile,
        "starting configured community instance"
    );
    for chat_id in config.managed_chat_ids() {
        if let Some(chat) = config.chat_by_id(chat_id) {
            tracing::info!(chat_key = %chat.key, chat_id, "managed Telegram chat");
        }
    }
    GenAiTransport::cached(config.llm_proxy_url.as_deref())?;
    let bot = Bot::from_env().parse_mode(ParseMode::Html);
    #[cfg(feature = "ask")]
    let bot_identity = if config.community.ask.gateway.mention_enabled
        || config.community.ask.gateway.reply_to_bot_enabled
    {
        let me = bot.get_me().await?;
        if config.community.ask.gateway.mention_enabled && me.username().is_empty() {
            anyhow::bail!(
                "ask.gateway.mention_enabled requires a Telegram bot username from getMe"
            );
        }
        Some(state::TelegramBotIdentity {
            user_id: me.id.0 as i64,
            username: Some(me.username().to_owned()),
        })
    } else {
        None
    };
    preflight_managed_chats(&bot, &config).await?;
    preflight_moderation_message_deletion(&bot, &config).await?;
    #[cfg(feature = "moderation")]
    preflight_risk_captcha_permissions(&bot, &config).await?;
    let pool = build_pool().await?;
    migrate(&pool).await?;
    #[cfg(feature = "moderation")]
    if config.community.moderation.enabled {
        if let Err(err) =
            features::url_relations::refresh_configured_linked_chats(bot.inner(), &pool, &config)
                .await
        {
            tracing::warn!(%err, "failed to refresh configured Telegram linked chats at startup");
        }
        features::url_relations::spawn_linked_chat_refresh_worker(
            bot.inner().clone(),
            pool.clone(),
            config.clone(),
        );
    }
    if let Err(err) = refresh_known_member_snapshots(&bot, &pool, &config).await {
        tracing::warn!(%err, "failed to refresh member snapshots");
    }
    if let Err(err) = warn_if_group_update_permissions_unavailable(&bot, &config).await {
        tracing::warn!(%err, "failed to check Telegram group privacy and update permissions");
    }
    if let Err(err) = warn_if_join_leave_cleanup_unavailable(&bot, &config).await {
        tracing::warn!(%err, "failed to check join/leave message cleanup availability");
    }
    if let Err(err) = warn_if_ephemeral_and_greetings_unavailable(&bot, &config).await {
        tracing::warn!(%err, "failed to check ephemeral replies and welcome messages availability");
    }
    #[cfg(feature = "spam-sync")]
    let spam_reputation_store = if config.community.spam_reputation.enabled {
        let path = config
            .community
            .spam_reputation
            .sqlite_path
            .as_deref()
            .expect("enabled spam reputation config was validated");
        let store = SpamReputationStore::connect(path).await?;
        let chat_ids = community_reputation_chat_ids(&config);
        match store
            .synchronize_instance(&pool, &config.community.instance.id, &chat_ids)
            .await
        {
            Ok(summary) => tracing::info!(
                spammer_count = summary.spammer_count,
                source_decision_count = summary.source_decision_count,
                "synchronized shared spam reputation at startup"
            ),
            Err(err) => {
                tracing::warn!(%err, "initial shared spam reputation sync failed; background worker will retry")
            }
        }
        Some(store)
    } else {
        None
    };

    let state = AppState::new(pool, config);
    #[cfg(feature = "moderation")]
    if state.config.community.moderation.url_feed_enabled {
        features::url_feeds::spawn_worker(state.url_feed_snapshot.clone(), state.config.clone());
    }
    #[cfg(feature = "ask")]
    let state = match bot_identity {
        Some(identity) => state.with_telegram_bot_identity(identity),
        None => state,
    };
    #[cfg(feature = "ask")]
    spawn_ask_memory_expiry_worker(state.pool.clone());
    #[cfg(feature = "spam-sync")]
    let state = match spam_reputation_store {
        Some(store) => state.with_spam_reputation(store),
        None => state,
    };
    #[cfg(feature = "spam-sync")]
    spawn_spam_reputation_worker(state.clone());
    #[cfg(feature = "moderation")]
    if state.config.community.moderation.qr_evidence_enabled {
        features::qr_url::spawn_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    features::decision_audit::service::spawn_worker(
        bot.inner().clone(),
        state.pool.clone(),
        state.config.clone(),
    )?;
    let profile_refresh_queue = spawn_profile_refresh_workers(
        bot.inner().clone(),
        state.pool.clone(),
        state.config.clone(),
        #[cfg(feature = "moderation")]
        state.url_feed_snapshot.clone(),
    );
    spawn_spammer_avatar_embedding_worker(bot.inner().clone(), state.clone());
    #[cfg(feature = "moderation")]
    if state.config.new_user_audit_enabled {
        spawn_new_user_audit_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state.config.community.moderation.captcha_enabled
        && !state.config.community.moderation.captcha_dry_run
    {
        spawn_risk_captcha_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state.config.new_user_audit_enabled
        || review_delivery_enabled(&state.config.community.moderation)
    {
        spawn_spam_review_delivery_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state
        .config
        .community
        .chats
        .values()
        .any(|chat| chat.reports)
    {
        spawn_report_delivery_worker(bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "auto-comment")]
    if state.config.community.first_comment.enabled {
        spawn_post_comment_worker(bot.clone(), state.clone());
    }
    spawn_post_history_worker(state.clone());
    spawn_chat_retrieval_embedding_worker(state.clone());
    #[cfg(feature = "voice")]
    spawn_voice_transcription_worker(bot.clone(), state.clone());

    let handler = update_handler_tree!(
        handle_command,
        handle_message,
        handle_message_reaction,
        handle_message_reaction_count,
        handle_edited_message,
        handle_callback_query,
        handle_chat_member
    );

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state, profile_refresh_queue])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

#[cfg(feature = "ask")]
fn spawn_ask_memory_expiry_worker(pool: sqlx::PgPool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        loop {
            interval.tick().await;
            match features::ask::conversation_memory::purge_expired(&pool).await {
                Ok(deleted) if deleted > 0 => {
                    tracing::info!(deleted, "purged expired /ask conversation memory turns");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "failed to purge expired /ask conversation memory")
                }
            }
        }
    });
}

#[cfg(feature = "spam-sync")]
fn community_reputation_chat_ids(config: &Config) -> Vec<i64> {
    config
        .community
        .chats
        .values()
        .map(|chat| chat.id)
        .collect()
}

#[cfg(feature = "spam-sync")]
fn spawn_spam_reputation_worker(state: AppState) {
    let Some(store) = state.spam_reputation.clone() else {
        return;
    };
    let chat_ids = community_reputation_chat_ids(&state.config);
    let instance_id = state.config.community.instance.id.clone();
    tokio::spawn(async move {
        loop {
            match store
                .synchronize_instance(&state.pool, &instance_id, &chat_ids)
                .await
            {
                Ok(summary) => tracing::debug!(
                    spammer_count = summary.spammer_count,
                    source_decision_count = summary.source_decision_count,
                    "shared spam reputation synchronized"
                ),
                Err(err) => tracing::warn!(%err, "shared spam reputation sync failed"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

#[cfg(feature = "auto-comment")]
fn spawn_post_comment_worker(bot: teloxide::adaptors::DefaultParseMode<Bot>, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_post_comment_job(&bot, &state).await {
                Ok(true) => continue,
                Ok(false) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
                Err(err) => {
                    tracing::warn!(%err, "post comment worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
            }
        }
    });
}

fn spawn_post_history_worker(state: AppState) {
    if !state.config.rag_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_history_entry(&state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        POST_HISTORY_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "post history worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        POST_HISTORY_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

fn spawn_chat_retrieval_embedding_worker(state: AppState) {
    if !state.config.chat_retrieval_embeddings_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_embedding_batch(&state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        state.config.chat_retrieval_embedding_poll_sec,
                    ))
                    .await;
                }
                Err(err) => {
                    tracing::warn!(%err, "chat retrieval embedding worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        state.config.chat_retrieval_embedding_poll_sec,
                    ))
                    .await;
                }
            }
        }
    });
}

fn spawn_spammer_avatar_embedding_worker(bot: Bot, state: AppState) {
    if !state.config.avatar_embeddings_enabled {
        return;
    }
    tokio::spawn(async move {
        match enqueue_existing_spammer_avatars(&state.pool).await {
            Ok(queued) => tracing::info!(queued, "queued confirmed spammer avatars for embedding"),
            Err(err) => tracing::warn!(%err, "failed to queue existing confirmed spammer avatars"),
        }
        loop {
            match process_next_spammer_avatar_embedding(&bot, &state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        spammer_avatar_worker_idle_seconds(),
                    ))
                    .await;
                }
                Err(err) => {
                    tracing::warn!(%err, "spammer avatar embedding worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        spammer_avatar_worker_error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

#[cfg(feature = "voice")]
fn spawn_voice_transcription_worker(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    state: AppState,
) {
    if !state.config.voice_transcription_enabled {
        return;
    }
    tokio::spawn(async move {
        loop {
            match process_next_voice_job(&bot, &state).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        VOICE_TRANSCRIPTION_POLL.idle_seconds(),
                    ))
                    .await;
                }
                Err(err) => {
                    tracing::warn!(%err, "voice transcription worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        VOICE_TRANSCRIPTION_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

async fn handle_message(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
    profile_refresh_queue: ProfileRefreshQueue,
) -> ResponseResult<()> {
    let private_gateway_allowed = msg.chat.is_private()
        && state.config.community.ask.gateway.private_messages_enabled
        && msg.from.as_ref().is_some_and(|user| {
            state
                .config
                .ask_private_user_ids
                .contains(&(user.id.0 as i64))
        });
    if !is_managed_chat(&state.config, msg.chat.id.0) && !private_gateway_allowed {
        tracing::debug!(chat_id = msg.chat.id.0, "ignored message from unknown chat");
        return Ok(());
    }
    let mut message_ingested = false;
    if !msg.chat.is_private() {
        maybe_delete_join_leave_message(&bot, &msg, &state.config).await;
        match ingest_message(&state.pool, &msg, &state.config).await {
            Ok(true) => message_ingested = true,
            // Ask activation is independently gated by ask_enabled and
            // chats.<key>.ask, so ingest=false must not suppress an explicit
            // mention or direct reply trigger.
            Ok(false) => {}
            Err(err) => {
                tracing::error!(%err, chat_id = msg.chat.id.0, "failed to ingest message");
                return Ok(());
            }
        }
        if message_ingested {
            enqueue_message_author_profile_refresh(&msg, &state, &profile_refresh_queue);
        }
    }

    #[cfg(feature = "ask")]
    if !msg.chat.is_private() || private_gateway_allowed {
        let gateway = &state.config.community.ask.gateway;
        let chat_gateway_allowed = state.config.ask_enabled
            && managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.ask);
        if (chat_gateway_allowed || private_gateway_allowed)
            && let Some(question) = features::ask::gateway::invocation_question(
                &msg,
                state.telegram_bot_identity.as_ref(),
                gateway.mention_enabled,
                gateway.reply_to_bot_enabled,
                private_gateway_allowed && gateway.private_messages_enabled,
            )
        {
            if !msg.chat.is_private()
                && state.config.mcp_read_scope_for_chat(msg.chat.id.0).is_err()
            {
                return Ok(());
            }
            telegram::command_handler::handle_gateway_ask(&bot, &msg, &state, &question).await?;
            return Ok(());
        }
    }

    if !message_ingested {
        return Ok(());
    }

    if handle_reply_user_stats_command(bot.clone(), msg.clone(), state.clone()).await? {
        return Ok(());
    }

    #[cfg(feature = "voice")]
    match maybe_transcribe_voice(&bot, &msg, &state).await {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => tracing::error!(%err, "failed to process voice transcription"),
    }

    #[cfg(feature = "auto-comment")]
    if let Err(err) = maybe_comment_post(&msg, &state).await {
        tracing::error!(%err, "failed to process message");
    }

    Ok(())
}

fn enqueue_message_author_profile_refresh(
    msg: &Message,
    state: &AppState,
    profile_refresh_queue: &ProfileRefreshQueue,
) {
    if !managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.ingest)
        || msg.is_automatic_forward()
    {
        return;
    }

    let Some(user) = msg.from.as_ref() else {
        return;
    };
    if user.is_bot {
        return;
    }

    let user_id = user.id.0 as i64;
    match profile_refresh_queue.try_enqueue(msg.chat.id.0, user_id) {
        ProfileRefreshEnqueueResult::Queued => {}
        ProfileRefreshEnqueueResult::Coalesced => {
            tracing::debug!(user_id, "coalesced duplicate profile refresh event");
        }
        ProfileRefreshEnqueueResult::Full => {
            tracing::warn!(
                user_id,
                "skipped profile refresh because bounded queue is full"
            );
        }
        ProfileRefreshEnqueueResult::Closed => {
            tracing::warn!(user_id, "skipped profile refresh because queue is closed");
        }
    }
}

#[cfg(feature = "moderation")]
async fn handle_callback_query(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    query: CallbackQuery,
    state: AppState,
) -> ResponseResult<()> {
    if query
        .data
        .as_deref()
        .is_some_and(features::risk_captcha::is_callback_data)
    {
        return handle_risk_captcha_callback(&bot, &query, &state).await;
    }
    if let Some((report_id, action)) = query.data.as_deref().and_then(reports::parse_callback) {
        return handle_report_callback(&bot, &query, &state, report_id, action).await;
    }

    let Some((request_id, decision)) = query.data.as_deref().and_then(parse_callback) else {
        return answer_unsupported_callback(bot.inner(), query.id).await;
    };
    let reviewer_id = query.from.id.0 as i64;
    let is_configured_reviewer = state.config.reviewer_user_ids().contains(&reviewer_id)
        || state.config.owner_telegram_id == Some(reviewer_id);
    let is_review_chat_admin = if is_configured_reviewer {
        false
    } else if let Some(review_chat_id) = callback_message_chat_id(&query) {
        match is_chat_admin(bot.inner(), review_chat_id, reviewer_id).await {
            Ok(is_admin) => is_admin,
            Err(error) => {
                tracing::warn!(%error, review_chat_id, reviewer_id, "failed to verify spam review callback actor");
                bot.answer_callback_query(query.id)
                    .text("Не удалось проверить права администратора.")
                    .await?;
                return Ok(());
            }
        }
    } else {
        false
    };
    if !is_configured_reviewer && !is_review_chat_admin {
        bot.answer_callback_query(query.id)
            .text("Недостаточно прав.")
            .await?;
        return Ok(());
    }
    match apply_callback(
        &state.pool,
        request_id,
        decision,
        reviewer_id,
        state.config.avatar_embeddings_enabled,
    )
    .await
    {
        Ok(Some(text)) => {
            bot.answer_callback_query(query.id.clone())
                .text(text)
                .await?;
            if let Some(message) = query.regular_message() {
                bot.delete_message(message.chat.id, message.id).await?;
            }
        }
        Ok(None) => {
            bot.answer_callback_query(query.id)
                .text("Решение уже принято или кнопка устарела.")
                .await?;
        }
        Err(err) => {
            tracing::error!(%err, request_id, "failed to apply spam review callback");
            bot.answer_callback_query(query.id)
                .text("Не удалось сохранить решение.")
                .await?;
        }
    }
    Ok(())
}

#[cfg(feature = "moderation")]
async fn answer_unsupported_callback(
    bot: &Bot,
    callback_query_id: teloxide::types::CallbackQueryId,
) -> ResponseResult<()> {
    bot.answer_callback_query(callback_query_id).await?;
    Ok(())
}

#[cfg(feature = "moderation")]
async fn handle_risk_captcha_callback(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    query: &CallbackQuery,
    state: &AppState,
) -> ResponseResult<()> {
    let callback_chat_id = query.regular_message().map(|message| message.chat.id.0);
    let outcome = match features::risk_captcha::handle_callback(
        bot.inner(),
        &state.pool,
        callback_chat_id,
        query.from.id.0 as i64,
        query.data.as_deref().unwrap_or_default(),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(
                %error,
                user_id = query.from.id.0,
                "risk captcha callback failed"
            );
            bot.answer_callback_query(query.id.clone()).await?;
            return Ok(());
        }
    };

    let message = query.regular_message();
    let success_text = outcome.success_message();
    let answer = bot.answer_callback_query(query.id.clone());
    if let Some(success_text) = success_text
        && message.is_some_and(|message| message.ephemeral_message_id.is_none())
    {
        // Legacy public captcha cards are removed after deploy; show their
        // success result only to the participant who pressed the button.
        answer.text(success_text).show_alert(true).await?;
    } else {
        answer.await?;
    }

    if outcome.is_terminal()
        && let Some(message) = message
    {
        features::risk_captcha::finish_captcha_card(
            bot.inner(),
            message.chat.id,
            query.from.id,
            message.id,
            message.ephemeral_message_id,
            success_text,
        )
        .await;
    }
    Ok(())
}

#[cfg(feature = "moderation")]
async fn handle_report_callback(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    query: &CallbackQuery,
    state: &AppState,
    report_id: i64,
    action: reports::ReportAction,
) -> ResponseResult<()> {
    let actor_id = query.from.id.0 as i64;
    if !reports::is_admin_callback_actor(bot.inner(), &state.config, actor_id).await {
        bot.answer_callback_query(query.id.clone())
            .text("Недостаточно прав.")
            .await?;
        return Ok(());
    }

    match reports::apply_action(
        &state.pool,
        report_id,
        action,
        actor_id,
        state.config.avatar_embeddings_enabled,
    )
    .await
    {
        Ok(ReportActionResult::Applied(resolution)) => {
            let text = match resolution {
                reports::ReportResolution::Accepted => "Репорт принят.",
                reports::ReportResolution::Rejected => "Репорт отклонён.",
                reports::ReportResolution::Pending => "Решение сохранено.",
            };
            bot.answer_callback_query(query.id.clone())
                .text(text)
                .await?;
            if let Some(message) = query.regular_message() {
                match reports::load_report(&state.pool, report_id).await {
                    Ok(card) => {
                        if let Err(err) = bot
                            .inner()
                            .edit_message_rich_text(
                                message.chat.id,
                                message.id,
                                reports::render_report(&card),
                            )
                            .await
                        {
                            tracing::warn!(%err, report_id, "failed to update report card after decision");
                        }
                        if let Err(err) = bot
                            .inner()
                            .edit_message_reply_markup(message.chat.id, message.id)
                            .reply_markup(reports::render_report_keyboard(&card))
                            .await
                        {
                            tracing::warn!(%err, report_id, "failed to update report card buttons after decision");
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, report_id, "failed to reload report card after decision");
                    }
                }
            }
        }
        Ok(ReportActionResult::AlreadyResolved) => {
            bot.answer_callback_query(query.id.clone())
                .text("Решение уже принято.")
                .await?;
        }
        Ok(ReportActionResult::Missing) => {
            bot.answer_callback_query(query.id.clone())
                .text("Репорт не найден.")
                .await?;
        }
        Err(err) => {
            tracing::error!(%err, report_id, "failed to apply report callback");
            bot.answer_callback_query(query.id.clone())
                .text("Не удалось сохранить решение.")
                .await?;
        }
    }
    Ok(())
}

#[cfg(feature = "moderation")]
fn spawn_new_user_audit_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_new_user_audit_job(&bot, &state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "unified new user audit worker failed to claim a job");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

#[cfg(feature = "moderation")]
fn spawn_risk_captcha_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match features::risk_captcha::process_next_expired(&bot, &state.pool).await {
                Ok(true) => continue,
                Err(error) => {
                    tracing::warn!(%error, "risk captcha expiration worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                    continue;
                }
                Ok(false) => {}
            }
            match features::risk_captcha::process_next_setup(
                &bot,
                &state.pool,
                state.config.community.moderation.captcha_ttl_sec,
            )
            .await
            {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(error) => {
                    tracing::warn!(%error, "risk captcha setup worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

#[cfg(feature = "moderation")]
fn spawn_spam_review_delivery_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match process_next_review_delivery(&bot, &state.pool, &state.config).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "spam review delivery worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await;
                }
            }
        }
    });
}

#[cfg(feature = "moderation")]
fn spawn_report_delivery_worker(bot: Bot, state: AppState) {
    tokio::spawn(async move {
        loop {
            match reports::process_next_delivery(&bot, &state.pool).await {
                Ok(true) => continue,
                Ok(false) => {
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.idle_seconds(),
                    ))
                    .await
                }
                Err(err) => {
                    tracing::warn!(%err, "report delivery worker failed");
                    tokio::time::sleep(std::time::Duration::from_secs(
                        EXTERNAL_ANALYSIS_POLL.error_seconds(),
                    ))
                    .await
                }
            }
        }
    });
}

async fn handle_message_reaction(
    reaction: MessageReactionUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, reaction.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_message_reaction(&state.pool, &reaction).await {
        tracing::error!(%err, "failed to save message reaction");
    }

    Ok(())
}

async fn handle_message_reaction_count(
    reaction_count: MessageReactionCountUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, reaction_count.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    if let Err(err) = save_message_reaction_count(&state.pool, &reaction_count).await {
        tracing::error!(%err, "failed to save message reaction count");
    }

    Ok(())
}

async fn handle_edited_message(msg: Message, state: AppState) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    match save_edited_telegram_message(&state.pool, &msg, &state.config).await {
        Ok(()) => {
            #[cfg(feature = "moderation")]
            if state
                .config
                .chat_allows(msg.chat.id.0, |chat| chat.moderation)
                && let Some(user) = msg.from.as_ref().filter(|user| !user.is_bot)
            {
                let url_feed = state.url_feed_snapshot.read().await.clone();
                if let Err(error) =
                    features::new_user_analysis::refresh_url_shadow_observation_from_links(
                        &state.pool,
                        &state.config,
                        msg.chat.id.0,
                        user.id.0 as i64,
                        url_feed.as_deref(),
                    )
                    .await
                {
                    tracing::warn!(%error, chat_id = msg.chat.id.0, user_id = user.id.0, "failed to refresh URL shadow after message edit");
                }
            }
        }
        Err(err) => tracing::error!(%err, "failed to save edited message"),
    }

    Ok(())
}

async fn handle_chat_member(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    member: ChatMemberUpdated,
    state: AppState,
) -> ResponseResult<()> {
    if !is_managed_chat(&state.config, member.chat.id.0) {
        return Ok(());
    }
    if managed_chat_allows(&state.config, member.chat.id.0, |chat| chat.ingest)
        && let Err(err) = save_chat_member_event(&state.pool, &member).await
    {
        tracing::error!(%err, "failed to save chat member event");
    }
    if let Err(err) = handle_chat_member_update(&bot, &member, &state.config).await {
        tracing::warn!(%err, chat_id = member.chat.id.0, "failed to send configured welcome or farewell message");
    }

    Ok(())
}

async fn warn_if_group_update_permissions_unavailable(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    let me = bot.get_me().await?;
    for chat_id in config.managed_chat_ids().filter(|chat_id| *chat_id < 0) {
        let member = match bot.get_chat_member(ChatId(chat_id), me.id).await {
            Ok(member) => member,
            Err(err) => {
                tracing::warn!(
                    %err,
                    chat_id,
                    "could not check bot membership; reaction and chat_member updates may be unavailable"
                );
                continue;
            }
        };
        let is_admin = matches!(
            member.kind,
            ChatMemberKind::Administrator(_) | ChatMemberKind::Owner(_)
        );
        if !is_admin {
            tracing::warn!(
                chat_id,
                status = ?member.kind,
                "bot is not chat administrator; Telegram will not send message_reaction, message_reaction_count, or chat_member updates"
            );
        }

        if config.chat_allows(chat_id, |chat| chat.ingest)
            && privacy_mode_blocks_group_messages(me.can_read_all_group_messages, is_admin)
        {
            tracing::error!(
                chat_id,
                "bot cannot read ordinary group messages: disable Group Privacy in BotFather and re-add the bot, or make it a chat administrator; message ingestion, anti-spam context, and channel-post comments will miss updates"
            );
        }
    }

    Ok(())
}

fn privacy_mode_blocks_group_messages(privacy_disabled: bool, is_admin: bool) -> bool {
    !privacy_disabled && !is_admin
}

async fn warn_if_join_leave_cleanup_unavailable(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    let chat_ids = config
        .managed_chat_ids()
        .filter(|chat_id| config.chat_allows(*chat_id, |chat| chat.delete_join_leave_messages))
        .collect::<Vec<_>>();
    if chat_ids.is_empty() {
        return Ok(());
    }

    let me = bot.get_me().await?;
    for chat_id in chat_ids {
        match bot.get_chat_member(ChatId(chat_id), me.id).await {
            Ok(member) if member.kind.can_delete_messages() => {}
            Ok(member) => tracing::warn!(
                chat_id,
                status = ?member.kind,
                "join/leave cleanup is enabled but bot lacks can_delete_messages"
            ),
            Err(err) => tracing::warn!(
                %err,
                chat_id,
                "join/leave cleanup is enabled but bot permissions could not be checked"
            ),
        }
    }

    Ok(())
}

async fn warn_if_ephemeral_and_greetings_unavailable(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    let chat_ids = config
        .managed_chat_ids()
        .filter(|chat_id| {
            config.chat_by_id(*chat_id).is_some_and(|chat| {
                chat.config.ephemeral_command_replies
                    || chat
                        .config
                        .welcome_message
                        .as_deref()
                        .is_some_and(|message| !message.trim().is_empty())
                    || chat
                        .config
                        .farewell_message
                        .as_deref()
                        .is_some_and(|message| !message.trim().is_empty())
            })
        })
        .collect::<Vec<_>>();
    if chat_ids.is_empty() {
        return Ok(());
    }

    let me = bot.get_me().await?;
    for chat_id in chat_ids {
        match bot.get_chat_member(ChatId(chat_id), me.id).await {
            Ok(member)
                if matches!(
                    member.kind,
                    ChatMemberKind::Administrator(_) | ChatMemberKind::Owner(_)
                ) => {}
            Ok(member) => tracing::warn!(
                chat_id,
                status = ?member.kind,
                "ephemeral replies and member greetings require the bot to be a chat administrator"
            ),
            Err(err) => tracing::warn!(
                %err,
                chat_id,
                "ephemeral replies and member greetings are enabled but bot permissions could not be checked"
            ),
        }
    }

    Ok(())
}

async fn preflight_managed_chats(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    for chat_id in config.managed_chat_ids() {
        bot.get_chat(ChatId(chat_id))
            .await
            .map_err(|error| anyhow::anyhow!("managed chat {chat_id} preflight failed: {error}"))?;
    }
    Ok(())
}

async fn preflight_moderation_message_deletion(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    let chat_ids = config
        .managed_chat_ids()
        .filter(|chat_id| {
            config.chat_allows(*chat_id, |chat| {
                chat.moderation_delete_target_message || chat.moderation_delete_command_message
            })
        })
        .collect::<Vec<_>>();
    if chat_ids.is_empty() {
        return Ok(());
    }

    let bot_id = bot.get_me().await?.id;
    for chat_id in chat_ids {
        let member = bot
            .get_chat_member(ChatId(chat_id), bot_id)
            .await
            .map_err(|_| {
                anyhow::anyhow!("cannot verify message deletion permission in chat {chat_id}")
            })?;
        if !member.kind.can_delete_messages() {
            anyhow::bail!(
                "moderation message deletion is enabled for chat {chat_id}, but the bot lacks can_delete_messages"
            );
        }
    }
    Ok(())
}

#[cfg(feature = "moderation")]
async fn preflight_risk_captcha_permissions(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    config: &Config,
) -> anyhow::Result<()> {
    if !config.community.moderation.captcha_enabled || config.community.moderation.captcha_dry_run {
        return Ok(());
    }
    let bot_id = bot.get_me().await?.id;
    for chat_id in config
        .managed_chat_ids()
        .filter(|chat_id| config.chat_allows(*chat_id, |chat| chat.moderation))
    {
        let member = bot
            .get_chat_member(ChatId(chat_id), bot_id)
            .await
            .map_err(|_| anyhow::anyhow!("cannot verify captcha permissions in chat {chat_id}"))?;
        let has_permissions = match member.kind {
            ChatMemberKind::Owner(_) => true,
            ChatMemberKind::Administrator(admin) => {
                admin.can_delete_messages && admin.can_restrict_members
            }
            _ => false,
        };
        if !has_permissions {
            anyhow::bail!(
                "moderation.captcha_enabled=true requires the bot to have can_delete_messages and can_restrict_members in chat {chat_id}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::privacy_mode_blocks_group_messages;

    #[test]
    fn privacy_mode_only_blocks_group_ingress_when_bot_is_not_admin() {
        assert!(privacy_mode_blocks_group_messages(false, false));
        assert!(!privacy_mode_blocks_group_messages(true, false));
        assert!(!privacy_mode_blocks_group_messages(false, true));
    }
}
