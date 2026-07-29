use sqlx::PgPool;

use crate::config::Config;
use crate::features::jobs::claim::CasResult;
use crate::features::new_user_audit::prompt::{build_input, output_schema, system_prompt};
use crate::features::new_user_audit::repo::{
    NewUserAuditJob, NewUserAuditOutcome, claim_next_new_user_audit_job,
    finalize_new_user_audit_job, mark_new_user_audit_failed, mark_new_user_audit_retry,
};
use crate::features::new_user_audit::types::NewUserAuditAssessment;
use crate::llm::service::{GenerateTextOptions, generate_text_with_provider_checked};
use crate::llm::types::{LlmTransportError, StructuredOutput};

/// Обрабатывает одну готовую unified-audit job.
///
/// Снимок в job уже является каноническим входом: worker не читает профиль,
/// не скачивает аватар и не меняет модерационные оценки.
pub async fn process_next_new_user_audit_job(
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_new_user_audit_job(pool).await? else {
        return Ok(false);
    };

    process_job(pool, config, &job).await;
    Ok(true)
}

async fn process_job(pool: &PgPool, config: &Config, job: &NewUserAuditJob) {
    let result = generate_and_finalize(pool, config, job).await;
    let Err(error) = result else { return };

    let failure = classify_audit_failure(&error);
    let result = match failure {
        AuditFailure::Retry { error_kind } => {
            mark_new_user_audit_retry(pool, job, error_kind, None).await
        }
        AuditFailure::Terminal { error_kind } => {
            mark_new_user_audit_failed(pool, job, error_kind).await
        }
    };

    match result {
        Ok(CasResult::Applied) => match failure {
            AuditFailure::Retry { error_kind } => tracing::warn!(
                job_id = job.id,
                error_kind,
                "new user audit job failed and was scheduled for retry"
            ),
            AuditFailure::Terminal { error_kind } => tracing::warn!(
                job_id = job.id,
                error_kind,
                "new user audit job failed permanently"
            ),
        },
        Ok(CasResult::LeaseLost) => {
            tracing::warn!(
                job_id = job.id,
                attempts = job.attempts,
                "new user audit failure ignored because its lease was reclaimed"
            );
        }
        Err(_) => {
            tracing::warn!(
                job_id = job.id,
                "failed to persist new user audit failure state"
            );
        }
    }
}

async fn generate_and_finalize(
    pool: &PgPool,
    config: &Config,
    job: &NewUserAuditJob,
) -> anyhow::Result<()> {
    let prompt = build_input(&job.input_json)?;
    let has_avatar_input = job.avatar_file_id.is_some();
    let output_validator = move |output: &str| {
        NewUserAuditAssessment::parse_for_input(output, has_avatar_input).map(|_| ())
    };
    let generation = generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            route: Some("new_user_audit"),
            // Profile mode resolves the route above. Legacy mode needs explicit main
            // LLM settings because this feature intentionally has no separate config.
            provider_override: Some(&config.llm_provider),
            model_override: config.llm_model.as_deref(),
            system_prompt: Some(system_prompt()),
            prompt: &prompt,
            // Avatar download is deliberately outside this runtime-only slice.
            image_base64: None,
            temperature: 0.0,
            num_predict: config.llm_max_tokens,
            output_validator: Some(&output_validator),
            structured_output: Some(StructuredOutput {
                name: "new_user_audit_assessment",
                schema: output_schema(),
            }),
        },
    )
    .await?;

    NewUserAuditAssessment::parse_for_input(&generation.content, has_avatar_input)?;
    let assessment_json = serde_json::from_str(&generation.content)?;
    let finalized = finalize_new_user_audit_job(
        pool,
        job,
        NewUserAuditOutcome {
            assessment_json: &assessment_json,
            provider: &generation.provider,
            model: &generation.model,
        },
    )
    .await?;
    if finalized == CasResult::LeaseLost {
        tracing::warn!(
            job_id = job.id,
            attempts = job.attempts,
            "new user audit lease was reclaimed before finalization"
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum AuditFailure {
    Retry { error_kind: &'static str },
    Terminal { error_kind: &'static str },
}

fn classify_audit_failure(error: &anyhow::Error) -> AuditFailure {
    match error.downcast_ref::<LlmTransportError>() {
        Some(LlmTransportError::HttpStatus(400)) => AuditFailure::Terminal {
            error_kind: "http_400",
        },
        Some(LlmTransportError::HttpStatus(401)) => AuditFailure::Terminal {
            error_kind: "http_401",
        },
        Some(LlmTransportError::HttpStatus(403)) => AuditFailure::Terminal {
            error_kind: "http_403",
        },
        Some(LlmTransportError::HttpStatus(404)) => AuditFailure::Terminal {
            error_kind: "http_404",
        },
        Some(LlmTransportError::HttpStatus(408)) => AuditFailure::Retry {
            error_kind: "http_408",
        },
        Some(LlmTransportError::HttpStatus(422)) => AuditFailure::Terminal {
            error_kind: "http_422",
        },
        Some(LlmTransportError::HttpStatus(429)) => AuditFailure::Retry {
            error_kind: "http_429",
        },
        Some(LlmTransportError::HttpStatus(status)) if (500..=599).contains(status) => {
            AuditFailure::Retry {
                error_kind: "http_5xx",
            }
        }
        Some(LlmTransportError::HttpStatus(status)) if (400..=499).contains(status) => {
            AuditFailure::Terminal {
                error_kind: "http_4xx",
            }
        }
        Some(LlmTransportError::HttpStatus(_)) | Some(LlmTransportError::EmptyResponse) => {
            AuditFailure::Retry {
                error_kind: "transient",
            }
        }
        Some(LlmTransportError::Configuration) => AuditFailure::Terminal {
            error_kind: "configuration",
        },
        None if error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout) =>
        {
            AuditFailure::Retry {
                error_kind: "timeout",
            }
        }
        None => AuditFailure::Terminal {
            error_kind: "validation_failed",
        },
    }
}
