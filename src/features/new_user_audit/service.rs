use anyhow::Context;
use serde_json::Value;
use sqlx::{PgPool, Row};
use teloxide::prelude::Bot;

use crate::config::Config;
use crate::features::jobs::claim::CasResult;
use crate::features::memory::embedding::{embed_text, pgvector_literal};
use crate::features::new_user_audit::prompt::{build_input, output_schema, system_prompt};
use crate::features::new_user_audit::repo::{
    NewUserAuditJob, NewUserAuditOutcome, claim_next_new_user_audit_job,
    finalize_new_user_audit_job, is_transient_sqlx_error, mark_new_user_audit_failed,
    mark_new_user_audit_materialization_retry, mark_new_user_audit_materialization_stale,
    mark_new_user_audit_retry, materialize_new_user_audit_job,
};
use crate::features::new_user_audit::scoring::{spam_similarity, template_match_count};
use crate::features::user_profiles::avatar::cache_profile_avatar;
use crate::llm::service::{GenerateTextOptions, generate_text_checked};
use crate::llm::types::{LlmTransportError, StructuredOutput};
use teloxide_antispam::assessment::NewUserAuditAssessment;
use teloxide_antispam::scoring::{
    FirstMessageScoreContext, is_rkn_vpn_restriction_context, score_assessment,
};

/// Обрабатывает одну готовую unified-audit job.
///
/// Снимок в job уже является каноническим входом: worker не читает профиль,
/// не скачивает аватар и не меняет модерационные оценки.
pub async fn process_next_new_user_audit_job(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_new_user_audit_job(pool).await? else {
        return Ok(false);
    };

    process_job(bot, pool, config, &job).await;
    Ok(true)
}

