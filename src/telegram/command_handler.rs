use std::future::Future;
use teloxide::{
    drafter::{DeliveryCertainty, DraftConfig, DraftFinishError, Drafter},
    prelude::*,
    types::{
        InputFile, InputRichBlock, InputRichBlockParagraph, InputRichBlockSectionHeading,
        InputRichBlockThinking, InputRichMessage, ReplyParameters, RichText,
    },
    utils::command::BotCommands,
};
use tokio::sync::mpsc;

use crate::db::telegram::save_telegram_message;
use crate::features::ask::chat_search::message_url;
use crate::features::ask::notes::{add_chat_note, add_user_note};
use crate::features::ask::repo;
use crate::features::ask::service::AskService;
use crate::features::ask::types::{AskCommandInput, AskFailureKind, AskProgress, AskRunStatus};
use crate::features::first_comment::clean::{clean_post_for_llm, should_generate_comment};
use crate::features::first_comment::pipeline::download_largest_photo_base64;
use crate::features::first_comment::render::build_comment_html;
use crate::features::memory::report::send_memory_notes;
use crate::features::stats::report::{
    UserStatsTarget, send_chat_stats, send_top_messages, send_top_reacted, send_user_stats,
};
use crate::features::stats::types::{StatsPeriod, StatsRender};
use crate::features::voice::pipeline::transcribe_reply;
use crate::state::AppState;
use crate::telegram::ask_drafter::AskDrafterBackend;
use crate::telegram::commands::Command;
use crate::telegram::custom_emoji::send_custom_emoji_ids;
use crate::telegram::html::TELEGRAM_TEXT_LIMIT;
use crate::telegram::render::{escape_html, send_html};

pub async fn handle_command(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    cmd: Command,
    state: AppState,
) -> ResponseResult<()> {
    let pool = &state.pool;
    let config = &state.config;

    if let Err(err) = save_telegram_message(pool, &msg, config).await {
        tracing::error!(%err, "failed to save command message");
    }

    match cmd {
        Command::Help => {
            send_html(
                &bot,
                msg.chat.id,
                escape_html(&Command::descriptions().to_string()),
            )
            .await?;
        }
        Command::Ping => {
            bot.send_message(msg.chat.id, "pong").await?;
        }
        Command::Db => {
            let row: (i64,) = sqlx::query_as("select 1::bigint")
                .fetch_one(pool)
                .await
                .map_err(|err| {
                    tracing::error!(%err, "database check failed");
                    teloxide::RequestError::Io(
                        std::io::Error::other("database check failed").into(),
                    )
                })?;

            bot.send_message(msg.chat.id, format!("db ok: {}", row.0))
                .await?;
        }
        Command::EmojiIds => {
            send_custom_emoji_ids(&bot, &msg).await?;
        }
        Command::FormatTest(post_text) => {
            if !should_generate_comment(&post_text, config) {
                bot.send_message(
                    msg.chat.id,
                    "Пропускаю: в посте нет сигнатуры обычного поста, похоже на рекламу или служебный пост.",
                )
                .await?;
                return Ok(());
            }

            let clean_post = clean_post_for_llm(&post_text, config);
            let text = build_comment_html(&clean_post, config);
            send_html(&bot, msg.chat.id, text).await?;
        }
        Command::Memory => {
            send_memory_notes(&bot, msg.chat.id, pool).await?;
        }
        Command::Transcribe => {
            transcribe_reply(&bot, &msg, &state).await.map_err(|err| {
                tracing::error!(%err, "manual voice transcription command failed");
                teloxide::RequestError::Io(
                    std::io::Error::other("manual voice transcription failed").into(),
                )
            })?;
        }
        Command::Ask(question) => {
            handle_ask_command(&bot, &msg, &state, &question).await?;
        }
        Command::ChatNote(note) => {
            handle_note_command(&bot, &msg, &state, &note, None).await?;
        }
        Command::UserNote(note) => {
            handle_note_command(&bot, &msg, &state, &note, reply_user_id(&msg)).await?;
        }
        Command::StatsDay(args) => {
            let render = render_from_message_or_args(&msg, &args);
            send_chat_stats(
                &bot,
                msg.chat.id,
                pool,
                config,
                &state.main_formatter,
                StatsPeriod::Day,
                render,
            )
            .await?;
        }
        Command::StatsWeek(args) => {
            let render = render_from_message_or_args(&msg, &args);
            send_chat_stats(
                &bot,
                msg.chat.id,
                pool,
                config,
                &state.main_formatter,
                StatsPeriod::Week,
                render,
            )
            .await?;
        }
        Command::StatsMonth(args) => {
            let render = render_from_message_or_args(&msg, &args);
            send_chat_stats(
                &bot,
                msg.chat.id,
                pool,
                config,
                &state.main_formatter,
                StatsPeriod::Month,
                render,
            )
            .await?;
        }
        Command::Status(args) => {
            let raw_args = raw_message_args(&msg).unwrap_or(args.as_str());
            let render = render_from_message_or_args(&msg, &args);
            let period = status_period_from_args(raw_args).unwrap_or(StatsPeriod::Day);
            send_chat_stats(
                &bot,
                msg.chat.id,
                pool,
                config,
                &state.main_formatter,
                period,
                render,
            )
            .await?;
        }
        Command::TopMsg(args) => {
            send_top_messages(
                &bot,
                msg.chat.id,
                pool,
                config,
                render_from_message_or_args(&msg, &args),
            )
            .await?;
        }
        Command::TopReact(args) => {
            send_top_reacted(
                &bot,
                msg.chat.id,
                pool,
                config,
                render_from_message_or_args(&msg, &args),
            )
            .await?;
        }
        Command::UserStats(target) | Command::UserStatus(target) => {
            let raw_args = raw_message_args(&msg).unwrap_or(target.as_str());
            let args = parse_user_stats_args(raw_args);
            let fallback_user_id = reply_user_id(&msg).or_else(|| sender_user_id(&msg));

            send_user_stats(
                &bot,
                msg.chat.id,
                pool,
                config,
                &state.main_formatter,
                UserStatsTarget {
                    target: args.target.as_deref(),
                    reply_user_id: fallback_user_id,
                },
                args.render,
            )
            .await?;
        }
    }

    Ok(())
}

