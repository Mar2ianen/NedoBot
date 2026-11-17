use std::sync::Arc;
use teloxide::{
    dispatching::UpdateFilterExt,
    dptree,
    prelude::*,
    types::{ChatId, ChatMemberKind, ParseMode},
};

use tg_ai_bot_teloxide::{config, db, features, llm, state, telegram};

use config::Config;
use db::telegram::{refresh_known_member_snapshots, save_edited_telegram_message};
use db::{build_pool, migrate};
use features::chat_retrieval::process_next_embedding_batch;
#[cfg(feature = "auto-comment")]
use features::first_comment::pipeline::{maybe_comment_post, process_next_post_comment_job};
use features::ingest::{IngressOutcome, ingest_message, is_managed_chat, managed_chat_allows};
#[cfg(feature = "moderation")]
use features::jobs::policy::EXTERNAL_ANALYSIS_POLL;
#[cfg(feature = "voice")]
use features::jobs::policy::VOICE_TRANSCRIPTION_POLL;
use features::jobs::policy::{POST_HISTORY_POLL, WorkerPollPolicy};
use features::jobs::worker::{
    PollOutcome, WorkerId, WorkerSpec, WorkerSupervisor, run_fixed_delay_worker,
    run_interval_worker, run_poll_worker,
};
use features::memory::service::process_next_history_entry;
#[cfg(feature = "moderation")]
use features::new_user_audit::service::process_next_new_user_audit_job;
#[cfg(feature = "moderation")]
use features::reports;
use features::service_message_cleanup::maybe_delete_join_leave_message;
#[cfg(feature = "spam-sync")]
use features::spam_reputation::SpamReputationStore;
#[cfg(feature = "moderation")]
use features::spam_review::{process_next_review_delivery, review_delivery_enabled};
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
    let mut workers = WorkerSupervisor::new();
    #[cfg(feature = "moderation")]
    if config.community.moderation.enabled {
        if let Err(err) =
            features::url_relations::refresh_configured_linked_chats(bot.inner(), &pool, &config)
                .await
        {
            tracing::warn!(%err, "failed to refresh configured Telegram linked chats at startup");
        }
        features::url_relations::spawn_linked_chat_refresh_worker(
            &mut workers,
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
        features::url_feeds::spawn_worker(
            &mut workers,
            state.url_feed_snapshot.clone(),
            Arc::clone(&state.config),
        );
    }
    #[cfg(feature = "ask")]
    let state = match bot_identity {
        Some(identity) => state.with_telegram_bot_identity(identity),
        None => state,
    };
    #[cfg(feature = "ask")]
    spawn_ask_memory_expiry_worker(&mut workers, state.pool.clone());
    #[cfg(feature = "spam-sync")]
    let state = match spam_reputation_store {
        Some(store) => state.with_spam_reputation(store),
        None => state,
    };
    #[cfg(feature = "spam-sync")]
    spawn_spam_reputation_worker(&mut workers, state.clone());
    #[cfg(feature = "moderation")]
    if state.config.community.moderation.qr_evidence_enabled {
        features::qr_url::spawn_worker(&mut workers, bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    features::decision_audit::service::spawn_worker(
        &mut workers,
        bot.inner().clone(),
        state.pool.clone(),
        Arc::clone(&state.config),
    )?;
    let profile_refresh_queue = spawn_profile_refresh_workers(
        &mut workers,
        bot.inner().clone(),
        state.pool.clone(),
        Arc::clone(&state.config),
        #[cfg(feature = "moderation")]
        state.url_feed_snapshot.clone(),
    );
    spawn_spammer_avatar_embedding_worker(&mut workers, bot.inner().clone(), state.clone());
    #[cfg(feature = "moderation")]
    if state.config.new_user_audit_enabled {
        spawn_new_user_audit_worker(&mut workers, bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state.config.community.moderation.captcha_enabled
        && !state.config.community.moderation.captcha_dry_run
    {
        spawn_risk_captcha_worker(&mut workers, bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state.config.new_user_audit_enabled
        || review_delivery_enabled(&state.config.community.moderation)
    {
        spawn_spam_review_delivery_worker(&mut workers, bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "moderation")]
    if state
        .config
        .community
        .chats
        .values()
        .any(|chat| chat.reports)
    {
        spawn_report_delivery_worker(&mut workers, bot.inner().clone(), state.clone());
    }
    #[cfg(feature = "auto-comment")]
    if state.config.community.first_comment.enabled {
        spawn_post_comment_worker(&mut workers, bot.clone(), state.clone());
    }
    spawn_post_history_worker(&mut workers, state.clone());
    spawn_chat_retrieval_embedding_worker(&mut workers, state.clone());
    #[cfg(feature = "voice")]
    spawn_voice_transcription_worker(&mut workers, bot.clone(), state.clone());

    let handler = telegram::router::build(telegram::router::Routes {
        command: dptree::entry()
            .filter_command::<Command>()
            .endpoint(handle_command),
        message: dptree::endpoint(handle_message),
        reaction: Update::filter_message_reaction_updated()
            .endpoint(telegram::updates::handle_message_reaction),
        reaction_count: Update::filter_message_reaction_count_updated()
            .endpoint(telegram::updates::handle_message_reaction_count),
        edited_message: Update::filter_edited_message().endpoint(handle_edited_message),
        #[cfg(feature = "moderation")]
        callback: Update::filter_callback_query()
            .endpoint(telegram::callbacks::handle_callback_query),
        chat_member: Update::filter_chat_member().endpoint(telegram::updates::handle_chat_member),
    });

    let mut dispatcher = Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![state, profile_refresh_queue])
        .enable_ctrlc_handler()
        .build();

    workers.supervise_until(dispatcher.dispatch()).await?;

    Ok(())
}

#[cfg(feature = "ask")]
fn spawn_ask_memory_expiry_worker(supervisor: &mut WorkerSupervisor, pool: sqlx::PgPool) {
    supervisor.spawn(WorkerId::AskMemoryExpiry, move |shutdown| async move {
        run_interval_worker(
            WorkerId::AskMemoryExpiry,
            shutdown,
            std::time::Duration::from_secs(60 * 60),
            false,
            false,
            || async {
                match features::ask::conversation_memory::purge_expired(&pool).await {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!(deleted, "purged expired /ask conversation memory turns");
                        None
                    }
                    Ok(_) => None,
                    Err(_) => Some("database_operation_failed"),
                }
            },
        )
        .await;
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
fn spawn_spam_reputation_worker(supervisor: &mut WorkerSupervisor, state: AppState) {
    let Some(store) = state.spam_reputation.clone() else {
        return;
    };
    let chat_ids = community_reputation_chat_ids(&state.config);
    let instance_id = state.config.community.instance.id.clone();
    supervisor.spawn(WorkerId::SpamReputationSync, move |shutdown| async move {
        run_fixed_delay_worker(
            WorkerId::SpamReputationSync,
            shutdown,
            std::time::Duration::from_secs(30),
            || async {
                match store
                    .synchronize_instance(&state.pool, &instance_id, &chat_ids)
                    .await
                {
                    Ok(summary) => {
                        tracing::debug!(
                            spammer_count = summary.spammer_count,
                            source_decision_count = summary.source_decision_count,
                            "shared spam reputation synchronized"
                        );
                        None
                    }
                    Err(_) => Some("synchronization_failed"),
                }
            },
        )
        .await;
    });
}

#[cfg(feature = "auto-comment")]
fn spawn_post_comment_worker(
    supervisor: &mut WorkerSupervisor,
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    state: AppState,
) {
    supervisor.spawn(WorkerId::FirstComment, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::FirstComment, WorkerPollPolicy::new(2, 2)),
            shutdown,
            || async {
                match process_next_post_comment_job(&bot, &state).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("claim_or_process_failed"),
                }
            },
        )
        .await;
    });
}

fn spawn_post_history_worker(supervisor: &mut WorkerSupervisor, state: AppState) {
    if !state.config.rag_enabled {
        return;
    }
    supervisor.spawn(WorkerId::PostHistory, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::PostHistory, POST_HISTORY_POLL),
            shutdown,
            || async {
                match process_next_history_entry(&state.pool, &state.config).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("claim_or_process_failed"),
                }
            },
        )
        .await;
    });
}