async fn process_job(bot: &Bot, pool: &PgPool, config: &Config, job: &NewUserAuditJob) {
    if job.is_materialization_replay {
        match materialize_stored_assessment(pool, config, job).await {
            Ok(Some(score)) => {
                if let Err(error) = crate::features::auto_moderation::maybe_enforce_audit(
                    bot, pool, config, job, score,
                )
                .await
                {
                    tracing::warn!(
                        job_id = job.id,
                        %error,
                        "auto moderation enforcement failed"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => {
                if let Some(sqlx::Error::Database(database_error)) =
                    error.downcast_ref::<sqlx::Error>()
                {
                    tracing::warn!(
                        job_id = job.id,
                        sqlstate = ?database_error.code(),
                        constraint = ?database_error.constraint(),
                        "new user audit materialization hit a database error"
                    );
                }
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
        }
        return;
    }
    let result = generate_and_finalize(bot, pool, config, job).await;
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

async fn materialize_stored_assessment(
    pool: &PgPool,
    config: &Config,
    job: &NewUserAuditJob,
) -> anyhow::Result<Option<i32>> {
    let assessment_json = job
        .assessment_json
        .as_ref()
        .context("materialization replay requires stored assessment")?;
    let assessment = parse_stored_assessment(job, assessment_json)
        .map_err(|error| MalformedStoredAssessment(error.to_string()))?;
    let (baseline_score, baseline_signals) = load_baseline_component(pool, job).await?;
    let (mut first_message_context, reputation_inputs) =
        load_first_message_score_context(pool, config, job, &assessment).await?;
    let provisional = score_assessment(
        baseline_score,
        baseline_signals.clone(),
        &assessment,
        first_message_context.clone(),
        job.review_threshold,
    );
    // Second pass folds the reputation head in: its audit_risk feature is the
    // pre-reputation total, exactly the quantity training snapshots hold.
    // Reputation is a small supporting slot, never decisive.
    if let Some((probability, version, calibration)) = score_reputation(
        config,
        job,
        &first_message_context,
        &reputation_inputs,
        provisional.final_score(),
    ) {
        first_message_context.reputation_probability = Some(probability);
        first_message_context.reputation_model_version = Some(version);
        first_message_context.reputation_calibration = Some(calibration);
    }
    let mut components = score_assessment(
        baseline_score,
        baseline_signals,
        &assessment,
        first_message_context,
        job.review_threshold,
    );
    // CAS — слабый внешний сигнал: положительный вердикт добавляет не более
    // EXTERNAL_SCORE_CAP, unknown/clean ничего не меняют. Проверка выполняется
    // для каждого аудита, а не только при наличии первого сообщения: рецидивист
    // из глобального banlist опознаётся и по пустому профилю.
    if config.community.moderation.cas_enabled {
        let verdict = teloxide_antispam::external::check_cas(
            job.telegram_user_id,
            std::time::Duration::from_secs(config.community.moderation.cas_timeout_sec),
        )
        .await;
        let (score, signals) = teloxide_antispam::external::external_component(verdict);
        components.apply_external(score, signals);
    }
    let finalized =
        materialize_new_user_audit_job(pool, job, &components, &config.rag_embedding_model).await?;
    if finalized == CasResult::LeaseLost {
        tracing::warn!(
            job_id = job.id,
            attempts = job.attempts,
            "new user audit materialization lease was reclaimed"
        );
        return Ok(None);
    }
    Ok(Some(components.final_score()))
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
    let generation = generate_text_checked(
        config,
        GenerateTextOptions {
            route: "new_user_audit",
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
    let finalized = finalize_new_user_audit_job(pool, job, outcome).await?;
    if finalized == CasResult::LeaseLost {
        tracing::warn!(
            job_id = job.id,
            attempts = job.attempts,
            "new user audit lease was reclaimed before finalization"
        );
    }
    Ok(())
}

/// Валидирует сохранённый результат перед materialization replay.
///
/// Stored assessment уже прошёл modality validation на generation boundary.
/// Поэтому исторический Telegram file ID не используется для восстановления
/// факта реально скачанного изображения: старый ID может остаться в job, даже
/// если generation продолжила работу в text-only режиме.
fn parse_stored_assessment(
    job: &NewUserAuditJob,
    assessment_json: &Value,
) -> anyhow::Result<NewUserAuditAssessment> {
    let assessment =
        NewUserAuditAssessment::parse_stored(&serde_json::to_string(assessment_json)?)?;

    if has_first_message_input(&job.input_json) && assessment.first_message_assessment.is_none() {
        anyhow::bail!(
            "first_message_assessment must be present when stored input has a first message"
        );
    }
    if !has_first_message_input(&job.input_json) && assessment.first_message_assessment.is_some() {
        anyhow::bail!(
            "first_message_assessment must be null when stored input has no first message"
        );
    }

    let has_avatar_metadata = job.avatar_file_id.is_some() || job.avatar_file_unique_id.is_some();
    if !has_avatar_metadata && assessment.avatar_observation.is_some() {
        anyhow::bail!("stored avatar observation has no corresponding avatar metadata");
    }

    Ok(assessment)
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
) -> anyhow::Result<(FirstMessageScoreContext, ReputationInputs)> {
    let personal_channel_content = job.input_json["personal_channel"]["recent_content_preview"]
        .as_str()
        .filter(|content| !content.trim().is_empty())
        .map(str::to_owned);
    if assessment.first_message_assessment.is_none() {
        return Ok((
            FirstMessageScoreContext {
                personal_channel_content,
                ..Default::default()
            },
            ReputationInputs::default(),
        ));
    }
    let row = sqlx::query(
        "select first_message_id, first_message_text, first_name_feminine_pattern, message_count, link_count, max_normalized_message_reuse_count, account_seen_age_sec from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(job.chat_id)
    .bind(job.telegram_user_id)
    .fetch_one(pool)
    .await?;
    let reputation_inputs = ReputationInputs {
        message_count: row.get::<i64, _>("message_count"),
        link_count: row.get::<i64, _>("link_count"),
        dup_reuse: row
            .get::<Option<i64>, _>("max_normalized_message_reuse_count")
            .unwrap_or(0),
        account_age_sec: row
            .get::<Option<i64>, _>("account_seen_age_sec")
            .unwrap_or(0),
        ..Default::default()
    };
    let Some(text) = row.get::<Option<String>, _>("first_message_text") else {
        return Ok((
            FirstMessageScoreContext {
                personal_channel_content,
                ..Default::default()
            },
            reputation_inputs,
        ));
    };
    if text.trim().is_empty() {
        return Ok((
            FirstMessageScoreContext {
                personal_channel_content,
                ..Default::default()
            },
            reputation_inputs,
        ));
    }
    let mut reputation_inputs = reputation_inputs;
    reputation_inputs.is_command = text.trim_start().starts_with('/');
    let embedding = embed_text(config, &text).await?;
    let embedding = pgvector_literal(&embedding)?;
    let reply_context = first_message_reply_context(&job.input_json);
    let linear_spam_probability = config
        .linear_spam_model
        .as_ref()
        .map(|model| teloxide_antispam::logreg::spam_probability(model, &text));
    let (
        embedding_spam_probability,
        embedding_model_version,
        embedding_head_version,
        embedding_calibration,
    ) = load_embedding_spam_signal(
        pool,
        config,
        job.chat_id,
        row.get::<Option<i32>, _>("first_message_id"),
    )
    .await;
    let context = FirstMessageScoreContext {
        template_matches: template_match_count(pool, job.chat_id, job.telegram_user_id, &text)
            .await?,
        spam_similarity: spam_similarity(pool, job.telegram_user_id, &embedding).await?,
        feminine_profile_name: row.get("first_name_feminine_pattern"),
        rkn_vpn_restriction_context: is_rkn_vpn_restriction_context(reply_context),
        personal_channel_content,
        // Персист корпуса для будущих similarity-проверок выполняется
        // в materialize через ScoreComponents.first_message_embedding.
        embedding: Some(embedding),
        linear_spam_probability,
        linear_spam_model_version: config
            .linear_spam_model
            .as_ref()
            .map(|model| model.version.clone()),
        text_observations: Some(teloxide_antispam::preprocess::prepare_text(&text).flags),
        linear_spam_calibration: config
            .linear_spam_model
            .as_ref()
            .map(|model| model.calibration.clone()),
        embedding_spam_probability,
        embedding_model_version,
        embedding_head_version,
        embedding_calibration,
        // Category heads have no trained weights yet; the embedding head is
        // configured explicitly and stays a supporting signal.
        ..Default::default()
    };
    let behavior = load_behavior_signals(pool, job.chat_id, job.telegram_user_id).await;
    if let Some(behavior) = behavior {
        reputation_inputs.active_days = behavior.active_days;
        reputation_inputs.received = behavior.received;
    }
    Ok((context, reputation_inputs))
}

/// Point-in-time behavior for the reputation head. Best-effort: any failure
/// disables reputation for this audit instead of failing it.
#[derive(Debug, Clone, Default)]
struct ReputationInputs {
    message_count: i64,
    link_count: i64,
    dup_reuse: i64,
    account_age_sec: i64,
    active_days: i64,
    received: Vec<(String, i64)>,
    /// Bot commands (`/cmd`) carry no spam evidence; training drops them.
    is_command: bool,
}

struct BehaviorSignals {
    active_days: i64,
    received: Vec<(String, i64)>,
}

async fn load_behavior_signals(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> Option<BehaviorSignals> {
    let active_days: Option<i64> = sqlx::query_scalar(
        "select count(distinct date_trunc('day', created_at)) from telegram_messages where chat_id = $1 and user_id = $2",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()?;
    let rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        select je.emoji, count(*)::bigint
        from telegram_message_reactions r
        join telegram_messages m on m.chat_id = r.chat_id and m.message_id = r.message_id
        join jsonb_to_recordset(r.new_reactions) as je(emoji text) on true
        where m.chat_id = $1 and m.user_id = $2 and je.emoji is not null
        group by je.emoji
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .ok()?;
    Some(BehaviorSignals {
        active_days: active_days.unwrap_or(0),
        received: rows,
    })
}

/// Builds the 12 reputation features in artifact order and scores the head.
/// Commands (`/cmd`) and missing heads/models are silent `None`: no evidence,
/// never an audit failure.
fn score_reputation(
    config: &Config,
    job: &NewUserAuditJob,
    context: &FirstMessageScoreContext,
    inputs: &ReputationInputs,
    provisional_score: i32,
) -> Option<(f64, String, teloxide_statistics::reputation::Calibration)> {
    let head = config.reputation_model.as_ref()?;
    if inputs.is_command {
        return None;
    }
    let id_model = config.moderation_risk_profile()?.telegram_id.as_ref()?;
    let id_prior =
        teloxide_antispam::signals::telegram_id_spam_probability(job.telegram_user_id, id_model);
    let mut pos = 0u64;
    let mut neg = 0u64;
    let mut total = 0u64;
    for (emoji, count) in &inputs.received {
        let count = (*count).max(0) as u64;
        total += count;
        match teloxide_statistics::sentiment::classify(emoji) {
            teloxide_statistics::sentiment::ReactionSentiment::Positive => pos += count,
            teloxide_statistics::sentiment::ReactionSentiment::Negative => neg += count,
            _ => {}
        }
    }
    let decisive = pos + neg;
    let positivity = if decisive > 0 {
        pos as f64 / decisive as f64
    } else {
        0.5
    };
    let values = [
        id_prior,
        f64::from(provisional_score.clamp(0, 100)) / 100.0,
        (inputs.message_count.max(0) as f64 + 1.0).ln(),
        (inputs.active_days.max(0) as f64 + 1.0).ln(),
        inputs.link_count.max(0) as f64 / inputs.message_count.max(1) as f64,
        (inputs.dup_reuse.max(0) as f64 + 1.0).ln(),
        positivity,
        neg as f64 / total.max(1) as f64,
        (inputs.account_age_sec.max(0) as f64 / 86_400.0 + 1.0).ln(),
        context.linear_spam_probability.unwrap_or(0.5),
        context.embedding_spam_probability.unwrap_or(0.5),
        1.0,
    ];
    let probability = head.score(&values)?;
    Some((probability, head.version.clone(), head.calibration.clone()))
}

/// Scores the stored Gemma retrieval vector when the embedding head is
/// configured. Reads only `ready` vectors with a matching `embedding_model`;
/// a missing, mismatched or unreadable vector is no evidence (`None`), never
/// a failure of the whole audit.
async fn load_embedding_spam_signal(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    message_id: Option<i32>,
) -> (
    Option<f64>,
    Option<String>,
    Option<String>,
    Option<teloxide_antispam::calibration::LinearScoreCalibration>,
) {
    let Some(head) = config.embedding_spam_model.as_ref() else {
        return (None, None, None, None);
    };
    let Some(message_id) = message_id else {
        return (None, None, None, None);
    };
    let stored: Option<(String, String)> = match sqlx::query_as(
        "select embedding::text, embedding_model from telegram_message_embeddings_gemma where chat_id = $1 and message_id = $2 and status = 'ready'",
    )
    .bind(chat_id)
    .bind(message_id)
    .fetch_optional(pool)
    .await
    {
        Ok(stored) => stored,
        Err(error) => {
            tracing::warn!(%error, chat_id, message_id, "embedding spam lookup failed");
            return (None, None, None, None);
        }
    };
    let Some((literal, stored_model)) = stored else {
        return (None, None, None, None);
    };
    if stored_model != head.embedding_model {
        return (None, None, None, None);
    }
    let vector = parse_halfvec_literal(&literal);
    let Some(vector) = vector else {
        tracing::warn!(chat_id, message_id, "embedding spam vector unparsable");
        return (None, None, None, None);
    };
    let probability = head.spam_probability(&vector);
    if probability.is_none() {
        tracing::warn!(
            chat_id,
            message_id,
            "embedding spam vector rejected by head"
        );
    }
    (
        probability,
        Some(head.embedding_model.clone()),
        Some(head.version.clone()),
        Some(head.calibration.clone()),
    )
}

/// Parses a pgvector `halfvec`/`vector` text literal (`[0.1,0.2,...]`) into
/// floats. Rejects empty input, missing brackets and non-finite values so a
/// corrupt stored row can never become model evidence.
fn parse_halfvec_literal(literal: &str) -> Option<Vec<f32>> {
    let values: Option<Vec<f32>> = literal
        .trim()
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .and_then(|inner| {
            inner
                .split(',')
                .map(|part| part.trim().parse::<f32>().ok())
                .collect::<Option<Vec<_>>>()
        });
    match values {
        Some(values) if !values.is_empty() && values.iter().all(|value| value.is_finite()) => {
            Some(values)
        }
        _ => None,
    }
}

fn first_message_reply_context(input_json: &Value) -> &str {
    input_json["text"]["first_message_reply_context_preview"]
        .as_str()
        .unwrap_or_default()
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
    if let Some(sql_error) = error.downcast_ref::<sqlx::Error>() {
        return if is_transient_sqlx_error(sql_error) {
            MaterializationFailure::Retry {
                error_kind: "sql_transient",
            }
        } else {
            MaterializationFailure::Stale {
                error_kind: "sql_permanent",
            }
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

    #[test]
    fn halfvec_literal_parsing_rejects_corrupt_rows() {
        assert_eq!(
            parse_halfvec_literal("[0.5, -1.25, 3]"),
            Some(vec![0.5, -1.25, 3.0])
        );
        for bad in ["", "[]", "[0.1", "0.1]", "[NaN]", "[inf]", "[0.1, oops]"] {
            assert_eq!(parse_halfvec_literal(bad), None, "must reject {bad:?}");
        }
    }

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
            review_threshold: 70,
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
    fn first_message_reply_context_comes_from_the_job_snapshot() {
        let job = job_with_input(json!({
            "text": {
                "first_message_reply_context_preview":
                    "Как настроить VPN для обхода блокировок?"
            }
        }));

        let reply_context = first_message_reply_context(&job.input_json);

        assert_eq!(reply_context, "Как настроить VPN для обхода блокировок?");
        assert!(is_rkn_vpn_restriction_context(reply_context));
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
    fn stored_replay_rejects_first_message_assessment_without_job_input() {
        let job = job_with_input(json!({ "text": { "first_message_preview": null } }));
        let mut assessment = assessment_without_first_message();
        assessment["first_message_assessment"] = json!({
            "relation_to_chat": "on_topic",
            "direct_dm_offer": false,
            "offtopic_promo": false,
            "template_campaign": false,
            "self_reference_grammar": "none_or_unclear",
            "profile_name_grammar_relation": "not_applicable",
            "risk_markers": [],
            "evidence": [],
            "summary": "Контекста первого сообщения нет.",
            "confidence": 0.8
        });

        let error = parse_stored_assessment(&job, &assessment)
            .expect_err("stored replay must reject an assessment without first-message input")
            .to_string();

        assert!(error.contains("first_message_assessment must be null"));
    }

    #[test]
    fn stored_replay_rejects_avatar_observation_without_avatar_input() {
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

        let error = parse_stored_assessment(&job, &assessment)
            .expect_err("stored avatar observation requires avatar input")
            .to_string();

        assert!(error.contains("stored avatar observation has no corresponding avatar metadata"));
    }

    #[test]
    fn stored_replay_accepts_text_only_assessment_with_stale_avatar_id() {
        let mut job = job_with_input(json!({ "text": { "first_message_preview": null } }));
        job.avatar_file_id = Some("stale-file-id".to_string());

        let assessment = parse_stored_assessment(&job, &assessment_without_first_message())
            .expect("stored text-only assessment must survive replay");

        assert_eq!(assessment.avatar_observation, None);
    }
}