async fn handle_note_command(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    note: &str,
    target_user_id: Option<i64>,
) -> ResponseResult<()> {
    let Some(author) = msg.from.as_ref() else {
        return Ok(());
    };
    if msg.chat.id.0 != state.config.discussion_chat_id {
        return Ok(());
    }
    let allowed = state.config.owner_telegram_id == Some(author.id.0 as i64)
        || (state.config.ask_allow_chat_admins
            && bot
                .get_chat_member(msg.chat.id, author.id)
                .await
                .map(|member| member.kind.is_privileged())
                .unwrap_or(false));
    if !allowed {
        return Ok(());
    }
    let result = match target_user_id {
        Some(target_user_id) => {
            add_user_note(
                &state.pool,
                msg.chat.id.0,
                target_user_id,
                author.id.0 as i64,
                note,
            )
            .await
        }
        None => add_chat_note(&state.pool, msg.chat.id.0, author.id.0 as i64, note).await,
    };
    match result {
        Ok(()) => send_html(bot, msg.chat.id, "Заметка сохранена.")
            .await
            .map(|_| ()),
        Err(_) => send_html(
            bot,
            msg.chat.id,
            "Не удалось сохранить заметку: проверь текст и reply для /user_note.",
        )
        .await
        .map(|_| ()),
    }
}

