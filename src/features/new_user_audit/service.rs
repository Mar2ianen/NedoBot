use anyhow::Context;
use serde_json::Value;
use sqlx::{PgPool, Row};
use teloxide::prelude::Bot;

use crate::config::Config;
use crate::features::first_message_spam::{spam_similarity, template_match_count};
use crate::features::jobs::claim::CasResult;
use crate::features::memory::embedding::{embed_text, pgvector_literal};
use crate::features::new_user_audit::prompt::{build_input, output_schema, system_prompt};
use crate::features::new_user_audit::repo::{
    NewUserAuditJob, NewUserAuditOutcome, claim_next_new_user_audit_job_with_materialization,
    finalize_authoritative_new_user_audit_job, finalize_new_user_audit_job,
    mark_new_user_audit_failed, mark_new_user_audit_materialization_retry,
    mark_new_user_audit_materialization_stale, mark_new_user_audit_retry,
    materialize_authoritative_new_user_audit_job,
};
use crate::features::new_user_audit::scoring::{FirstMessageScoreContext, score_assessment};
use crate::features::new_user_audit::types::NewUserAuditAssessment;
use crate::features::user_profiles::avatar::cache_profile_avatar;
use crate::llm::service::{GenerateTextOptions, generate_text_with_provider_checked};
use crate::llm::types::{LlmTransportError, StructuredOutput};

/// Обрабатывает одну готовую unified-audit job.
///
/// Снимок в job уже является каноническим входом: worker не читает профиль,
/// не скачивает аватар и не меняет модерационные оценки.
pub async fn process_next_new_user_audit_job(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_new_user_audit_job_with_materialization(
        pool,
        config.new_user_audit_authoritative_enabled,
    )
    .await?
    else {
        return Ok(false);
    };

    process_job(bot, pool, config, &job).await;
    Ok(true)
}

async fn process_job(bot: &Bot, pool: &PgPool, config: &Config, job: &NewUserAuditJob) {
    let result = if job.is_materialization_replay {
        materialize_stored_assessment(pool, config, job).await
    } else {
        generate_and_finalize(bot, pool, config, job).await
    };
    let Err(error) = result else { return };

    if job.is_materialization_replay {
        let failure = classify_materialization_failure(&error);
        let (result, error_kind) = match failure {
            MaterializationFailure::Retry { error_kind } => (
                mark_new_user_audit_materialization_retry(pool, job, error_kind).await,
                error_kind,
            ),
            MaterializationFailure::Stale { error_kind } => (
                mark_new_user_audit_materialization_stale(pool, job, error_kind).await,
                error_kind,
            ),
        };
        log_materialization_failure(result, job, error_kind);
        return;
    }

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

async fn materialize_stored_assessment(
    pool: &PgPool,
    config: &Config,
    job: &NewUserAuditJob,
) -> anyhow::Result<()> {
    let assessment_json = job
        .assessment_json
        .as_ref()
        .context("materialization replay requires stored assessment")?;
    let assessment = parse_stored_assessment(job, assessment_json)
        .map_err(|error| MalformedStoredAssessment(error.to_string()))?;
    let (baseline_score, baseline_signals) = load_baseline_component(pool, job).await?;
    let first_message_context =
        load_first_message_score_context(pool, config, job, &assessment).await?;
    let components = score_assessment(
        baseline_score,
        baseline_signals,
        &assessment,
        first_message_context,
    );
    let finalized = materialize_authoritative_new_user_audit_job(pool, job, &components).await?;
    if finalized == CasResult::LeaseLost {
        tracing::warn!(
            job_id = job.id,
            attempts = job.attempts,
            "new user audit materialization lease was reclaimed"
        );
    }
    Ok(())
}

fn log_materialization_failure(
    result: anyhow::Result<CasResult>,
    job: &NewUserAuditJob,
    error_kind: &str,
) {
    match result {
        Ok(CasResult::Applied) => tracing::warn!(
            job_id = job.id,
            error_kind,
            "stored audit assessment was not materialized"
        ),
        Ok(CasResult::LeaseLost) => {
            tracing::warn!(job_id = job.id, "stale materialization failure ignored")
        }
        Err(_) => tracing::warn!(
            job_id = job.id,
            "failed to persist materialization failure state"
        ),
    }
}

async fn generate_and_finalize(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
    job: &NewUserAuditJob,
) -> anyhow::Result<()> {
    let image_base64 = load_avatar_input(bot, config, job).await?;
    let has_avatar_input = image_base64.is_some();
    let mut input_json = job.input_json.clone();
    if let Some(profile) = input_json.get_mut("profile").and_then(Value::as_object_mut) {
        profile.insert(
            "avatar_image_available".to_string(),
            Value::Bool(has_avatar_input),
        );
    }
    let prompt = build_input(&input_json)?;
    let has_first_message_input = has_first_message_input(&input_json);
    let output_validator = move |output: &str| {
        NewUserAuditAssessment::parse_for_modalities(
            output,
            has_avatar_input,
            has_first_message_input,
        )
        .map(|_| ())
    };
    let generation = generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            route: Some("new_user_audit"),
            // Profile mode resolves the route above. Legacy mode keeps this audit
            // independent from the ordinary generation provider.
            provider_override: Some(&config.new_user_audit_provider),
            model_override: config.new_user_audit_model.as_deref(),
            system_prompt: Some(system_prompt()),
            prompt: &prompt,
            image_base64: image_base64.as_deref(),
            temperature: 0.0,
            num_predict: config.new_user_audit_max_tokens,
            output_validator: Some(&output_validator),
            structured_output: Some(StructuredOutput {
                name: "new_user_audit_assessment",
                schema: output_schema(),
            }),
        },
    )
    .await?;

    NewUserAuditAssessment::parse_for_modalities(
        &generation.content,
        has_avatar_input,
        has_first_message_input,
    )?;
    let assessment_json = serde_json::from_str(&generation.content)?;
    let outcome = NewUserAuditOutcome {
        assessment_json: &assessment_json,
        provider: &generation.provider,
        model: &generation.model,
    };
    let finalized = if config.new_user_audit_authoritative_enabled {
        finalize_authoritative_new_user_audit_job(pool, job, outcome).await?
    } else {
        finalize_new_user_audit_job(pool, job, outcome).await?
    };
    if finalized == CasResult::LeaseLost {
        tracing::warn!(
            job_id = job.id,
            attempts = job.attempts,
            "new user audit lease was reclaimed before finalization"
        );
    }
    Ok(())
}

