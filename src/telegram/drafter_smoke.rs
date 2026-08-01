use std::time::Duration;

use anyhow::{Context, Result, bail};
use teloxide::{
    adaptors::DefaultParseMode,
    drafter::{
        DraftAccumulator, DraftConfig, Drafter, DrafterMetricsCollector, NativeRichBackend,
        RichEditInPlaceBackend,
    },
    prelude::{Bot, Message, UserId},
    types::{
        InputRichBlock, InputRichBlockDetails, InputRichBlockDivider, InputRichBlockList,
        InputRichBlockListItem, InputRichBlockParagraph, InputRichBlockPreformatted,
        InputRichBlockSectionHeading, InputRichBlockThinking, InputRichMessage, ReplyParameters,
        RichText,
    },
};

use crate::state::AppState;

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Default)]
struct SmokeAccumulator {
    text: String,
}

impl DraftAccumulator for SmokeAccumulator {
    type Update = String;
    type Preview = InputRichMessage;

    fn apply(&mut self, update: Self::Update) {
        self.text.push_str(&update);
    }

    fn snapshot(&self) -> Option<Self::Preview> {
        if self.text.is_empty() {
            return None;
        }
        Some(accumulator_preview(&self.text))
    }

    fn reset_segment(&mut self) {
        self.text.clear();
    }
}

/// Runs the live Drafter smoke test in a private or configured discussion chat.
///
/// The command is intentionally available only from debug builds and the
/// configured private allowlist. Private chats use Telegram-native rich
/// drafts; the discussion chat edits one rich message in place. The private
/// path exercises the full lifecycle, while the chat path stays deliberately
/// compact so the smoke command does not flood the conversation.
pub async fn run(bot: &DefaultParseMode<Bot>, msg: &Message, state: &AppState) -> Result<String> {
    let Some(user) = msg.from.as_ref() else {
        bail!("drafter smoke requires a user sender");
    };
    if !cfg!(debug_assertions) {
        bail!("drafter smoke is available only in debug builds");
    }
    if !state
        .config
        .ask_private_user_ids
        .contains(&(user.id.0 as i64))
    {
        bail!("drafter smoke is not enabled for this user");
    }
    let is_private = msg.chat.is_private();
    let is_discussion_chat = msg.chat.id.0 == state.config.discussion_chat_id;
    if !is_private && !is_discussion_chat {
        bail!("drafter smoke must be run in a private chat or the configured discussion chat");
    }

    let config = smoke_config();
    if is_private {
        let snapshot_metrics = run_snapshot_lifecycle(bot, msg, state, &config).await?;
        let accumulator_metrics = run_accumulator_lifecycle(bot, msg, state, &config).await?;
        run_abort_lifecycle(bot, msg, state, &config).await?;
        Ok(format_metrics(snapshot_metrics, accumulator_metrics))
    } else {
        let metrics = run_edit_in_place_lifecycle(bot, msg, state, &config).await?;
        Ok(format_edit_metrics(metrics))
    }
}

async fn run_snapshot_lifecycle(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    config: &DraftConfig,
) -> Result<DrafterMetricsCollector> {
    let metrics = DrafterMetricsCollector::default();
    let backend = native_backend(bot, msg, msg.from.as_ref().map(|user| user.id))?;
    let (mut drafter, sink) = Drafter::snapshots_with_observer(
        backend,
        state.drafter_limiter.clone(),
        config.clone(),
        std::sync::Arc::new(metrics.clone()),
    )
    .context("failed to create snapshot smoke drafter")?;

    sink.update(rich_preview("thinking: preparing", "snapshot revision 1"))
        .context("failed to queue first snapshot")?;
    sink.update(rich_preview("thinking: latest wins", "snapshot revision 2"))
        .context("failed to queue second snapshot")?;
    drafter.flush().await.context("snapshot flush failed")?;

    tokio::time::sleep(REFRESH_INTERVAL + Duration::from_millis(300)).await;

    sink.update(rich_preview(
        "thinking: after refresh",
        "snapshot revision 3",
    ))
    .context("failed to queue refreshed snapshot")?;
    drafter
        .flush()
        .await
        .context("refreshed snapshot flush failed")?;

    drafter
        .commit_segment(rich_final("segment 1 committed", "native draft rotated"))
        .await
        .context("snapshot segment commit failed")?;

    sink.update(rich_preview(
        "thinking: segment 2",
        "this preview belongs to the new draft segment",
    ))
    .context("failed to queue rotated snapshot")?;
    drafter
        .flush()
        .await
        .context("rotated snapshot flush failed")?;

    let preview_metrics = metrics.snapshot();
    drafter
        .finish(rich_final(
            "snapshot lifecycle complete",
            &format!(
                "previews={} refreshes={} segments={}",
                preview_metrics.sent_previews,
                preview_metrics.refresh_requests,
                preview_metrics.segment_count
            ),
        ))
        .await
        .context("snapshot finish failed")?;

    Ok(metrics)
}