fn spawn_chat_retrieval_embedding_worker(supervisor: &mut WorkerSupervisor, state: AppState) {
    if !state.config.chat_retrieval_embeddings_enabled {
        return;
    }
    let poll = state.config.chat_retrieval_embedding_poll_sec;
    supervisor.spawn(
        WorkerId::ChatRetrievalEmbedding,
        move |shutdown| async move {
            run_poll_worker(
                WorkerSpec::new(
                    WorkerId::ChatRetrievalEmbedding,
                    WorkerPollPolicy::new(poll, poll),
                ),
                shutdown,
                || async {
                    match process_next_embedding_batch(&state.pool, &state.config).await {
                        Ok(true) => PollOutcome::Processed,
                        Ok(false) => PollOutcome::Idle,
                        Err(_) => PollOutcome::Failed("claim_or_process_failed"),
                    }
                },
            )
            .await;
        },
    );
}

fn spawn_spammer_avatar_embedding_worker(
    supervisor: &mut WorkerSupervisor,
    bot: Bot,
    state: AppState,
) {
    if !state.config.avatar_embeddings_enabled {
        return;
    }
    supervisor.spawn(
        WorkerId::SpammerAvatarEmbedding,
        move |shutdown| async move {
            match enqueue_existing_spammer_avatars(&state.pool).await {
                Ok(queued) => {
                    tracing::info!(queued, "queued confirmed spammer avatars for embedding")
                }
                Err(_) => tracing::warn!(
                    error_kind = "enqueue_failed",
                    "failed to queue existing confirmed spammer avatars"
                ),
            }
            run_poll_worker(
                WorkerSpec::new(
                    WorkerId::SpammerAvatarEmbedding,
                    WorkerPollPolicy::new(
                        spammer_avatar_worker_idle_seconds(),
                        spammer_avatar_worker_error_seconds(),
                    ),
                ),
                shutdown,
                || async {
                    match process_next_spammer_avatar_embedding(&bot, &state.pool, &state.config)
                        .await
                    {
                        Ok(true) => PollOutcome::Processed,
                        Ok(false) => PollOutcome::Idle,
                        Err(_) => PollOutcome::Failed("claim_or_process_failed"),
                    }
                },
            )
            .await;
        },
    );
}