/// Валидирует сохранённый результат в контексте канонического снимка job.
///
/// Аватар считаем доступным консервативно: после успешной генерации Telegram
/// может удалить file reference, но это не должно делать ранее сохранённое
/// наблюдение невалидным при replay.
fn parse_stored_assessment(
    job: &NewUserAuditJob,
    assessment_json: &Value,
) -> anyhow::Result<NewUserAuditAssessment> {
    NewUserAuditAssessment::parse_for_modalities(
        &serde_json::to_string(assessment_json)?,
        true,
        has_first_message_input(&job.input_json),
    )
}

fn has_first_message_input(input_json: &Value) -> bool {
    input_json["text"]["first_message_preview"]
        .as_str()
        .is_some_and(|text| !text.trim().is_empty())
}

async fn load_first_message_score_context(
    pool: &PgPool,
    config: &Config,
    job: &NewUserAuditJob,
    assessment: &NewUserAuditAssessment,
) -> anyhow::Result<FirstMessageScoreContext> {
    if assessment.first_message_assessment.is_none() {
        return Ok(FirstMessageScoreContext::default());
    }
    let row = sqlx::query(
        "select first_message_text, first_name_feminine_pattern from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_one(pool)
    .await?;
    let Some(text) = row.get::<Option<String>, _>("first_message_text") else {
        return Ok(FirstMessageScoreContext::default());
    };
    if text.trim().is_empty() {
        return Ok(FirstMessageScoreContext::default());
    }
    let embedding = embed_text(config, &text).await?;
    let embedding = pgvector_literal(&embedding)?;
    Ok(FirstMessageScoreContext {
        template_matches: template_match_count(pool, job.chat_id, job.telegram_user_id, &text)
            .await?,
        spam_similarity: spam_similarity(pool, &embedding).await?,
        feminine_profile_name: row.get("first_name_feminine_pattern"),
    })
}

async fn load_baseline_component(
    pool: &PgPool,
    job: &NewUserAuditJob,
) -> anyhow::Result<(i32, Value)> {
    let row = sqlx::query(
        "select risk_baseline_score, risk_baseline_signals from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_one(pool)
    .await?;
    Ok((
        row.get("risk_baseline_score"),
        row.get("risk_baseline_signals"),
    ))
}

async fn load_avatar_input(
    bot: &Bot,
    config: &Config,
    job: &NewUserAuditJob,
) -> anyhow::Result<Option<String>> {
    let cached = match cache_profile_avatar(
        bot,
        &config.static_files_dir,
        job.telegram_user_id,
        job.avatar_file_id.as_deref(),
        job.avatar_file_unique_id.as_deref(),
    )
    .await
    {
        Ok(cached) => cached,
        // Telegram may have discarded an old file reference. This is an expected
        // text-only audit state, not a reason to burn the whole retry budget.
        Err(error)
            if error
                .downcast_ref::<teloxide::RequestError>()
                .is_some_and(|error| matches!(error, teloxide::RequestError::Api(_))) =>
        {
            tracing::info!(
                job_id = job.id,
                "unified audit avatar is unavailable; continuing without image"
            );
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let Some(cached) = cached else {
        return Ok(None);
    };
    Ok(Some(cached.base64().await?))
}

#[derive(Debug)]
struct MalformedStoredAssessment(String);

impl std::fmt::Display for MalformedStoredAssessment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "malformed stored assessment: {}", self.0)
    }
}

impl std::error::Error for MalformedStoredAssessment {}

#[derive(Clone, Copy)]
enum MaterializationFailure {
    Retry { error_kind: &'static str },
    Stale { error_kind: &'static str },
}

fn classify_materialization_failure(error: &anyhow::Error) -> MaterializationFailure {
    if error.downcast_ref::<MalformedStoredAssessment>().is_some() {
        return MaterializationFailure::Stale {
            error_kind: "malformed_assessment",
        };
    }
    if error.downcast_ref::<sqlx::Error>().is_some() {
        return MaterializationFailure::Retry {
            error_kind: "sql_transient",
        };
    }
    if error.downcast_ref::<reqwest::Error>().is_some() {
        return MaterializationFailure::Retry {
            error_kind: "embedding_transient",
        };
    }
    MaterializationFailure::Retry {
        error_kind: "materialization_transient",
    }
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
        Some(
            LlmTransportError::Timeout
            | LlmTransportError::HttpStatus(_)
            | LlmTransportError::EmptyResponse
            | LlmTransportError::InvalidResponse
            | LlmTransportError::StructuredOutputRejected,
        ) => AuditFailure::Retry {
            error_kind: "transient",
        },
        Some(LlmTransportError::UnsupportedFeature) => AuditFailure::Terminal {
            error_kind: "unsupported_feature",
        },
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn job_with_input(input_json: Value) -> NewUserAuditJob {
        NewUserAuditJob {
            id: 1,
            chat_id: 1,
            telegram_user_id: 1,
            snapshot_hash: "snapshot".to_string(),
            prompt_version: "prompt".to_string(),
            input_json,
            avatar_file_id: None,
            avatar_file_unique_id: None,
            assessment_json: None,
            attempts: 1,
            materialization_attempts: 1,
            is_materialization_replay: true,
        }
    }

    fn assessment_without_first_message() -> Value {
        json!({
            "avatar_observation": null,
            "first_message_assessment": null,
            "profile_assessment": {
                "risk_patterns": ["no_material_risk_pattern"],
                "evidence": [],
                "contradictions": ["Нет независимых признаков."],
                "review_priority": "low",
                "confidence": 0.5,
                "summary": "Оснований для проверки нет."
            }
        })
    }

    #[test]
    fn stored_replay_requires_first_message_assessment_when_job_input_has_first_message() {
        let job = job_with_input(json!({
            "text": { "first_message_preview": "Здравствуйте, предлагаю заработок" }
        }));
        let error = parse_stored_assessment(&job, &assessment_without_first_message())
            .expect_err("stored replay must honor first-message input")
            .to_string();

        assert!(error.contains("first_message_assessment must be present"));
    }

    #[test]
    fn stored_replay_keeps_avatar_validation_conservative() {
        let job = job_with_input(json!({ "text": { "first_message_preview": null } }));
        let mut assessment = assessment_without_first_message();
        assessment["avatar_observation"] = json!({
            "primary_class": "ordinary_personal",
            "personal_photo_probability": 0.9,
            "secondary_classes": [],
            "face_visibility": "clear",
            "adult_level": "none",
            "visual_motifs": ["лицо"],
            "description": "Фотография человека.",
            "confidence": 0.8
        });

        parse_stored_assessment(&job, &assessment)
            .expect("stored avatar observation must remain valid during replay");
    }
}