async fn run_accumulator_lifecycle(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    config: &DraftConfig,
) -> Result<DrafterMetricsCollector> {
    let metrics = DrafterMetricsCollector::default();
    let backend = native_backend(bot, msg, msg.from.as_ref().map(|user| user.id))?;
    let (mut drafter, sink) = Drafter::accumulating_with_observer(
        SmokeAccumulator::default(),
        backend,
        state.drafter_limiter.clone(),
        config.clone(),
        std::sync::Arc::new(metrics.clone()),
    )
    .context("failed to create accumulator smoke drafter")?;

    sink.push("accumulator segment 1: ".to_owned())
        .context("failed to push first accumulator chunk")?;
    sink.push("old text must not leak".to_owned())
        .context("failed to push second accumulator chunk")?;
    drafter.flush().await.context("accumulator flush failed")?;

    drafter
        .commit_segment(rich_final(
            "accumulator segment 1 committed",
            "the next preview must start from an empty accumulator",
        ))
        .await
        .context("accumulator segment commit failed")?;

    sink.push("accumulator segment 2 only".to_owned())
        .context("failed to push new accumulator segment")?;
    drafter
        .flush()
        .await
        .context("new accumulator segment flush failed")?;

    let preview_metrics = metrics.snapshot();
    drafter
        .finish(rich_final(
            "accumulator lifecycle complete",
            &format!(
                "previews={} segments={}",
                preview_metrics.sent_previews, preview_metrics.segment_count
            ),
        ))
        .await
        .context("accumulator finish failed")?;

    Ok(metrics)
}

async fn run_abort_lifecycle(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    config: &DraftConfig,
) -> Result<()> {
    let backend = native_backend(bot, msg, msg.from.as_ref().map(|user| user.id))?;
    let (drafter, sink) =
        Drafter::snapshots(backend, state.drafter_limiter.clone(), config.clone())
            .context("failed to create abort smoke drafter")?;
    sink.update(rich_preview(
        "thinking: abort path",
        "this draft should be aborted",
    ))
    .context("failed to queue abort preview")?;
    drafter
        .flush()
        .await
        .context("abort preview flush failed")?;
    drafter.abort().await.context("abort lifecycle failed")?;
    Ok(())
}

async fn run_edit_in_place_lifecycle(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    state: &AppState,
    config: &DraftConfig,
) -> Result<DrafterMetricsCollector> {
    let metrics = DrafterMetricsCollector::default();
    let backend = edit_backend(bot, msg);
    let (drafter, sink) = Drafter::snapshots_with_observer(
        backend,
        state.drafter_limiter.clone(),
        config.clone(),
        std::sync::Arc::new(metrics.clone()),
    )
    .context("failed to create rich edit smoke drafter")?;

    sink.update(edit_preview("rich preview revision 1"))
        .context("failed to queue first rich preview")?;
    sink.update(edit_preview("rich preview revision 2 (latest wins)"))
        .context("failed to queue second rich preview")?;
    drafter.flush().await.context("rich preview flush failed")?;

    tokio::time::sleep(config.min_update_interval + Duration::from_millis(100)).await;
    sink.update(edit_preview("rich preview edited in place"))
        .context("failed to queue edited rich preview")?;
    drafter
        .flush()
        .await
        .context("edited rich preview flush failed")?;

    let preview_metrics = metrics.snapshot();
    drafter
        .finish(rich_final(
            "one rich message complete",
            &format!(
                "the same message was sent and edited in place; previews={} refreshes={}",
                preview_metrics.sent_previews, preview_metrics.refresh_requests
            ),
        ))
        .await
        .context("rich edit finish failed")?;

    Ok(metrics)
}

fn edit_backend(bot: &DefaultParseMode<Bot>, msg: &Message) -> RichEditInPlaceBackend {
    RichEditInPlaceBackend::new(bot.inner().clone(), msg.chat.id)
        .reply_parameters(ReplyParameters::new(msg.id).allow_sending_without_reply())
}

fn native_backend(
    bot: &DefaultParseMode<Bot>,
    msg: &Message,
    user_id: Option<UserId>,
) -> Result<NativeRichBackend<DefaultParseMode<Bot>>> {
    let user_id = user_id.context("drafter smoke requires a private user id")?;
    Ok(NativeRichBackend::new(bot.clone(), user_id)
        .reply_parameters(ReplyParameters::new(msg.id).allow_sending_without_reply()))
}

fn smoke_config() -> DraftConfig {
    DraftConfig {
        coalesce_window: Duration::from_millis(100),
        min_update_interval: Duration::from_millis(200),
        refresh_interval: REFRESH_INTERVAL,
        request_timeout: Duration::from_secs(1),
        retry_initial: Duration::from_millis(200),
        retry_max: Duration::from_secs(1),
        max_consecutive_preview_failures: Some(2),
    }
}

