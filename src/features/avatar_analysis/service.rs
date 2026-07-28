use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use teloxide::prelude::Bot;

use crate::config::Config;
use crate::features::avatar_analysis::prompt::{
    PROMPT_VERSION, build_input, output_schema, system_prompt,
};
use crate::features::avatar_analysis::repo::{
    AvatarAnalysisJob, AvatarAnalysisSuccess, claim_next_avatar_analysis_job,
    enqueue_avatar_analysis_job, mark_avatar_analysis_failed, mark_avatar_analysis_succeeded,
};
use crate::features::jobs::claim::CasResult;
use crate::features::spam_review::{create_review, send_review};
use crate::features::user_profiles::avatar::cache_profile_avatar;
use crate::llm::service::{GenerateTextOptions, generate_text_with_provider_checked};
use crate::llm::types::StructuredOutput;

pub async fn enqueue_current_avatar_analysis(pool: &PgPool, user_id: i64) -> anyhow::Result<()> {
    let row = sqlx::query(
        r#"
        select p.profile_photo_file_id, p.profile_photo_file_unique_id,
               jsonb_build_object(
                   'username', p.username,
                   'first_name', p.first_name,
                   'last_name', p.last_name,
                   'bio', p.bio,
                   'profile_photo_count', p.profile_photo_count,
                   'personal_channel_title', p.personal_channel_title,
                   'personal_channel_username', p.personal_channel_username,
                   'personal_channel_last_text', p.personal_channel_last_text,
                   'personal_channel_has_adult_links', p.personal_channel_has_adult_links,
                   'message_count', coalesce(cu.message_count, 0),
                   'link_count', coalesce(cu.link_count, 0),
                   'first_seen_at', cu.first_seen_at,
                   'last_seen_at', cu.last_seen_at,
                   'chat_behavior', coalesce((
                       select jsonb_build_object(
                           'chat_id', audit.chat_id,
                           'analyzed_at', audit.analyzed_at,
                           'risk_score', audit.risk_score,
                           'risk_level', audit.risk_level,
                           'primary_risk_class', audit.primary_risk_class,
                           'risk_class_scores', audit.risk_class_scores,
                           'risk_labels', audit.risk_labels,
                           'risk_signal_breakdown', audit.risk_signal_breakdown,
                           'message_style', audit.raw_features -> 'message_style',
                           'chat_context', audit.raw_features -> 'chat_context'
                       )
                       from telegram_new_user_profile_audits audit
                       where audit.telegram_user_id = p.telegram_user_id
                       order by audit.analyzed_at desc
                       limit 1
                   ), '{}'::jsonb),
                   'avatar_seen_count', (
                       select count(*) from telegram_profile_identity_observations o
                       where o.profile_photo_file_unique_id = p.profile_photo_file_unique_id
                   ),
                   'avatar_spammer_count', (
                       select count(*) from telegram_profile_identity_observations o
                       join telegram_chat_users other on other.telegram_user_id = o.telegram_user_id
                       where o.profile_photo_file_unique_id = p.profile_photo_file_unique_id
                         and other.is_spammer
                   )
               ) as features_json
        from telegram_user_profiles p
        left join telegram_chat_users cu on cu.telegram_user_id = p.telegram_user_id
        where p.telegram_user_id = $1
        order by cu.last_seen_at desc nulls last
        limit 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let file_id: Option<String> = row.get("profile_photo_file_id");
    let unique_id: Option<String> = row.get("profile_photo_file_unique_id");
    let (Some(file_id), Some(unique_id)) = (file_id, unique_id) else {
        return Ok(());
    };
    let features: serde_json::Value = row.get("features_json");
    let bytes = serde_json::to_vec(&features)?;
    let snapshot_hash = format!("{:x}", Sha256::digest(bytes));
    enqueue_avatar_analysis_job(
        pool,
        user_id,
        &file_id,
        &unique_id,
        &snapshot_hash,
        &features,
        PROMPT_VERSION,
    )
    .await
}

pub async fn process_next_avatar_analysis_job(
    bot: &Bot,
    pool: &PgPool,
    config: &Config,
) -> anyhow::Result<bool> {
    let Some(job) = claim_next_avatar_analysis_job(pool).await? else {
        return Ok(false);
    };
    process_job(bot, pool, config, job).await;
    Ok(true)
}

async fn process_job(bot: &Bot, pool: &PgPool, config: &Config, job: AvatarAnalysisJob) {
    let result: anyhow::Result<()> = async {
        let avatar = cache_profile_avatar(
            bot,
            &config.static_files_dir,
            job.telegram_user_id,
            Some(&job.profile_photo_file_id),
            Some(&job.profile_photo_file_unique_id),
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("profile avatar is unavailable"))?;
        let image_base64 = avatar.base64().await?;
        let prompt = build_input(&job.features_json)?;
        let input_hash = format!("{:x}", Sha256::digest(prompt.as_bytes()));
        let generation = generate_text_with_provider_checked(
            config,
            GenerateTextOptions {
                provider_override: Some("cerebras"),
                model_override: config.avatar_classifier_model.as_deref(),
                system_prompt: Some(system_prompt()),
                prompt: &prompt,
                image_base64: Some(&image_base64),
                temperature: 0.0,
                num_predict: config.avatar_classifier_max_tokens,
                output_validator: None,
                structured_output: Some(StructuredOutput {
                    name: "avatar_profile_assessment",
                    schema: output_schema(),
                }),
            },
        )
        .await?;
        let response: serde_json::Value = serde_json::from_str(&generation.content)?;
        let observation = response
            .get("avatar_observation")
            .ok_or_else(|| anyhow::anyhow!("missing avatar observation"))?;
        let assessment = response
            .get("profile_assessment")
            .ok_or_else(|| anyhow::anyhow!("missing profile assessment"))?;
        let finalized = mark_avatar_analysis_succeeded(
            pool,
            &job,
            AvatarAnalysisSuccess {
                provider: &generation.provider,
                model: &generation.model,
                input_hash: &input_hash,
                observation,
                assessment,
                response: &response,
            },
        )
        .await?;
        if finalized == CasResult::LeaseLost {
            tracing::warn!(job_id = job.id, attempts = job.attempts, "avatar analysis lease was reclaimed before finalization");
            return Ok(());
        }
        let affected_chat_ids = apply_avatar_risk_signal(
            pool,
            job.telegram_user_id,
            &job.profile_photo_file_unique_id,
            observation,
        )
        .await?;
        for chat_id in affected_chat_ids {
            if let Some(review) = create_review(pool, chat_id, job.telegram_user_id).await?
                && let Err(err) = send_review(bot, pool, &review).await
            {
                tracing::warn!(%err, user_id = job.telegram_user_id, "failed to send avatar risk review");
            }
        }
        Ok(())
    }
    .await;
    if let Err(err) = result {
        let kind = if err.to_string().contains("429") {
            "http_429"
        } else {
            "error"
        };
        match mark_avatar_analysis_failed(pool, &job, kind, None).await {
            Ok(CasResult::Applied) => {
                tracing::warn!(job_id = job.id, error_kind = kind, "avatar analysis failed");
            }
            Ok(CasResult::LeaseLost) => {
                tracing::warn!(
                    job_id = job.id,
                    attempts = job.attempts,
                    "avatar analysis failure ignored after lease was reclaimed"
                );
            }
            Err(save_err) => {
                tracing::warn!(%save_err, job_id = job.id, "failed to persist avatar analysis error");
            }
        }
    }
}

pub async fn apply_avatar_risk_signal(
    pool: &PgPool,
    user_id: i64,
    profile_photo_file_unique_id: &str,
    observation: &serde_json::Value,
) -> anyhow::Result<Vec<i64>> {
    let primary_class = observation
        .get("primary_class")
        .and_then(serde_json::Value::as_str);
    let personal_photo_probability = observation
        .get("personal_photo_probability")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let (coefficient, label, reason) = match primary_class {
        Some("suggestive_bait") => (
            8,
            "suggestive_avatar_bait",
            "Avatar analysis found a suggestive bait-style portrait",
        ),
        Some("ordinary_personal") if personal_photo_probability >= 0.8 => (
            3,
            "photorealistic_personal_portrait",
            "Avatar analysis found a photorealistic personal portrait",
        ),
        _ => return Ok(Vec::new()),
    };
    let signal = serde_json::json!({
        "class": "llm_profile_bait",
        "label": label,
        "reason": reason,
        "coefficient": coefficient,
        "warning_strength": "weak",
        "assessment": { "avatar_primary_class": primary_class, "personal_photo_probability": personal_photo_probability }
    });
    let rows = sqlx::query(
        r#"
        update telegram_new_user_profile_audits audit
        set risk_score = least(100, audit.risk_score + $3),
            risk_level = case when least(100, audit.risk_score + $3) >= 70 then 'high'
                              when least(100, audit.risk_score + $3) >= 40 then 'medium' else 'low' end,
            risk_signal_breakdown = coalesce(audit.risk_signal_breakdown, '[]'::jsonb)
                || jsonb_build_array($4::jsonb)
        from telegram_user_profiles profile
        where audit.telegram_user_id = $1
          and profile.telegram_user_id = audit.telegram_user_id
          and profile.profile_photo_file_unique_id = $2
          and not exists (
              select 1
              from jsonb_array_elements(coalesce(audit.risk_signal_breakdown, '[]'::jsonb)) item
              where item ->> 'label' = $5
          )
        returning audit.chat_id
        "#,
    )
    .bind(user_id)
    .bind(profile_photo_file_unique_id)
    .bind(coefficient)
    .bind(signal)
    .bind(label)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|row| row.get("chat_id")).collect())
}
