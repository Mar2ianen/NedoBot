use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use teloxide::prelude::Bot;

use crate::config::Config;
use crate::features::avatar_analysis::prompt::{
    PROMPT_VERSION, build_input, output_schema, system_prompt,
};
use crate::features::avatar_analysis::repo::{
    AvatarAnalysisJob, claim_next_avatar_analysis_job, enqueue_avatar_analysis_job,
    mark_avatar_analysis_failed, mark_avatar_analysis_succeeded,
};
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
    let result = async {
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
        mark_avatar_analysis_succeeded(
            pool,
            &job,
            &generation.provider,
            &generation.model,
            &input_hash,
            observation,
            assessment,
            &response,
        )
        .await
    }
    .await;
    if let Err(err) = result {
        let kind = if err.to_string().contains("429") {
            "http_429"
        } else {
            "error"
        };
        if let Err(save_err) = mark_avatar_analysis_failed(pool, &job, kind, None).await {
            tracing::warn!(%save_err, job_id = job.id, "failed to persist avatar analysis error");
        }
        tracing::warn!(job_id = job.id, error_kind = kind, "avatar analysis failed");
    }
}