fn rich_preview(thinking: &str, text: &str) -> InputRichMessage {
    InputRichMessage::blocks([
        InputRichBlock::Heading(InputRichBlockSectionHeading {
            text: RichText::from("Drafter smoke preview"),
            size: 2,
        }),
        InputRichBlock::Thinking(InputRichBlockThinking {
            text: thinking.into(),
        }),
        InputRichBlock::Paragraph(InputRichBlockParagraph { text: text.into() }),
        InputRichBlock::Pre(InputRichBlockPreformatted {
            text: "latest-wins / flush / watchdog".into(),
            language: Some("text".to_owned()),
        }),
        InputRichBlock::List(InputRichBlockList {
            items: vec![InputRichBlockListItem {
                blocks: vec![InputRichBlock::Paragraph(InputRichBlockParagraph {
                    text: "rich blocks are active".into(),
                })],
                has_checkbox: Some(true),
                is_checked: Some(true),
                value: Some(1),
                type_field: Some("1".to_owned()),
            }],
        }),
        InputRichBlock::Divider(InputRichBlockDivider {}),
        InputRichBlock::Details(InputRichBlockDetails {
            summary: "nested rich block".into(),
            blocks: vec![InputRichBlock::Paragraph(InputRichBlockParagraph {
                text: "thinking is draft-only; this block is safe to finish".into(),
            })],
            is_open: Some(true),
        }),
    ])
}

fn edit_preview(text: &str) -> InputRichMessage {
    InputRichMessage::blocks([
        InputRichBlock::Heading(InputRichBlockSectionHeading {
            text: RichText::from("Drafter smoke rich preview"),
            size: 2,
        }),
        InputRichBlock::Paragraph(InputRichBlockParagraph { text: text.into() }),
        InputRichBlock::Pre(InputRichBlockPreformatted {
            text: "send once / edit in place / rich final".into(),
            language: Some("text".to_owned()),
        }),
        InputRichBlock::List(InputRichBlockList {
            items: vec![InputRichBlockListItem {
                blocks: vec![InputRichBlock::Paragraph(InputRichBlockParagraph {
                    text: "rich blocks are active".into(),
                })],
                has_checkbox: Some(true),
                is_checked: Some(true),
                value: Some(1),
                type_field: Some("1".to_owned()),
            }],
        }),
        InputRichBlock::Divider(InputRichBlockDivider {}),
        InputRichBlock::Details(InputRichBlockDetails {
            summary: "nested rich block".into(),
            blocks: vec![InputRichBlock::Paragraph(InputRichBlockParagraph {
                text: "this preview is valid for send and edit".into(),
            })],
            is_open: Some(true),
        }),
    ])
}

fn rich_final(title: &str, body: &str) -> InputRichMessage {
    InputRichMessage::blocks([
        InputRichBlock::Heading(InputRichBlockSectionHeading {
            text: title.into(),
            size: 2,
        }),
        InputRichBlock::Paragraph(InputRichBlockParagraph { text: body.into() }),
        InputRichBlock::Pre(InputRichBlockPreformatted {
            text: "thinking block intentionally removed from final".into(),
            language: Some("text".to_owned()),
        }),
    ])
}

fn accumulator_preview(text: &str) -> InputRichMessage {
    InputRichMessage::blocks([
        InputRichBlock::Heading(InputRichBlockSectionHeading {
            text: "Accumulator preview".into(),
            size: 2,
        }),
        InputRichBlock::Paragraph(InputRichBlockParagraph { text: text.into() }),
    ])
}

fn format_metrics(
    snapshot: DrafterMetricsCollector,
    accumulator: DrafterMetricsCollector,
) -> String {
    let snapshot = snapshot.snapshot();
    let accumulator = accumulator.snapshot();
    format!(
        "✅ Drafter smoke passed (native rich draft в личке)\n\nSnapshot: previews={}, refreshes={}, segments={}, updates={}\nAccumulator: previews={}, segments={}, updates={}\nAbort: ok\n\nПроверены native rich draft, thinking block, rich blocks, latest-wins, flush, watchdog refresh, segment rotation, accumulator reset, finish и abort.",
        snapshot.sent_previews,
        snapshot.refresh_requests,
        snapshot.segment_count,
        snapshot.received_updates,
        accumulator.sent_previews,
        accumulator.segment_count,
        accumulator.received_updates,
    )
}

fn format_edit_metrics(metrics: DrafterMetricsCollector) -> String {
    let metrics = metrics.snapshot();
    format!(
        "✅ Drafter smoke passed (one rich message в обычном чате)\n\nRich edit: previews={}, refreshes={}, updates={}\nFinal: то же сообщение превращено в permanent rich message.\n\nПроверены send rich once, latest-wins, flush, edit-in-place, rich blocks и final edit.",
        metrics.sent_previews, metrics.refresh_requests, metrics.received_updates,
    )
}