#[cfg(feature = "voice")]
fn spawn_voice_transcription_worker(
    supervisor: &mut WorkerSupervisor,
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    state: AppState,
) {
    if !state.config.voice.enabled {
        return;
    }
    supervisor.spawn(WorkerId::VoiceTranscription, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::VoiceTranscription, VOICE_TRANSCRIPTION_POLL),
            shutdown,
            || async {
                match process_next_voice_job(&bot, &state).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("claim_or_process_failed"),
                }
            },
        )
        .await;
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
            Ok(IngressOutcome::Persisted(_)) => message_ingested = true,
            // Ask activation is independently gated by ask_enabled and
            // chats.<key>.ask, so ingest=false must not suppress an explicit
            // mention or direct reply trigger.
            Ok(IngressOutcome::Ignored { .. }) => {}
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
fn spawn_new_user_audit_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    supervisor.spawn(WorkerId::NewUserAudit, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::NewUserAudit, EXTERNAL_ANALYSIS_POLL),
            shutdown,
            || async {
                match process_next_new_user_audit_job(&bot, &state.pool, &state.config).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("claim_or_process_failed"),
                }
            },
        )
        .await;
    });
}

#[cfg(feature = "moderation")]
fn spawn_risk_captcha_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    supervisor.spawn(WorkerId::RiskCaptchaLifecycle, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::RiskCaptchaLifecycle, EXTERNAL_ANALYSIS_POLL),
            shutdown,
            || async {
                match features::risk_captcha::process_next_expired(&bot, &state.pool).await {
                    Ok(true) => return PollOutcome::Processed,
                    Err(_) => return PollOutcome::Failed("expiration_operation_failed"),
                    Ok(false) => {}
                }
                match features::risk_captcha::process_next_setup(
                    &bot,
                    &state.pool,
                    state.config.community.moderation.captcha_ttl_sec,
                )
                .await
                {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("setup_operation_failed"),
                }
            },
        )
        .await;
    });
}

#[cfg(feature = "moderation")]
fn spawn_spam_review_delivery_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    supervisor.spawn(WorkerId::SpamReviewDelivery, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::SpamReviewDelivery, EXTERNAL_ANALYSIS_POLL),
            shutdown,
            || async {
                match process_next_review_delivery(&bot, &state.pool, &state.config).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("delivery_operation_failed"),
                }
            },
        )
        .await;
    });
}