async fn handle_ask_command(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    question: &str,
) -> ResponseResult<()> {
    let config = &state.config;
    let Some(user) = msg.from.as_ref() else {
        return Ok(());
    };
    let is_private_allowed =
        msg.chat.is_private() && config.ask_private_user_ids.contains(&(user.id.0 as i64));
    let is_discussion_chat = msg.chat.id.0 == config.discussion_chat_id;
    if !config.ask_enabled || (!is_discussion_chat && !is_private_allowed) {
        return Ok(());
    }
    if question.trim().is_empty() {
        send_html(bot, msg.chat.id, "Напиши вопрос: /ask <вопрос>.").await?;
        return Ok(());
    }

    let use_native_draft = msg.chat.is_private();
    let permit = state.ask_slots.clone().try_acquire_owned().map_err(|_| {
        teloxide::RequestError::Io(std::io::Error::other("ask assistant is busy").into())
    })?;
    let backend = AskDrafterBackend::new(
        bot.clone(),
        msg.chat.id,
        user.id,
        use_native_draft,
        ReplyParameters::new(msg.id).allow_sending_without_reply(),
    );
    let (drafter, draft_sink) = Drafter::snapshots(
        backend,
        state.drafter_limiter.clone(),
        DraftConfig::default(),
    )
    .map_err(|err| {
        tracing::error!(%err, "failed to initialize /ask drafter");
        teloxide::RequestError::Io(std::io::Error::other("failed to initialize ask drafter").into())
    })?;
    if let Err(err) = draft_sink.update(ask_progress_preview(
        AskProgress::Preparing,
        use_native_draft,
    )) {
        tracing::debug!(%err, "failed to queue initial /ask progress preview");
    }
    if let Err(err) = drafter.flush().await {
        tracing::debug!(%err, "failed to deliver initial /ask progress preview");
    }
    let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
    let reply_context = build_ask_reply_context(msg, config.discussion_chat_id);
    let reply_image_base64 = match msg.reply_to_message() {
        Some(reply) => match download_largest_photo_base64(bot, reply, config).await {
            Ok(image) => image,
            Err(err) => {
                tracing::warn!(%err, "failed to download /ask reply image");
                None
            }
        },
        None => None,
    };
    let input = AskCommandInput {
        chat_id: msg.chat.id.0,
        command_message_id: msg.id.0,
        requester_user_id: user.id.0 as i64,
        requester_identity: requester_identity(user),
        question: question.to_owned(),
        reply_to_message_id: msg.reply_to_message().map(|reply| reply.id.0),
        reply_context,
        reply_image_base64,
        allow_mutations: true,
    };
    let ask_service = AskService::new(&state.pool, config, &state.llm_formatter);
    let answer = ask_service.execute(input, Some(&progress_tx));
    tokio::pin!(answer);
    let mut progress_open = true;
    let mut last_progress = AskProgress::Preparing;
    let answer = loop {
        tokio::select! {
            answer = &mut answer => break answer,
            update = progress_rx.recv(), if progress_open => match update {
                Some(update) if update != last_progress => {
                    last_progress = update;
                    if let Err(err) = draft_sink.update(ask_progress_preview(
                        update,
                        use_native_draft,
                    )) {
                        tracing::debug!(%err, "failed to update ask progress preview");
                    }
                }
                Some(_) => {}
                None => progress_open = false,
            }
        }
    };
    drop(permit);
    match answer {
        Ok(answer) => {
            let rendered = answer.rendered;
            match drafter.finish(rendered.rich_message).await {
                Ok(_) => {
                    record_ask_delivery(
                        state,
                        answer.ask_run_id,
                        AskRunStatus::Completed,
                        "rich_delivered",
                        None,
                    )
                    .await;
                    Ok(())
                }
                Err(err) => {
                    fallback_after_finish_error(
                        bot,
                        msg.chat.id,
                        state,
                        answer.ask_run_id,
                        AskRunStatus::Completed,
                        &rendered.fallback_text,
                        err,
                    )
                    .await
                }
            }
        }
        Err(err) => {
            tracing::warn!(%err, error_kind = err.kind.as_str(), "ask assistant failed");
            let ask_run_id = err.ask_run_id;
            let failure_message = ask_failure_message(err.kind);
            match drafter
                .finish(InputRichMessage::markdown(failure_message))
                .await
            {
                Ok(_) => {
                    record_ask_delivery(
                        state,
                        ask_run_id,
                        AskRunStatus::Failed,
                        "failure_message_delivered",
                        None,
                    )
                    .await;
                    Ok(())
                }
                Err(finish_err) => {
                    fallback_after_finish_error(
                        bot,
                        msg.chat.id,
                        state,
                        ask_run_id,
                        AskRunStatus::Failed,
                        failure_message,
                        finish_err,
                    )
                    .await
                }
            }
        }
    }
}

