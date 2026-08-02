use chrono::{DateTime, Utc};
use jiff::Timestamp;
use sqlx::PgPool;
use teloxide::types::CustomEmojiId;
use teloxide::utils::time::{
    CustomEmojiBinding, LLM_DIALECT_VERSION, LlmMarkdownFormatter, RichTextBindings,
    RichTextPolicies, RichTextRenderContext, TIME_RENDERER_VERSION,
};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use crate::config::Config;
use crate::features::ask::agent::{self, AskRequest};
use crate::features::ask::repo::{self, CreateAskRunParams, RenderAudit};
use crate::features::ask::rich_markdown;
use crate::features::ask::types::{
    AskAnswer, AskCommandInput, AskFailureKind, AskProgress, AskRunStatus,
};
use crate::llm::profiles::RouteRequirements;
use crate::telegram::render::validate_rich_markdown;

pub struct AskService<'a> {
    pool: &'a PgPool,
    config: &'a Config,
    llm_formatter: &'a LlmMarkdownFormatter,
}

impl<'a> AskService<'a> {
    pub fn new(
        pool: &'a PgPool,
        config: &'a Config,
        llm_formatter: &'a LlmMarkdownFormatter,
    ) -> Self {
        Self {
            pool,
            config,
            llm_formatter,
        }
    }

