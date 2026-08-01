use std::time::Duration;

use anyhow::{Context, Result, bail};
use teloxide::{
    adaptors::DefaultParseMode,
    drafter::{DraftAccumulator, DraftConfig, Drafter, DrafterMetricsCollector, NativeRichBackend},
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

/// Runs the live native rich-draft smoke test in a private chat.
///
/// The command is intentionally available only from debug builds and the
/// configured private allowlist. It leaves successful final messages in the
/// chat so Telegram rendering can be inspected manually.
pub async fn run(bot: &DefaultParseMode<Bot>, msg: &Message, state: &AppState) -> Result<String> {
    let Some(user) = msg.from.as_ref() else {
        bail!("drafter smoke requires a user sender");
    };
    if !cfg!(debug_assertions) {
        bail!("drafter smoke is available only in debug builds");
    }
    if !msg.chat.is_private() {
        bail!("drafter smoke must be run in a private chat");
    }
    if !state
        .config
        .ask_private_user_ids
        .contains(&(user.id.0 as i64))
    {
        bail!("drafter smoke is not enabled for this user");
    }

    let config = smoke_config();
    let snapshot_metrics = run_snapshot_lifecycle(bot, msg, state, &config).await?;
    let accumulator_metrics = run_accumulator_lifecycle(bot, msg, state, &config).await?;
    run_abort_lifecycle(bot, msg, state, &config).await?;

    Ok(format_metrics(snapshot_metrics, accumulator_metrics))
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
                value: None,
                type_field: None,
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
        "✅ Drafter smoke passed\n\nSnapshot: previews={}, refreshes={}, segments={}, updates={}\nAccumulator: previews={}, segments={}, updates={}\nAbort: ok\n\nПроверены native rich draft, thinking block, rich blocks, latest-wins, flush, watchdog refresh, segment rotation, accumulator reset, finish и abort.",
        snapshot.sent_previews,
        snapshot.refresh_requests,
        snapshot.segment_count,
        snapshot.received_updates,
        accumulator.sent_previews,
        accumulator.segment_count,
        accumulator.received_updates,
    )
}
