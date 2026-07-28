use sqlx::PgPool;
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Config;
use crate::features::ask::agent::{self, AskRequest};
use crate::features::ask::repo::{self, CreateAskRunParams};
use crate::features::ask::rich_markdown;
use crate::features::ask::types::{
    AskAnswer, AskCommandInput, AskFailureKind, AskProgress, AskRunStatus,
};

pub struct AskService<'a> {
    pool: &'a PgPool,
    config: &'a Config,
}

impl<'a> AskService<'a> {
    pub fn new(pool: &'a PgPool, config: &'a Config) -> Self {
        Self { pool, config }
    }

    pub async fn execute(
        &self,
        input: AskCommandInput,
        progress: Option<&UnboundedSender<AskProgress>>,
    ) -> Result<AskAnswer, AskServiceError> {
        let ask_run_id = self.start_run(&input).await;
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
            },
        )
        .await;

        match answer {
            Ok(answer) => match rich_markdown::validate(&answer) {
                Ok(markdown) => {
                    self.finish_run(ask_run_id, AskRunStatus::Completed, Some(&markdown), None)
                        .await;
                    Ok(AskAnswer {
                        markdown,
                        ask_run_id,
                    })
                }
                Err(err) => {
                    tracing::warn!(%err, "ask assistant returned unsafe markdown");
                    let kind = AskFailureKind::InvalidOutput;
                    self.finish_run(ask_run_id, AskRunStatus::Failed, None, Some(kind))
                        .await;
                    Err(AskServiceError::new(kind, err))
                }
            },
            Err(err) => {
                let kind = AskFailureKind::from_error(&err);
                self.finish_run(ask_run_id, AskRunStatus::Failed, None, Some(kind))
                    .await;
                Err(AskServiceError::new(kind, err))
            }
        }
    }

    async fn start_run(&self, input: &AskCommandInput) -> Option<i64> {
        match repo::create_run(
            self.pool,
            CreateAskRunParams {
                chat_id: input.chat_id,
                command_message_id: input.command_message_id,
                requester_user_id: input.requester_user_id,
                question: &input.question,
                reply_to_message_id: input.reply_to_message_id,
                provider: &self.config.ask_llm_provider,
                model: self.config.ask_llm_model.as_deref(),
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
            failure_kind.map(AskFailureKind::as_str),
        )
        .await
        {
            tracing::warn!(%err, ask_run_id, "failed to finish ask audit run");
        }
    }
}

#[derive(Debug)]
pub struct AskServiceError {
    pub kind: AskFailureKind,
    source: anyhow::Error,
}

impl AskServiceError {
    fn new(kind: AskFailureKind, source: anyhow::Error) -> Self {
        Self { kind, source }
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
