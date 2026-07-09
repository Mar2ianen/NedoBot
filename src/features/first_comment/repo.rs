use sqlx::PgPool;

use crate::features::memory::service::extract_keywords;
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
            (post_comment_job_id, provider, model, prompt, image_used, response, final_html, attempts)
        values ($1, $2, $3, $4, $5, $6, $7, $8)
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

pub async fn load_topic_bot_comments(
    pool: &PgPool,
    post_text: &str,
) -> anyhow::Result<Vec<String>> {
    let post_keywords = extract_keywords(post_text);
    if post_keywords.is_empty() {
        return Ok(Vec::new());
    }

    let rows = sqlx::query_as::<_, (String, String)>(
        r#"
        select pcj.cleaned_post_text,
               coalesce(lg.response, lg.final_html)
        from llm_generations lg
        join post_comment_jobs pcj on pcj.id = lg.post_comment_job_id
        where coalesce(lg.response, lg.final_html) is not null
        order by lg.created_at desc
        limit 160
        "#,
    )
    .fetch_all(pool)
    .await?;

    let mut scored = rows
        .into_iter()
        .filter_map(|(old_post, comment)| {
            let old_keywords = extract_keywords(&old_post);
            let score = old_keywords
                .iter()
                .filter(|keyword| post_keywords.contains(keyword))
                .count();
            (score >= 2).then_some((score, normalize_comment_text(&comment)))
        })
        .filter(|(_, comment)| !comment.trim().is_empty())
        .collect::<Vec<_>>();

    scored.sort_by(|(left_score, _), (right_score, _)| right_score.cmp(left_score));

    let mut comments = Vec::new();
    for (_, comment) in scored {
        if !comments.iter().any(|saved| saved == &comment) {
            comments.push(comment);
        }
        if comments.len() >= 6 {
            break;
        }
    }

    Ok(comments)
}

fn normalize_comment_text(text: &str) -> String {
    normalize_ai_markers(&strip_links(text))
}
