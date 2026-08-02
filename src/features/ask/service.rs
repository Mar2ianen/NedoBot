use chrono::{DateTime, Utc};
use jiff::Timestamp;
use sqlx::PgPool;
use std::collections::HashSet;
use teloxide::types::CustomEmojiId;
use teloxide::utils::{
    rich_text::{
        CustomEmojiBinding, LLM_DIALECT_VERSION, LlmMarkdownFormatter, RICH_TEXT_RENDERER_VERSION,
        RichTextBindings, RichTextPolicies, RichTextRenderContext, UnknownLinkAliasPolicy,
    },
    time::{TimeBindings, TimeContext},
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
    render_time: &'a TimeContext,
}

impl<'a> AskService<'a> {
    pub fn new(
        pool: &'a PgPool,
        config: &'a Config,
        llm_formatter: &'a LlmMarkdownFormatter,
        render_time: &'a TimeContext,
    ) -> Self {
        Self {
            pool,
            config,
            llm_formatter,
            render_time,
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
                    let bindings = match self.rich_text_bindings(
                        &answer.observed_message_ids,
                        &answer.observed_source_urls,
                    ) {
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
                    if let Err(err) = self.validate_literal_link_provenance(
                        &markdown,
                        &answer.observed_message_ids,
                        &answer.observed_source_urls,
                        &input.question,
                        input.reply_context.as_deref(),
                    ) {
                        tracing::warn!(%err, "ask answer contains an untrusted literal link");
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
                    let time_bindings = TimeBindings::default();
                    let mut policies = RichTextPolicies::llm();
                    policies.unknown_link_alias = UnknownLinkAliasPolicy::Error;
                    let context =
                        RichTextRenderContext::for_llm(self.render_time, &time_bindings, &bindings)
                            .with_policies(policies);
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
            renderer_revision: Some(RICH_TEXT_RENDERER_VERSION.to_owned()),
            rendered_markdown: rendered_markdown.map(str::to_owned),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            ..RenderAudit::default()
        }
    }

    fn rich_text_bindings(
        &self,
        observed_message_ids: &[i32],
        observed_source_urls: &[String],
    ) -> anyhow::Result<RichTextBindings> {
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
        for (index, url) in observed_source_urls.iter().enumerate() {
            bindings.insert_link(format!("source_{}", index + 1), Url::parse(url)?)?;
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

    fn validate_literal_link_provenance(
        &self,
        markdown: &str,
        observed_message_ids: &[i32],
        observed_source_urls: &[String],
        question: &str,
        reply_context: Option<&str>,
    ) -> anyhow::Result<()> {
        let parsed = LlmMarkdownFormatter::new()
            .parse(markdown)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let mut allowed = HashSet::new();
        add_allowed_url(&mut allowed, &self.config.chat_invite_url);
        for url in observed_source_urls {
            add_allowed_url(&mut allowed, url);
        }
        for message_id in observed_message_ids {
            if let Some(url) = crate::features::ask::chat_search::message_url(
                self.config.discussion_chat_id,
                *message_id,
            ) {
                add_allowed_url(&mut allowed, &url);
            }
        }
        for text in [Some(question), reply_context] {
            if let Some(text) = text {
                for url in extract_urls(text) {
                    allowed.insert(url);
                }
            }
        }
        validate_literal_destinations(parsed.link_destinations(), &allowed)
    }

    fn semantic_aliases(&self) -> String {
        let link_aliases = ["chat", "message_<id>"];
        let mut emoji_aliases = Vec::new();
        for (alias, value) in [
            ("comment", self.config.comment_custom_emoji_id.as_ref()),
            ("tech", self.config.tech_custom_emoji_id.as_ref()),
            ("amd", self.config.amd_custom_emoji_id.as_ref()),
            ("radeon", self.config.radeon_custom_emoji_id.as_ref()),
            ("ryzen", self.config.ryzen_custom_emoji_id.as_ref()),
        ] {
            if value.is_some_and(|value| !value.is_empty()) {
                emoji_aliases.push(format!(":{alias}:"));
            }
        }
        let emoji_aliases = if emoji_aliases.is_empty() {
            "нет".to_owned()
        } else {
            emoji_aliases.join(", ")
        };
        format!(
            "link aliases: {}; custom emoji aliases: {}",
            link_aliases.join(", "),
            emoji_aliases,
        )
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

fn add_allowed_url(allowed: &mut HashSet<String>, value: &str) {
    if let Some(value) = normalize_url(value) {
        allowed.insert(value);
    }
}

fn normalize_url(value: &str) -> Option<String> {
    Url::parse(value).ok().map(|url| url.to_string())
}

fn extract_urls(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split_whitespace().filter_map(|candidate| {
        let candidate = candidate.trim_matches(|character: char| {
            matches!(
                character,
                '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | ';' | '.'
            )
        });
        normalize_url(candidate)
    })
}

fn validate_literal_destinations(
    destinations: impl IntoIterator<Item = String>,
    allowed: &HashSet<String>,
) -> anyhow::Result<()> {
    for destination in destinations {
        let Some(normalized) = normalize_url(&destination) else {
            anyhow::bail!("literal link is not a valid trusted URL: {destination}");
        };
        if !allowed.contains(&normalized) {
            anyhow::bail!(
                "literal link was not present in trusted input or tool evidence: {destination}"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_links_must_come_from_trusted_inputs() {
        let allowed = ["https://example.com/docs".to_owned()]
            .into_iter()
            .collect::<HashSet<_>>();
        assert!(
            validate_literal_destinations(vec!["https://example.com/docs".to_owned()], &allowed,)
                .is_ok()
        );
        assert!(
            validate_literal_destinations(
                vec!["https://invented.example/foo".to_owned()],
                &allowed,
            )
            .is_err()
        );
    }

    #[test]
    fn urls_from_question_are_normalized_and_punctuation_is_ignored() {
        assert_eq!(
            extract_urls("см. (https://example.com/docs), затем tg://resolve?domain=test")
                .collect::<Vec<_>>(),
            vec![
                "https://example.com/docs".to_owned(),
                "tg://resolve?domain=test".to_owned(),
            ]
        );
    }
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