#[cfg(feature = "moderation")]
fn spawn_report_delivery_worker(supervisor: &mut WorkerSupervisor, bot: Bot, state: AppState) {
    supervisor.spawn(WorkerId::ReportDelivery, move |shutdown| async move {
        run_poll_worker(
            WorkerSpec::new(WorkerId::ReportDelivery, EXTERNAL_ANALYSIS_POLL),
            shutdown,
            || async {
                match reports::process_next_delivery(&bot, &state.pool).await {
                    Ok(true) => PollOutcome::Processed,
                    Ok(false) => PollOutcome::Idle,
                    Err(_) => PollOutcome::Failed("delivery_operation_failed"),
                }
            },
        )
        .await;
    });
}

async fn handle_edited_message(msg: Message, state: AppState) -> ResponseResult<()> {
    if !managed_chat_allows(&state.config, msg.chat.id.0, |chat| chat.ingest) {
        return Ok(());
    }
    match save_edited_telegram_message(&state.pool, &msg, &state.config).await {
        Ok(_) => {
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
    use teloxide::types::{Update, UpdateKind};

    #[test]
    fn synthetic_update_shapes_cover_hidden_links_edits_bots_and_anonymous_senders() {
        let hidden_link: Update = serde_json::from_value(serde_json::json!({
            "update_id": 1002,
            "message": {
                "message_id": 1002,
                "date": 1700000000,
                "chat": {"id": -1001000000001_i64, "type": "supergroup"},
                "from": {"id": 900000002, "is_bot": false, "first_name": "Synthetic user"},
                "text": "read this",
                "entities": [{"type": "text_link", "offset": 0, "length": 4, "url": "https://example.org/landing"}]
            }
        }))
        .unwrap();
        let anonymous: Update = serde_json::from_value(serde_json::json!({
            "update_id": 1003,
            "message": {
                "message_id": 1003,
                "date": 1700000000,
                "chat": {"id": -1001000000001_i64, "type": "supergroup"},
                "sender_chat": {"id": -1001000000002_i64, "type": "channel", "title": "Synthetic sender"},
                "text": "anonymous synthetic post"
            }
        }))
        .unwrap();
        let bot_sender: Update = serde_json::from_value(serde_json::json!({
            "update_id": 1004,
            "message": {
                "message_id": 1004,
                "date": 1700000000,
                "chat": {"id": -1001000000001_i64, "type": "supergroup"},
                "from": {"id": 900000003, "is_bot": true, "first_name": "Synthetic helper"},
                "text": "synthetic bot message"
            }
        }))
        .unwrap();
        let edited: Update = serde_json::from_value(serde_json::json!({
            "update_id": 1005,
            "edited_message": {
                "message_id": 1005,
                "date": 1700000000,
                "edit_date": 1700000001,
                "chat": {"id": -1001000000001_i64, "type": "supergroup"},
                "from": {"id": 900000002, "is_bot": false, "first_name": "Synthetic user"},
                "text": "edited synthetic message"
            }
        }))
        .unwrap();

        let UpdateKind::Message(message) = hidden_link.kind else {
            panic!("hidden-link fixture must use the message update variant");
        };
        let entities = message
            .entities()
            .expect("text link fixture must retain text entities");
        assert_eq!(entities.len(), 1);
        assert!(matches!(
            &entities[0].kind,
            teloxide::types::MessageEntityKind::TextLink { url }
                if url.as_str() == "https://example.org/landing"
        ));

        let UpdateKind::Message(message) = anonymous.kind else {
            panic!("anonymous fixture must use the message update variant");
        };
        assert!(message.from.is_none());
        assert!(message.sender_chat.is_some());

        let UpdateKind::Message(message) = bot_sender.kind else {
            panic!("bot fixture must use the message update variant");
        };
        assert!(message.from.is_some_and(|sender| sender.is_bot));

        assert!(matches!(edited.kind, UpdateKind::EditedMessage(_)));
    }

    #[test]
    fn privacy_mode_only_blocks_group_ingress_when_bot_is_not_admin() {
        assert!(privacy_mode_blocks_group_messages(false, false));
        assert!(!privacy_mode_blocks_group_messages(true, false));
        assert!(!privacy_mode_blocks_group_messages(false, true));
    }
}