    pub async fn execute(
        &self,
        input: AskCommandInput,
        progress: Option<&UnboundedSender<AskProgress>>,
    ) -> Result<AskAnswer, AskServiceError> {
        let ask_run_id = self.start_run(&input).await;
        let semantic_aliases = self.semantic_aliases();
        let answer = agent::answer(
            self.config,
            self.pool,
            AskRequest {
                ask_run_id,
                requester_user_id: input.requester_user_id,
                requester_identity: &input.requester_identity,
                question: &input.question,
                reply_context: input.reply_context.as_deref(),
                image_base64: input.reply_image_base64.as_deref(),
                progress,
                allow_mutations: input.allow_mutations,
                semantic_aliases: &semantic_aliases,
            },
        )
        .await;

        match answer {
            Ok(answer) => match rich_markdown::validate(&answer.markdown) {
                Ok(markdown) => {
                    let captured_now = Timestamp::now();
                    let bindings = match self.rich_text_bindings(&answer.observed_message_ids) {
                        Ok(bindings) => bindings,
                        Err(err) => {
                            tracing::warn!(%err, "failed to build ask rich-text bindings");
                            let kind = AskFailureKind::InvalidOutput;
                            self.finish_run(
                                ask_run_id,
                                AskRunStatus::Failed,
                                Some(&markdown),
                                self.render_audit(captured_now, None),
                                Some(kind),
                            )
                            .await;
                            return Err(AskServiceError::new(kind, err, ask_run_id));
                        }
                    };
                    let context = RichTextRenderContext::new(self.llm_formatter.time(), &bindings)
                        .with_policies(RichTextPolicies::llm());
                    match self.llm_formatter.render_with_context_at(
                        &markdown,
                        &context,
                        captured_now,
                    ) {
                        Ok(rendered) => {
                            if !rendered.diagnostics.is_empty() {
                                tracing::debug!(
                                    diagnostics = rendered.diagnostics.len(),
                                    "ask rich-text render completed with diagnostics"
                                );
                            }
                            let render = self.render_audit(captured_now, Some(&rendered.markdown));
                            if let Err(err) = validate_rich_markdown(&rendered.markdown) {
                                tracing::warn!(%err, "compiled ask rich markdown is not deliverable");
                                let kind = AskFailureKind::InvalidOutput;
                                self.finish_run(
                                    ask_run_id,
                                    AskRunStatus::Failed,
                                    Some(&markdown),
                                    render,
                                    Some(kind),
                                )
                                .await;
                                Err(AskServiceError::new(
                                    kind,
                                    anyhow::Error::new(err),
                                    ask_run_id,
                                ))
                            } else {
                                self.finish_run(
                                    ask_run_id,
                                    AskRunStatus::DeliveryPending,
                                    Some(&markdown),
                                    render,
                                    None,
                                )
                                .await;
                                Ok(AskAnswer {
                                    markdown,
                                    rendered,
                                    ask_run_id,
                                })
                            }
                        }
                        Err(err) => {
                            tracing::warn!(%err, "ask assistant returned invalid time markup");
                            let kind = AskFailureKind::InvalidOutput;
                            self.finish_run(
                                ask_run_id,
                                AskRunStatus::Failed,
                                Some(&markdown),
                                self.render_audit(captured_now, None),
                                Some(kind),
                            )
                            .await;
                            Err(AskServiceError::new(
                                kind,
                                anyhow::Error::new(err),
                                ask_run_id,
                            ))
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(%err, "ask assistant returned unsafe markdown");
                    let kind = AskFailureKind::InvalidOutput;
                    self.finish_run(
                        ask_run_id,
                        AskRunStatus::Failed,
                        None,
                        RenderAudit::default(),
                        Some(kind),
                    )
                    .await;
                    Err(AskServiceError::new(kind, err, ask_run_id))
                }
            },
            Err(err) => {
                let kind = AskFailureKind::from_error(&err);
                self.finish_run(
                    ask_run_id,
                    AskRunStatus::Failed,
                    None,
                    RenderAudit::default(),
                    Some(kind),
                )
                .await;
                Err(AskServiceError::new(kind, err, ask_run_id))
            }
        }
    }

    async fn start_run(&self, input: &AskCommandInput) -> Option<i64> {
        let (provider, model) = self
            .config
            .llm_profiles
            .as_ref()
            .and_then(|profiles| {
                profiles
                    .resolve_route(
                        "ask",
                        &RouteRequirements {
                            requires_images: input.reply_image_base64.is_some(),
                            requires_tools: true,
                            requires_system_prompt: true,
                            num_predict: Some(self.config.ask_llm_max_tokens),
                            ..RouteRequirements::default()
                        },
                    )
                    .ok()
                    .and_then(|route| {
                        route.selections.first().map(|selection| {
                            (
                                selection.provider_key.to_string(),
                                Some(selection.model.model.clone()),
                            )
                        })
                    })
            })
            .unwrap_or_else(|| ("profile_route".to_string(), None));
        match repo::create_run(
            self.pool,
            CreateAskRunParams {
                chat_id: input.chat_id,
                command_message_id: input.command_message_id,
                requester_user_id: input.requester_user_id,
                question: &input.question,
                reply_to_message_id: input.reply_to_message_id,
                provider: &provider,
                model: model.as_deref(),
            },
        )
        .await
        {
            Ok(ask_run_id) => Some(ask_run_id),
            Err(err) => {
                tracing::warn!(%err, "failed to start ask audit run");
                None
            }
        }
    }

    async fn finish_run(
        &self,
        ask_run_id: Option<i64>,
        status: AskRunStatus,
        answer_markdown: Option<&str>,
        render: RenderAudit,
        failure_kind: Option<AskFailureKind>,
    ) {
        let Some(ask_run_id) = ask_run_id else {
            return;
        };
        if let Err(err) = repo::finish_run(
            self.pool,
            ask_run_id,
            status,
            answer_markdown,
            render,
            failure_kind.map(AskFailureKind::as_str),
        )
        .await
        {
            tracing::warn!(%err, ask_run_id, "failed to finish ask audit run");
        }
    }

    fn render_audit(
        &self,
        captured_now: Timestamp,
        rendered_markdown: Option<&str>,
    ) -> RenderAudit {
        RenderAudit {
            captured_now: timestamp_to_chrono(captured_now),
            dialect: Some(LLM_DIALECT_VERSION.to_owned()),
            timezone: Some(self.config.render_timezone.clone()),
            renderer_revision: Some(TIME_RENDERER_VERSION.to_owned()),
            rendered_markdown: rendered_markdown.map(str::to_owned),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            ..RenderAudit::default()
        }
    }

    fn rich_text_bindings(&self, observed_message_ids: &[i32]) -> anyhow::Result<RichTextBindings> {
        let mut bindings = RichTextBindings::new();
        bindings.insert_link("chat", Url::parse(&self.config.chat_invite_url)?)?;
        for message_id in observed_message_ids {
            let Some(url) = crate::features::ask::chat_search::message_url(
                self.config.discussion_chat_id,
                *message_id,
            ) else {
                continue;
            };
            bindings.insert_link(format!("message_{message_id}"), Url::parse(&url)?)?;
        }
        self.insert_custom_emoji(
            &mut bindings,
            "comment",
            self.config.comment_custom_emoji_id.as_deref(),
            "😎",
        )?;
        self.insert_custom_emoji(
            &mut bindings,
            "tech",
            self.config.tech_custom_emoji_id.as_deref(),
            "🛠️",
        )?;
        self.insert_custom_emoji(
            &mut bindings,
            "amd",
            self.config.amd_custom_emoji_id.as_deref(),
            "🔴",
        )?;
        self.insert_custom_emoji(
            &mut bindings,
            "radeon",
            self.config.radeon_custom_emoji_id.as_deref(),
            "🎮",
        )?;
        self.insert_custom_emoji(
            &mut bindings,
            "ryzen",
            self.config.ryzen_custom_emoji_id.as_deref(),
            "⚙️",
        )?;
        Ok(bindings)
    }

    fn semantic_aliases(&self) -> String {
        let mut aliases = vec!["chat", "message_<id>"];
        for (alias, value) in [
            ("comment", self.config.comment_custom_emoji_id.as_ref()),
            ("tech", self.config.tech_custom_emoji_id.as_ref()),
            ("amd", self.config.amd_custom_emoji_id.as_ref()),
            ("radeon", self.config.radeon_custom_emoji_id.as_ref()),
            ("ryzen", self.config.ryzen_custom_emoji_id.as_ref()),
        ] {
            if value.is_some_and(|value| !value.is_empty()) {
                aliases.push(alias);
            }
        }
        aliases.join(", ")
    }

    fn insert_custom_emoji(
        &self,
        bindings: &mut RichTextBindings,
        alias: &str,
        custom_emoji_id: Option<&str>,
        fallback: &str,
    ) -> anyhow::Result<()> {
        let Some(custom_emoji_id) = custom_emoji_id.filter(|value| !value.is_empty()) else {
            return Ok(());
        };
        bindings.insert_custom_emoji(
            alias,
            CustomEmojiBinding {
                custom_emoji_id: CustomEmojiId(custom_emoji_id.to_owned()),
                fallback: fallback.to_owned(),
            },
        )?;
        Ok(())
    }
}

fn timestamp_to_chrono(timestamp: Timestamp) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(timestamp.as_second(), timestamp.subsec_nanosecond() as u32)
}

#[derive(Debug)]
pub struct AskServiceError {
    pub kind: AskFailureKind,
    pub ask_run_id: Option<i64>,
    source: anyhow::Error,
}

impl AskServiceError {
    fn new(kind: AskFailureKind, source: anyhow::Error, ask_run_id: Option<i64>) -> Self {
        Self {
            kind,
            ask_run_id,
            source,
        }
    }
}

impl std::fmt::Display for AskServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)
    }
}

impl std::error::Error for AskServiceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
