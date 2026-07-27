use sqlx::PgPool;
use sqlx::types::Json;

use crate::config::Config;
use crate::features::ask::types::{AskRunStatus, ToolCallAudit};

pub struct CreateAskRunParams<'a> {
    pub chat_id: i64,
    pub command_message_id: i32,
    pub requester_user_id: i64,
    pub question: &'a str,
    pub reply_to_message_id: Option<i32>,
}

pub async fn create_run(
    pool: &PgPool,
    config: &Config,
    input: CreateAskRunParams<'_>,
) -> anyhow::Result<i64> {
    let CreateAskRunParams {
        chat_id,
        command_message_id,
        requester_user_id,
        question,
        reply_to_message_id,
    } = input;
    sqlx::query_scalar(
        r#"
        insert into ask_runs (
            chat_id, command_message_id, requester_user_id, question, reply_to_message_id,
            provider, model, status
        )
        values ($1, $2, $3, $4, $5, $6, $7, 'running')
        returning id
        "#,
    )
    .bind(chat_id)
    .bind(command_message_id)
    .bind(requester_user_id)
    .bind(question)
    .bind(reply_to_message_id)
    .bind(&config.ask_llm_provider)
    .bind(config.ask_llm_model.as_deref())
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

pub async fn record_tool_call(pool: &PgPool, audit: ToolCallAudit<'_>) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into ask_tool_calls (
            ask_run_id, step_number, tool_name, arguments, status,
            result_count, latency_ms, error_kind
        )
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(audit.ask_run_id)
    .bind(audit.step_number)
    .bind(audit.tool_name)
    .bind(Json(audit.arguments))
    .bind(audit.status.as_str())
    .bind(audit.result_count)
    .bind(audit.latency_ms)
    .bind(audit.error_kind)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish_run(
    pool: &PgPool,
    ask_run_id: i64,
    status: AskRunStatus,
    answer_markdown: Option<&str>,
    error_kind: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update ask_runs
        set status = $2,
            error_kind = $3,
            answer_markdown = $4,
            tool_call_count = (select count(*) from ask_tool_calls where ask_run_id = $1),
            step_count = coalesce(
                (select max(step_number) from ask_tool_calls where ask_run_id = $1),
                0
            ),
            completed_at = now()
        where id = $1
        "#,
    )
    .bind(ask_run_id)
    .bind(status.as_str())
    .bind(error_kind)
    .bind(answer_markdown)
    .execute(pool)
    .await?;
    Ok(())
}
