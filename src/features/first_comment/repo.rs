use sqlx::PgPool;

use crate::features::first_comment::render::ChatLinkTarget;
use crate::text::{normalize_ai_markers, strip_links};

pub struct LlmGenerationInsert<'a> {
    pub job_id: i64,
    pub provider: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub image_used: bool,
    pub response: &'a str,
    pub final_html: &'a str,
    pub attempts: &'a serde_json::Value,
    pub used_search_result_id: Option<i32>,
    pub used_chat_message_ids: &'a [i32],
}

pub async fn create_post_comment_job(
    pool: &PgPool,
    discussion_chat_id: i64,
    discussion_message_id: i32,
    source_channel_id: i64,
    source_message_id: i32,
    cleaned_post_text: &str,
) -> anyhow::Result<Option<i64>> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        insert into post_comment_jobs
            (discussion_chat_id, discussion_message_id, source_channel_id, source_message_id, cleaned_post_text)
        values ($1, $2, $3, $4, $5)
        on conflict do nothing
        returning id
        "#,
    )
    .bind(discussion_chat_id)
    .bind(discussion_message_id)
    .bind(source_channel_id)
    .bind(source_message_id)
    .bind(cleaned_post_text)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id,)| id))
}

pub async fn mark_post_comment_sent(
    pool: &PgPool,
    job_id: i64,
    bot_comment_message_id: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'sent', bot_comment_message_id = $2, updated_at = now()
        where id = $1
          and status in ('pending', 'processing')
        "#,
    )
    .bind(job_id)
    .bind(bot_comment_message_id)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn insert_llm_generation(
    pool: &PgPool,
    generation: LlmGenerationInsert<'_>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into llm_generations
            (post_comment_job_id, provider, model, prompt, image_used, response, final_html, attempts, used_search_result_id, used_chat_message_ids)
        values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
        "#,
    )
    .bind(generation.job_id)
    .bind(generation.provider)
    .bind(generation.model)
    .bind(generation.prompt)
    .bind(generation.image_used)
    .bind(generation.response)
    .bind(generation.final_html)
    .bind(generation.attempts)
    .bind(generation.used_search_result_id)
    .bind(generation.used_chat_message_ids)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn load_recent_bot_comments(pool: &PgPool) -> anyhow::Result<Vec<String>> {
    let rows = sqlx::query_as::<_, (String,)>(
        r#"
        select coalesce(response, final_html)
        from llm_generations
        where coalesce(response, final_html) is not null
        order by created_at desc
        limit 12
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(text,)| normalize_comment_text(&text))
        .filter(|text| !text.trim().is_empty())
        .collect())
}

pub async fn load_chat_link_targets(
    pool: &PgPool,
    chat_id: i64,
    message_ids: &[i32],
) -> anyhow::Result<Vec<ChatLinkTarget>> {
    let rows = sqlx::query_as::<_, (i32, String, Option<String>)>(
        r#"
        select m.message_id,
               coalesce(nullif(trim(p.first_name), ''), nullif(trim(p.username), ''), 'Участник') as author_name,
               nullif(trim(p.username), '') as author_username
        from telegram_messages m
        left join telegram_user_profiles p on p.telegram_user_id = m.user_id
        where m.chat_id = $1 and m.message_id = any($2)
          and m.deleted_by_bot_at is null and m.spam_marked_at is null
        "#,
    )
    .bind(chat_id)
    .bind(message_ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(message_id, author_name, author_username)| {
            crate::features::ask::chat_search::message_url(chat_id, message_id).map(|message_url| {
                ChatLinkTarget {
                    message_id,
                    author_name,
                    author_username,
                    message_url,
                }
            })
        })
        .collect())
}

fn normalize_comment_text(text: &str) -> String {
    normalize_ai_markers(&strip_links(text))
}