fn may_send_fallback(certainty: DeliveryCertainty) -> bool {
    matches!(
        certainty,
        DeliveryCertainty::NotAttempted | DeliveryCertainty::Rejected
    )
}

fn finish_error_certainty<E>(error: &DraftFinishError<E>) -> DeliveryCertainty {
    match error {
        DraftFinishError::WorkerStoppedBeforeCommand => DeliveryCertainty::NotAttempted,
        DraftFinishError::WorkerStoppedAfterCommand { delivery }
        | DraftFinishError::Backend { delivery, .. } => *delivery,
    }
}

async fn fallback_after_finish_error(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    state: &AppState,
    ask_run_id: Option<i64>,
    fallback_status: AskRunStatus,
    fallback_text: &str,
    error: DraftFinishError<teloxide::RequestError>,
) -> ResponseResult<()> {
    let certainty = finish_error_certainty(&error);
    if may_send_fallback(certainty) {
        tracing::warn!(?certainty, %error, "rich /ask delivery rejected; sending fallback");
    } else {
        tracing::error!(
            ?certainty,
            %error,
            ask_run_id,
            "rich /ask delivery is unknown; suppressing fallback"
        );
    }
    apply_finish_error_policy(
        certainty,
        fallback_status,
        || send_ask_fallback(bot, chat_id, fallback_text),
        || state.ask_delivery_metrics.record_unknown_delivery_failure(),
        |status, outcome, certainty| {
            record_ask_delivery(state, ask_run_id, status, outcome, certainty)
        },
    )
    .await
}

