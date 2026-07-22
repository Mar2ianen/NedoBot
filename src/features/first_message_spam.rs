use std::collections::BTreeSet;

use serde_json::{Value, json};
use sqlx::{PgPool, Row};

use crate::config::Config;
use crate::features::memory::embedding::{embed_text, pgvector_literal};
use crate::llm::service::{GenerateTextOptions, generate_text_with_provider_checked};
use crate::llm::types::StructuredOutput;
use crate::text::first_text_chars;

const PROMPT_VERSION: &str = "first-message-spam-v2";
const POST_CONTEXT_LIMIT: usize = 700;
const SYSTEM_PROMPT: &str = include_str!("../../prompts/first_message_spam_classification.md");

pub async fn analyze_first_message(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        "select first_message_text, first_message_analysis_at from telegram_new_user_profile_audits where chat_id = $1 and telegram_user_id = $2",
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(false) };
    if row
        .get::<Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>>, _>(
            "first_message_analysis_at",
        )
        .is_some()
    {
        return Ok(false);
    }
    let Some(text) = row.get::<Option<String>, _>("first_message_text") else {
        return Ok(false);
    };
    if text.trim().is_empty() {
        return Ok(false);
    }

    let embedding = embed_text(config, &text).await?;
    let embedding_literal = pgvector_literal(&embedding)?;
    let template_matches = template_match_count(pool, chat_id, user_id, &text).await?;
    let similarity = spam_similarity(pool, &embedding_literal).await?;
    let replied_post_context = replied_post_context(pool, chat_id, user_id).await?;
    let assessment = classify_text(config, &text, replied_post_context.as_deref()).await?;
    let delta = score_delta(&assessment, template_matches, similarity);
    let signal = json!({
        "class": "first_message_content",
        "label": "first_message_spam_analysis",
        "reason": format!("LLM markers; post context: {}; template matches: {template_matches}; spam similarity: {:.3}", if replied_post_context.is_some() { "available" } else { "absent" }, similarity.unwrap_or(0.0)),
        "coefficient": delta,
        "warning_strength": if delta >= 30 { "strong" } else { "supporting" },
        "assessment": assessment
    });
    let updated = sqlx::query(
        r#"
        update telegram_new_user_profile_audits
        set first_message_marker_assessment = $3,
            first_message_embedding = $4::vector,
            first_message_embedding_model = $5,
            first_message_spam_similarity = $6,
            first_message_template_matches = $7,
            first_message_analysis_at = now(),
            risk_score = least(100, risk_score + $8),
            risk_level = case when least(100, risk_score + $8) >= 70 then 'high'
                              when least(100, risk_score + $8) >= 40 then 'medium' else 'low' end,
            risk_signal_breakdown = coalesce(risk_signal_breakdown, '[]'::jsonb) || jsonb_build_array($9::jsonb)
        where chat_id = $1 and telegram_user_id = $2 and first_message_analysis_at is null
        returning risk_level
        "#,
    )
    .bind(chat_id).bind(user_id).bind(&assessment).bind(&embedding_literal)
    .bind(&config.rag_embedding_model).bind(similarity).bind(template_matches).bind(delta).bind(&signal)
    .fetch_optional(pool).await?;
    Ok(updated.is_some())
}

