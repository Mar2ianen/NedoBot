#[cfg(feature = "moderation")]
use teloxide::types::CallbackQuery;

#[cfg(feature = "moderation")]
use crate::{
    features::{
        self,
        reports::{self, ReportActionResult},
        spam_review::{apply_callback, callback_message_chat_id, is_chat_admin, parse_callback},
    },
    state::AppState,
};
#[cfg(feature = "moderation")]
use teloxide::prelude::*;

#[cfg(feature = "moderation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpamReviewDecision {
    Spam,
    NotSpam,
    Unsupported,
}

#[cfg(feature = "moderation")]
impl SpamReviewDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::NotSpam => "normal",
            Self::Unsupported => "",
        }
    }
}

#[cfg(feature = "moderation")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallbackKind {
    RiskCaptcha,
    Report(i64, reports::ReportAction),
    SpamReview(i64, SpamReviewDecision),
    Unsupported,
}

#[cfg(feature = "moderation")]
fn classify_callback(data: Option<&str>) -> CallbackKind {
    let Some(data) = data else {
        return CallbackKind::Unsupported;
    };
    if features::risk_captcha::is_callback_data(data) {
        return CallbackKind::RiskCaptcha;
    }
    if let Some((report_id, action)) = reports::parse_callback(data) {
        return CallbackKind::Report(report_id, action);
    }
    if let Some((request_id, decision)) = parse_callback(data) {
        let decision = match decision {
            "spam" => SpamReviewDecision::Spam,
            "normal" => SpamReviewDecision::NotSpam,
            _ => SpamReviewDecision::Unsupported,
        };
        return CallbackKind::SpamReview(request_id, decision);
    }
    CallbackKind::Unsupported
}

#[cfg(feature = "moderation")]
pub async fn handle_callback_query(
    bot: teloxide::adaptors::DefaultParseMode<Bot>,
    query: CallbackQuery,
    state: AppState,
) -> ResponseResult<()> {
    match classify_callback(query.data.as_deref()) {
        CallbackKind::RiskCaptcha => handle_risk_captcha_callback(&bot, &query, &state).await,
        CallbackKind::Report(report_id, action) => {
            handle_report_callback(&bot, &query, &state, report_id, action).await
        }
        CallbackKind::SpamReview(request_id, decision) => {
            handle_spam_review_callback(&bot, &query, &state, request_id, decision.as_str()).await
        }
        CallbackKind::Unsupported => answer_unsupported_callback(&bot, &query).await,
    }
}

#[cfg(feature = "moderation")]
async fn answer_unsupported_callback(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    query: &CallbackQuery,
) -> ResponseResult<()> {
    bot.answer_callback_query(query.id.clone()).await?;
    Ok(())
}

#[cfg(feature = "moderation")]
async fn handle_spam_review_callback(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    query: &CallbackQuery,
    state: &AppState,
    request_id: i64,
    decision: &str,
) -> ResponseResult<()> {
    let reviewer_id = query.from.id.0 as i64;
    let is_configured_reviewer = state.config.reviewer_user_ids().contains(&reviewer_id)
        || state.config.owner_telegram_id == Some(reviewer_id);
    let is_review_chat_admin = if is_configured_reviewer {
        false
    } else if let Some(review_chat_id) = callback_message_chat_id(query) {
        match is_chat_admin(bot.inner(), review_chat_id, reviewer_id).await {
            Ok(is_admin) => is_admin,
            Err(error) => {
                tracing::warn!(%error, review_chat_id, reviewer_id, "failed to verify spam review callback actor");
                bot.answer_callback_query(query.id.clone())
                    .text("Не удалось проверить права администратора.")
                    .await?;
                return Ok(());
            }
        }
    } else {
        false
    };
    if !is_configured_reviewer && !is_review_chat_admin {
        bot.answer_callback_query(query.id.clone())
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
            bot.answer_callback_query(query.id.clone())
                .text("Решение уже принято или кнопка устарела.")
                .await?;
        }
        Err(err) => {
            tracing::error!(%err, request_id, "failed to apply spam review callback");
            bot.answer_callback_query(query.id.clone())
                .text("Не удалось сохранить решение.")
                .await?;
        }
    }
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

#[cfg(all(test, feature = "moderation"))]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{CallbackKind, answer_unsupported_callback, classify_callback};
    use crate::features::reports::ReportAction;
    use axum::{Router, body::Bytes, extract::State, routing::post};
    use teloxide::{
        prelude::{Bot, RequesterExt},
        types::ParseMode,
    };

    type CapturedRequest = Arc<Mutex<Option<(String, String)>>>;

    async fn fake_telegram(
        State(captured): State<CapturedRequest>,
        uri: axum::http::Uri,
        body: Bytes,
    ) -> axum::Json<serde_json::Value> {
        *captured.lock().expect("request capture lock") = Some((
            uri.path().to_owned(),
            String::from_utf8_lossy(&body).into_owned(),
        ));
        axum::Json(serde_json::json!({"ok": true, "result": true}))
    }

    #[test]
    fn callback_classifier_preserves_captcha_report_spam_review_precedence() {
        assert_eq!(
            classify_callback(Some("rc:not-a-uuid:1")),
            CallbackKind::RiskCaptcha
        );
        assert_eq!(
            classify_callback(Some("report:42:accept")),
            CallbackKind::Report(42, ReportAction::Accept)
        );
        assert_eq!(
            classify_callback(Some("spam_review:77:spam")),
            CallbackKind::SpamReview(77, super::SpamReviewDecision::Spam)
        );
    }

    #[test]
    fn malformed_or_unsupported_callback_data_is_a_noop_classification() {
        assert_eq!(classify_callback(None), CallbackKind::Unsupported);
        assert_eq!(
            classify_callback(Some("report:0:accept")),
            CallbackKind::Unsupported
        );
        assert_eq!(
            classify_callback(Some("report:42:accept:extra")),
            CallbackKind::Unsupported
        );
        assert_eq!(
            classify_callback(Some("unrecognized:payload")),
            CallbackKind::Unsupported
        );
    }

    #[tokio::test]
    async fn unsupported_callback_is_acknowledged_without_side_effects() {
        let captured = Arc::new(Mutex::new(None));
        let app = Router::new()
            .route("/{*path}", post(fake_telegram))
            .with_state(Arc::clone(&captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("mock Telegram listener should bind");
        let address = listener.local_addr().expect("listener address is known");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let bot = Bot::new("123456:TEST_TOKEN")
            .set_api_url(format!("http://{address}/").parse().expect("mock URL"))
            .parse_mode(ParseMode::Html);
        let query = serde_json::from_value(serde_json::json!({
            "id": "query-1",
            "from": {"id": 42, "is_bot": false, "first_name": "Test"},
            "chat_instance": "instance",
            "data": "unrecognized:payload"
        }))
        .expect("synthetic callback query should deserialize");

        answer_unsupported_callback(&bot, &query)
            .await
            .expect("unsupported callback should be answered");
        server.abort();

        let (path, body) = captured
            .lock()
            .expect("request capture lock")
            .take()
            .expect("Telegram callback acknowledgement should be sent");
        assert!(
            path.ends_with("/AnswerCallbackQuery"),
            "unexpected Telegram method path: {path}"
        );
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["callback_query_id"], "query-1");
        assert!(payload.get("text").is_none());
    }
}
