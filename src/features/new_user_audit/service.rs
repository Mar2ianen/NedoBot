use sqlx::PgPool;

use crate::config::Config;
use crate::features::jobs::claim::CasResult;
use crate::features::new_user_audit::prompt::{build_input, output_schema, system_prompt};
use crate::features::new_user_audit::repo::{
    NewUserAuditJob, NewUserAuditOutcome, claim_next_new_user_audit_job,
    finalize_new_user_audit_job, mark_new_user_audit_retry,
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

    let error_kind = audit_error_kind(&error);
    match mark_new_user_audit_retry(pool, job, error_kind, None).await {
        Ok(CasResult::Applied) => {
            tracing::warn!(
                job_id = job.id,
                error_kind,
                "new user audit job failed and was scheduled for retry"
            );
        }
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
                "failed to persist new user audit retry state"
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
            output_validator: None,
            structured_output: Some(StructuredOutput {
                name: "new_user_audit_assessment",
                schema: output_schema(),
            }),
        },
    )
    .await?;

    NewUserAuditAssessment::parse(&generation.content)?;
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

fn audit_error_kind(error: &anyhow::Error) -> &'static str {
    match error.downcast_ref::<LlmTransportError>() {
        Some(LlmTransportError::HttpStatus(429)) => "http_429",
        Some(LlmTransportError::HttpStatus(_)) | Some(LlmTransportError::EmptyResponse) => {
            "transient"
        }
        Some(LlmTransportError::Configuration) => "error",
        None if error
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout) =>
        {
            "timeout"
        }
        None => "validation_failed",
    }
}