async fn replied_post_context(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
) -> anyhow::Result<Option<String>> {
    let context = sqlx::query_scalar::<_, Option<String>>(
        r#"
        select coalesce(nullif(trim(history.summary), ''), nullif(trim(job.cleaned_post_text), ''))
        from telegram_new_user_profile_audits audit
        join telegram_messages first_message
          on first_message.chat_id = audit.chat_id
         and first_message.message_id = audit.first_message_id
        join post_comment_jobs job
          on job.discussion_chat_id = first_message.chat_id
         and job.discussion_message_id = first_message.reply_to_message_id
        left join post_history_entries history
          on history.source_channel_id = job.source_channel_id
         and history.source_message_id = job.source_message_id
         and history.status = 'ready'
        where audit.chat_id = $1 and audit.telegram_user_id = $2
        "#,
    )
    .bind(chat_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(context.map(|text| first_text_chars(&text, POST_CONTEXT_LIMIT)))
}

async fn template_match_count(
    pool: &PgPool,
    chat_id: i64,
    user_id: i64,
    text: &str,
) -> anyhow::Result<i32> {
    let rows = sqlx::query(r#"
        select distinct m.text from telegram_messages m
        where m.chat_id = $1 and m.spam_marked_at is not null and m.user_id <> $2 and m.text is not null
    "#).bind(chat_id).bind(user_id).fetch_all(pool).await?;
    let current = token_set(text);
    Ok(rows
        .into_iter()
        .filter_map(|row| row.get::<Option<String>, _>("text"))
        .filter(|candidate| jaccard(&current, &token_set(candidate)) >= 0.5)
        .count()
        .min(10) as i32)
}

async fn spam_similarity(pool: &PgPool, embedding: &str) -> anyhow::Result<Option<f64>> {
    let value = sqlx::query_scalar::<_, Option<f64>>(r#"
        select max(1.0 - (a.first_message_embedding <=> $1::vector))
        from telegram_new_user_profile_audits a
        join telegram_chat_users u on u.chat_id = a.chat_id and u.telegram_user_id = a.telegram_user_id
        where u.is_spammer and a.first_message_embedding is not null
    "#).bind(embedding).fetch_one(pool).await?;
    Ok(value)
}

async fn classify_text(
    config: &Config,
    text: &str,
    replied_post_context: Option<&str>,
) -> anyhow::Result<Value> {
    let prompt = serde_json::to_string(&json!({
        "untrusted_first_message": text,
        "trusted_replied_post_context": replied_post_context,
        "prompt_version": PROMPT_VERSION
    }))?;
    let generation = generate_text_with_provider_checked(
        config,
        GenerateTextOptions {
            provider_override: Some("cerebras"),
            model_override: config.avatar_classifier_model.as_deref(),
            system_prompt: Some(SYSTEM_PROMPT),
            prompt: &prompt,
            image_base64: None,
            temperature: 0.0,
            num_predict: 450,
            output_validator: None,
            structured_output: Some(StructuredOutput {
                name: "first_message_spam_assessment",
                schema: output_schema(),
            }),
        },
    )
    .await?;
    let value: Value = serde_json::from_str(&generation.content)?;
    Ok(value)
}

fn output_schema() -> &'static Value {
    static SCHEMA: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| {
        json!({
            "type":"object", "additionalProperties":false,
            "properties": {
                "direct_dm_offer":{"type":"boolean"}, "offtopic_promo":{"type":"boolean"}, "template_campaign":{"type":"boolean"},
                "relation_to_replied_post":{"type":"string","enum":["on_topic","loosely_related","off_topic","no_post_context"]},
                "markers":{"type":"array","items":{"type":"string","enum":["send_or_share_offer","direct_messages","self_help_or_finance_promo","template_efficiency_narrative","masked_call_to_action"]}},
                "evidence":{"type":"array","items":{"type":"object","additionalProperties":false,"properties":{"marker":{"type":"string","enum":["send_or_share_offer","direct_messages","self_help_or_finance_promo","template_efficiency_narrative","masked_call_to_action"]},"quote":{"type":"string"}},"required":["marker","quote"]}},
                "explanation":{"type":"string"}
            }, "required":["direct_dm_offer","offtopic_promo","template_campaign","relation_to_replied_post","markers","evidence","explanation"]
        })
    });
    &SCHEMA
}

fn score_delta(assessment: &Value, template_matches: i32, similarity: Option<f64>) -> i32 {
    let direct = assessment["direct_dm_offer"].as_bool().unwrap_or(false);
    let offtopic = assessment["offtopic_promo"].as_bool().unwrap_or(false)
        && assessment["relation_to_replied_post"].as_str() == Some("off_topic");
    let campaign = assessment["template_campaign"].as_bool().unwrap_or(false);
    let llm = match (direct, offtopic, campaign) {
        (true, true, _) => 30,
        (true, _, true) => 24,
        (_, _, true) => 12,
        _ => 0,
    };
    let template = if template_matches > 0 { 24 } else { 0 };
    let embedding = match similarity {
        Some(value) if value >= 0.88 => 20,
        Some(value) if value >= 0.78 => 10,
        _ => 0,
    };
    (llm + template + embedding).min(45)
}

fn token_set(text: &str) -> BTreeSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 4)
        .map(campaign_token)
        .collect()
}

fn campaign_token(word: &str) -> String {
    match word {
        "отправить"
        | "отправлю"
        | "переслать"
        | "перешлю"
        | "скинуть"
        | "скину"
        | "поделиться"
        | "поделюсь"
        | "закинуть"
        | "закину" => "send_offer".to_string(),
        "личку" | "личные" | "сообщения" | "стучитесь" => {
            "direct_messages".to_string()
        }
        "аудиокнигу" | "аудиокнига" | "аудиоверсия" | "текстовая" => {
            "promoted_material".to_string()
        }
        _ => word.to_owned(),
    }
}
fn jaccard(left: &BTreeSet<String>, right: &BTreeSet<String>) -> f64 {
    let union = left.union(right).count();
    if union == 0 {
        0.0
    } else {
        left.intersection(right).count() as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_similarity_catches_campaign_variants() {
        assert!(
            jaccard(
                &token_set("могу переслать аудиокнигу пишите в личку"),
                &token_set("есть аудиоверсия могу отправить пишите в личные сообщения")
            ) >= 0.4
        );
    }
    #[test]
    fn direct_dm_campaign_is_strong_but_capped() {
        assert_eq!(
            score_delta(
                &json!({"direct_dm_offer":true,"offtopic_promo":true,"template_campaign":true,"relation_to_replied_post":"off_topic"}),
                2,
                Some(0.95)
            ),
            45
        );
    }

    #[test]
    fn offtopic_claim_without_post_context_is_not_a_strong_signal() {
        assert_eq!(
            score_delta(
                &json!({"direct_dm_offer":true,"offtopic_promo":true,"template_campaign":false,"relation_to_replied_post":"no_post_context"}),
                0,
                None
            ),
            0
        );
    }
}