async fn apply_finish_error_policy<SendFallback, SendFuture, RecordDelivery, RecordFuture>(
    certainty: DeliveryCertainty,
    fallback_status: AskRunStatus,
    send_fallback: SendFallback,
    record_unknown: impl FnOnce(),
    record_delivery: RecordDelivery,
) -> ResponseResult<()>
where
    SendFallback: FnOnce() -> SendFuture,
    SendFuture: Future<Output = ResponseResult<()>>,
    RecordDelivery: FnOnce(AskRunStatus, &'static str, Option<DeliveryCertainty>) -> RecordFuture,
    RecordFuture: Future<Output = ()>,
{
    if !may_send_fallback(certainty) {
        record_unknown();
        record_delivery(
            AskRunStatus::Failed,
            "rich_delivery_unknown",
            Some(certainty),
        )
        .await;
        return Ok(());
    }

    let result = send_fallback().await;
    let (status, outcome) = if result.is_ok() {
        (fallback_status, "fallback_delivered")
    } else {
        (AskRunStatus::Failed, "fallback_failed")
    };
    record_delivery(status, outcome, Some(certainty)).await;
    result
}

async fn record_ask_delivery(
    state: &AppState,
    ask_run_id: Option<i64>,
    status: AskRunStatus,
    outcome: &str,
    certainty: Option<DeliveryCertainty>,
) {
    let Some(ask_run_id) = ask_run_id else {
        return;
    };
    if let Err(error) =
        repo::finish_delivery(&state.pool, ask_run_id, status, outcome, certainty).await
    {
        tracing::warn!(%error, ask_run_id, outcome, "failed to record ask delivery outcome");
    }
}

async fn send_ask_fallback(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    chat_id: ChatId,
    markdown: &str,
) -> ResponseResult<()> {
    if markdown.chars().count() <= TELEGRAM_TEXT_LIMIT {
        return send_html(bot, chat_id, escape_html(markdown))
            .await
            .map(|_| ());
    }

    bot.send_document(
        chat_id,
        InputFile::memory(markdown.as_bytes().to_vec()).file_name("ask-answer.md"),
    )
    .await
    .map(|_| ())
}

fn build_ask_reply_context(msg: &Message, discussion_chat_id: i64) -> Option<String> {
    msg.reply_to_message().map(|reply| {
        let author = reply
            .from
            .as_ref()
            .map(|user| {
                let name = format!(
                    "{}{}",
                    user.first_name,
                    user.last_name
                        .as_deref()
                        .map(|last_name| format!(" {last_name}"))
                        .unwrap_or_default()
                );
                format!(
                    "{name} (telegram_user_id={}, username={})",
                    user.id.0,
                    user.username.as_deref().unwrap_or("нет")
                )
            })
            .unwrap_or_else(|| "неизвестный автор".to_string());
        let media = [
            reply.photo().is_some().then_some("photo"),
            reply.video().is_some().then_some("video"),
            reply.document().is_some().then_some("document"),
            reply.voice().is_some().then_some("voice"),
            reply.audio().is_some().then_some("audio"),
            reply.sticker().is_some().then_some("sticker"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        format!(
            "message_id={}\nauthor={}\nmessage_url={}\nmedia={}\ntext={}",
            reply.id.0,
            author,
            (reply.chat.id.0 == discussion_chat_id)
                .then(|| message_url(discussion_chat_id, reply.id.0))
                .flatten()
                .as_deref()
                .unwrap_or("нет"),
            if media.is_empty() { "нет" } else { &media },
            reply
                .text()
                .or_else(|| reply.caption())
                .unwrap_or("[нет текста]")
        )
    })
}

fn ask_progress_message(progress: AskProgress) -> &'static str {
    match progress {
        AskProgress::Preparing => "⏳ Подготавливаю ответ…",
        AskProgress::ResolvingPerson => "🔎 Нахожу участника и проверяю профиль…",
        AskProgress::SearchingChat => "🔎 Ищу и сверяю сообщения в истории чата…",
        AskProgress::CheckingExternalSources => "🌐 Проверяю внешние источники…",
        AskProgress::CheckingNotes => "📝 Проверяю сохранённые заметки…",
        AskProgress::FormingAnswer => "✍️ Формирую ответ…",
    }
}

fn ask_progress_preview(progress: AskProgress, use_native_draft: bool) -> InputRichMessage {
    let message = ask_progress_message(progress);
    if !use_native_draft {
        return InputRichMessage::markdown(message);
    }

    InputRichMessage::blocks([
        InputRichBlock::Heading(InputRichBlockSectionHeading {
            text: RichText::from("NedoBot /ask"),
            size: 2,
        }),
        InputRichBlock::Thinking(InputRichBlockThinking {
            text: message.into(),
        }),
        InputRichBlock::Paragraph(InputRichBlockParagraph {
            text: "Исследование продолжается; финальный ответ появится в этом draft."
                .to_owned()
                .into(),
        }),
    ])
}

fn requester_identity(user: &teloxide::types::User) -> String {
    let mut identity = user.first_name.clone();
    if let Some(last_name) = user.last_name.as_deref().filter(|value| !value.is_empty()) {
        identity.push(' ');
        identity.push_str(last_name);
    }
    if let Some(username) = user.username.as_deref().filter(|value| !value.is_empty()) {
        identity.push_str(" (@");
        identity.push_str(username);
        identity.push(')');
    }
    identity.chars().take(120).collect()
}

fn ask_failure_message(kind: AskFailureKind) -> &'static str {
    match kind {
        AskFailureKind::Timeout => {
            "Помощник не уложился в таймаут. Попробуй сузить вопрос или повторить позже."
        }
        AskFailureKind::ToolError => {
            "Сейчас недоступен поиск по истории чата. Попробуй повторить запрос позже."
        }
        AskFailureKind::InvalidAction => {
            "Модель не смогла завершить агентный ответ. Запрос можно повторить без изменений."
        }
        AskFailureKind::InvalidOutput | AskFailureKind::GenerationError => {
            "Не смог подготовить ответ из-за временной ошибки модели или инструмента. Попробуй ещё раз чуть позже."
        }
    }
}

pub async fn handle_reply_user_stats_command(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    msg: Message,
    state: AppState,
) -> ResponseResult<bool> {
    if !is_bare_userstats_command(&msg) {
        return Ok(false);
    }

    let pool = &state.pool;
    let config = &state.config;

    if let Err(err) = save_telegram_message(pool, &msg, config).await {
        tracing::error!(%err, "failed to save command message");
    }

    let render = msg
        .text()
        .or_else(|| msg.caption())
        .map(render_from_args)
        .unwrap_or(StatsRender::Rich);

    send_user_stats(
        &bot,
        msg.chat.id,
        pool,
        config,
        &state.main_formatter,
        UserStatsTarget {
            target: None,
            reply_user_id: reply_user_id(&msg).or_else(|| sender_user_id(&msg)),
        },
        render,
    )
    .await?;

    Ok(true)
}

fn reply_user_id(msg: &Message) -> Option<i64> {
    msg.reply_to_message()
        .and_then(|reply| reply.from.as_ref())
        .map(|user| user.id.0 as i64)
}

fn sender_user_id(msg: &Message) -> Option<i64> {
    msg.from.as_ref().map(|user| user.id.0 as i64)
}

fn is_bare_userstats_command(msg: &Message) -> bool {
    let Some(text) = msg.text().or_else(|| msg.caption()) else {
        return false;
    };

    let mut parts = text.split_whitespace();
    let command = parts.next().unwrap_or_default();
    if parts.next().is_some() {
        return false;
    }

    matches!(command, "/userstats" | "/userstatus")
        || command
            .strip_prefix("/userstats@")
            .or_else(|| command.strip_prefix("/userstatus@"))
            .is_some_and(|bot_name| !bot_name.is_empty())
}

fn render_from_message_or_args(msg: &Message, args: &str) -> StatsRender {
    raw_message_args(msg)
        .filter(|raw_args| has_render_flag(raw_args))
        .map(render_from_args)
        .unwrap_or_else(|| render_from_args(args))
}

fn raw_message_args(msg: &Message) -> Option<&str> {
    msg.text()
        .or_else(|| msg.caption())
        .and_then(raw_command_args)
}

struct UserStatsArgs {
    target: Option<String>,
    render: StatsRender,
}

fn parse_user_stats_args(args: &str) -> UserStatsArgs {
    let target = strip_render_flag(args);
    UserStatsArgs {
        target: (!target.is_empty()).then_some(target),
        render: render_from_args(args),
    }
}

fn render_from_args(args: &str) -> StatsRender {
    if args.split_whitespace().any(is_plain_render_flag) {
        StatsRender::Html
    } else {
        StatsRender::Rich
    }
}

fn has_render_flag(args: &str) -> bool {
    args.split_whitespace()
        .any(|part| is_rich_render_flag(part) || is_plain_render_flag(part))
}

fn is_rich_render_flag(part: &str) -> bool {
    matches!(part, "-r" | "--rich")
}

fn is_plain_render_flag(part: &str) -> bool {
    matches!(part, "-p" | "--plain" | "--poor")
}

fn raw_command_args(text: &str) -> Option<&str> {
    let mut parts = text.trim().splitn(2, char::is_whitespace);
    parts.next()?;
    Some(parts.next().unwrap_or_default().trim())
}

fn strip_render_flag(args: &str) -> String {
    args.split_whitespace()
        .filter(|part| !is_rich_render_flag(part) && !is_plain_render_flag(part))
        .collect::<Vec<_>>()
        .join(" ")
}

fn status_period_from_args(args: &str) -> Option<StatsPeriod> {
    strip_render_flag(args)
        .split_whitespace()
        .next()
        .and_then(|period| match period.to_lowercase().as_str() {
            "day" | "daily" | "день" | "дня" => Some(StatsPeriod::Day),
            "week" | "weekly" | "неделя" | "неделю" => Some(StatsPeriod::Week),
            "month" | "monthly" | "месяц" => Some(StatsPeriod::Month),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rich_and_forced_plain_flags() {
        assert!(matches!(render_from_args("-r"), StatsRender::Rich));
        assert!(matches!(render_from_args("week --rich"), StatsRender::Rich));
        assert!(matches!(render_from_args("week"), StatsRender::Rich));
        assert!(matches!(render_from_args("week -p"), StatsRender::Html));
        assert!(matches!(
            render_from_args("--rich --poor"),
            StatsRender::Html
        ));
    }

    #[test]
    fn reads_raw_command_args_from_full_message_text() {
        assert_eq!(raw_command_args("/stats_day -r"), Some("-r"));
        assert_eq!(
            raw_command_args("/userstats 445144708 -r"),
            Some("445144708 -r")
        );
        assert_eq!(raw_command_args("/topmsg"), Some(""));
    }

    #[test]
    fn parses_user_stats_target_and_render_flags_in_any_position() {
        let cases = [
            ("@vasya -r", Some("@vasya"), StatsRender::Rich),
            ("--rich @vasya", Some("@vasya"), StatsRender::Rich),
            ("123 --plain", Some("123"), StatsRender::Html),
            ("--rich --plain 123", Some("123"), StatsRender::Html),
            ("-r", None, StatsRender::Rich),
            ("", None, StatsRender::Rich),
        ];

        for (input, expected_target, expected_render) in cases {
            let args = parse_user_stats_args(input);
            assert_eq!(args.target.as_deref(), expected_target, "input: {input}");
            assert_eq!(args.render, expected_render, "input: {input}");
        }
    }

    #[test]
    fn parses_status_period() {
        assert!(matches!(
            status_period_from_args("week -r"),
            Some(StatsPeriod::Week)
        ));
        assert!(matches!(
            status_period_from_args("месяц"),
            Some(StatsPeriod::Month)
        ));
    }

    #[test]
    fn ask_progress_uses_thinking_only_for_native_drafts() {
        let native = serde_json::to_value(ask_progress_preview(AskProgress::Preparing, true))
            .expect("native progress should serialize");
        let native_blocks = native["blocks"].as_array().expect("native blocks");
        assert!(
            native_blocks
                .iter()
                .any(|block| { block["type"].as_str() == Some("thinking") })
        );

        let edit = serde_json::to_value(ask_progress_preview(AskProgress::Preparing, false))
            .expect("edit progress should serialize");
        assert!(edit["markdown"].is_string());
        assert!(edit["blocks"].is_null());
    }

    #[test]
    fn fallback_policy_allows_only_confirmed_non_delivery() {
        assert!(may_send_fallback(DeliveryCertainty::NotAttempted));
        assert!(may_send_fallback(DeliveryCertainty::Rejected));
        assert!(!may_send_fallback(DeliveryCertainty::Unknown));
    }

    #[test]
    fn finish_error_certainty_is_preserved_for_fallback_policy() {
        let before = DraftFinishError::<teloxide::RequestError>::WorkerStoppedBeforeCommand;
        assert_eq!(
            finish_error_certainty(&before),
            DeliveryCertainty::NotAttempted
        );

        let after = DraftFinishError::<teloxide::RequestError>::WorkerStoppedAfterCommand {
            delivery: DeliveryCertainty::Unknown,
        };
        assert_eq!(finish_error_certainty(&after), DeliveryCertainty::Unknown);

        let rejected = DraftFinishError::Backend {
            source: teloxide::RequestError::MigrateToChatId(ChatId(42)),
            class: teloxide::drafter::DrafterErrorClass::Permanent,
            delivery: DeliveryCertainty::Rejected,
        };
        assert_eq!(
            finish_error_certainty(&rejected),
            DeliveryCertainty::Rejected
        );
    }

    #[tokio::test]
    async fn unknown_backend_delivery_suppresses_fallback_in_real_finish_path() {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        };

        use teloxide::drafter::{
            DrafterBackend, DrafterCapabilities, DrafterErrorClass, DrafterErrorDisposition,
            DrafterMode, DrafterOperation, DrafterPermit, DrafterPriority, DrafterRateLimitKey,
            DrafterRateLimitScope, DrafterRateLimiter, PreviewAck,
        };

        #[derive(Debug)]
        struct UnknownDeliveryError;

        impl std::fmt::Display for UnknownDeliveryError {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("connection lost after send")
            }
        }

        impl std::error::Error for UnknownDeliveryError {}

        #[derive(Clone, Copy)]
        struct TestLimiter;

        impl DrafterRateLimiter for TestLimiter {
            async fn acquire(
                &self,
                _key: DrafterRateLimitKey,
                _priority: DrafterPriority,
            ) -> DrafterPermit {
                DrafterPermit::new()
            }

            fn penalize(&self, _scope: DrafterRateLimitScope, _retry_after: std::time::Duration) {}
        }

        struct UnknownDeliveryBackend {
            side_effect_started: Arc<AtomicBool>,
        }

        impl DrafterBackend for UnknownDeliveryBackend {
            type Preview = String;
            type Final = String;
            type SegmentOutput = String;
            type Output = String;
            type Error = UnknownDeliveryError;

            fn capabilities(&self) -> DrafterCapabilities {
                DrafterCapabilities {
                    mode: DrafterMode::EditInPlace,
                    expires_without_refresh: false,
                    supports_draft_thinking: false,
                    supports_rich_preview: false,
                }
            }

            async fn update(&mut self, _preview: String) -> Result<PreviewAck, Self::Error> {
                Ok(PreviewAck)
            }

            async fn commit_segment(
                &mut self,
                final_payload: &String,
            ) -> Result<String, Self::Error> {
                Ok(final_payload.clone())
            }

            async fn finish(&mut self, _final_payload: &String) -> Result<String, Self::Error> {
                self.side_effect_started.store(true, Ordering::Relaxed);
                Err(UnknownDeliveryError)
            }

            async fn abort(&mut self) -> Result<(), Self::Error> {
                Ok(())
            }

            fn classify_error(
                &self,
                _operation: DrafterOperation,
                _error: &Self::Error,
            ) -> DrafterErrorDisposition {
                DrafterErrorDisposition {
                    class: DrafterErrorClass::Ambiguous,
                    delivery: DeliveryCertainty::Unknown,
                }
            }
        }

        let side_effect_started = Arc::new(AtomicBool::new(false));
        let (drafter, _sink) = Drafter::snapshots(
            UnknownDeliveryBackend {
                side_effect_started: Arc::clone(&side_effect_started),
            },
            TestLimiter,
            DraftConfig::default(),
        )
        .expect("fake drafter should start");
        let finish_error = drafter
            .finish("final".to_owned())
            .await
            .expect_err("fake backend must lose delivery confirmation");

        let metrics = Arc::new(crate::features::ask::metrics::AskDeliveryMetrics::default());
        let fallback_calls = Arc::new(AtomicU64::new(0));
        let audit = Arc::new(Mutex::new(None));
        let fallback_calls_ref = Arc::clone(&fallback_calls);
        let metrics_ref = Arc::clone(&metrics);
        let audit_ref = Arc::clone(&audit);

        apply_finish_error_policy(
            finish_error_certainty(&finish_error),
            AskRunStatus::Completed,
            move || {
                fallback_calls_ref.fetch_add(1, Ordering::Relaxed);
                async { Ok(()) }
            },
            move || metrics_ref.record_unknown_delivery_failure(),
            move |status, outcome, certainty| async move {
                *audit_ref.lock().unwrap() = Some((status, outcome, certainty));
            },
        )
        .await
        .expect("unknown delivery is handled without a second response");

        assert!(side_effect_started.load(Ordering::Relaxed));
        assert_eq!(fallback_calls.load(Ordering::Relaxed), 0);
        assert_eq!(metrics.snapshot().unknown_delivery_failures, 1);
        let (status, outcome, certainty) = audit
            .lock()
            .unwrap()
            .take()
            .expect("unknown delivery audit");
        assert_eq!(status.as_str(), "failed");
        assert_eq!(outcome, "rich_delivery_unknown");
        assert_eq!(certainty, Some(DeliveryCertainty::Unknown));
    }
}
