use anyhow::Context;
use sqlx::PgPool;
use teloxide::{
    prelude::*,
    types::{MessageId, ParseMode},
};
use tg_ai_bot_teloxide::{
    config::Config,
    db::{build_pool, migrate},
    features::{
        first_comment::{
            draft::{
                first_comment_output_schema, parse_first_comment_draft,
                validate_first_comment_draft_with_search,
            },
            prompt::{CommentDirectives, build_llm_prompt_parts},
            render::build_comment_html,
            repo::{
                LlmGenerationInsert, insert_llm_generation, load_recent_bot_comments,
                load_topic_bot_comments, mark_post_comment_sent,
            },
        },
        memory::service::{load_relevant_memory_notes, remember_post},
    },
    llm::service::generate_text_checked_with_system_and_schema,
    telegram::render::send_html_reply,
};

#[derive(Debug)]
struct Args {
    limit: i64,
}

#[derive(Debug)]
struct PendingJob {
    id: i64,
    discussion_chat_id: i64,
    discussion_message_id: i32,
    source_channel_id: i64,
    source_message_id: i32,
    cleaned_post_text: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = parse_args()?;
    let config = Config::from_env();
    config.validate_runtime_secrets()?;
    let pool = build_pool().await?;
    migrate(&pool).await?;
    let bot = Bot::from_env().parse_mode(ParseMode::Html);

    let jobs = load_pending_jobs(&pool, args.limit).await?;
    println!("pending jobs: {}", jobs.len());

    for job in jobs {
        println!(
            "retry job id={} source_message_id={} discussion_message_id={}",
            job.id, job.source_message_id, job.discussion_message_id
        );

        match retry_job(&bot, &pool, &config, &job).await {
            Ok(message_id) => println!("sent job id={} bot_message_id={}", job.id, message_id),
            Err(err) => {
                mark_job_failed(&pool, job.id, &format!("{err:#}")).await?;
                println!("failed job id={}: {err:#}", job.id);
            }
        }
    }

    Ok(())
}

fn parse_args() -> anyhow::Result<Args> {
    let mut limit = 10i64;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--limit" => {
                limit = args
                    .next()
                    .context("--limit requires value")?
                    .parse()
                    .context("invalid --limit")?;
            }
            "-h" | "--help" => {
                println!("Usage: retry_pending_comments [--limit 10]");
                std::process::exit(0);
            }
            _ => anyhow::bail!("unknown option: {arg}"),
        }
    }

    Ok(Args { limit })
}

async fn load_pending_jobs(pool: &PgPool, limit: i64) -> anyhow::Result<Vec<PendingJob>> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query_as::<_, (i64, i64, i32, i64, i32, String)>(
        r#"
        with claimed as (
            select id,
                   discussion_chat_id,
                   discussion_message_id,
                   source_channel_id,
                   source_message_id,
                   cleaned_post_text
            from post_comment_jobs
            where status = 'pending'
            order by created_at
            for update skip locked
            limit $1
        )
        update post_comment_jobs as jobs
        set status = 'processing', updated_at = now()
        from claimed
        where jobs.id = claimed.id
        returning claimed.id,
                  claimed.discussion_chat_id,
                  claimed.discussion_message_id,
                  claimed.source_channel_id,
                  claimed.source_message_id,
                  claimed.cleaned_post_text
        "#,
    )
    .bind(limit)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                discussion_chat_id,
                discussion_message_id,
                source_channel_id,
                source_message_id,
                cleaned_post_text,
            )| PendingJob {
                id,
                discussion_chat_id,
                discussion_message_id,
                source_channel_id,
                source_message_id,
                cleaned_post_text,
            },
        )
        .collect())
}

async fn retry_job(
    bot: &teloxide::adaptors::DefaultParseMode<Bot>,
    pool: &PgPool,
    config: &Config,
    job: &PendingJob,
) -> anyhow::Result<i32> {
    let memory_notes = load_relevant_memory_notes(pool, &job.cleaned_post_text).await?;
    let recent_comments = load_recent_bot_comments(pool).await?;
    let topic_comments = load_topic_bot_comments(pool, &job.cleaned_post_text).await?;
    let prompt = build_llm_prompt_parts(
        &job.cleaned_post_text,
        None,
        &memory_notes,
        &recent_comments,
        &topic_comments,
        None,
        CommentDirectives::for_post(job.source_message_id, None),
    );
    let validator = |value: &str| validate_first_comment_draft_with_search(value, &[], false);
    let generation = generate_text_checked_with_system_and_schema(
        config,
        &prompt.system,
        &prompt.user,
        None,
        config.llm_temperature,
        config.llm_max_tokens,
        Some(&validator),
        first_comment_output_schema(),
    )
    .await?;
    let draft = parse_first_comment_draft(&generation.content)?;
    let used_search_result_id = draft.used_search_result_id.map(|id| id as i32);
    let prompt_for_log = prompt.compact_for_log();
    let attempts = serde_json::to_value(&generation.attempts)?;
    let final_html = build_comment_html(&draft.comment, config);
    if final_html.trim().is_empty() {
        anyhow::bail!(
            "empty rendered comment from LLM response: {}",
            draft.comment.chars().take(120).collect::<String>()
        );
    }

    let sent = send_html_reply(
        bot,
        ChatId(job.discussion_chat_id),
        MessageId(job.discussion_message_id),
        final_html.clone(),
    )
    .await?;

    mark_post_comment_sent(pool, job.id, sent.id.0).await?;
    insert_llm_generation(
        pool,
        LlmGenerationInsert {
            job_id: job.id,
            provider: &generation.provider,
            model: &generation.model,
            prompt: &prompt_for_log,
            image_used: false,
            response: &draft.comment,
            final_html: &final_html,
            attempts: &attempts,
            used_search_result_id,
        },
    )
    .await?;

    if let Err(err) = remember_post(
        pool,
        config,
        job.source_channel_id,
        job.source_message_id,
        &job.cleaned_post_text,
    )
    .await
    {
        tracing::warn!(%err, "failed to save post memory note");
    }

    Ok(sent.id.0)
}

async fn mark_job_failed(pool: &PgPool, job_id: i64, error: &str) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update post_comment_jobs
        set status = 'failed', error = $2, updated_at = now()
        where id = $1
          and status = 'processing'
        "#,
    )
    .bind(job_id)
    .bind(error)
    .execute(pool)
    .await?;

    Ok(())
}
