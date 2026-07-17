use serde_json::Value;
use sqlx::PgPool;
use sqlx::types::Json;

use crate::config::Config;

pub async fn create_run(
    pool: &PgPool,
    config: &Config,
    chat_id: i64,
    command_message_id: i32,
    requester_user_id: i64,
    question: &str,
    reply_to_message_id: Option<i32>,
) -> anyhow::Result<i64> {
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

pub async fn record_tool_call(
    pool: &PgPool,
    ask_run_id: i64,
    step_number: i32,
    tool_name: &str,
    arguments: &Value,
    status: &str,
    result_count: Option<i64>,
    latency_ms: Option<i64>,
    error_kind: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into ask_tool_calls (
            ask_run_id, step_number, tool_name, arguments, status,
            result_count, latency_ms, error_kind
        )
        values ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(ask_run_id)
    .bind(step_number)
    .bind(tool_name)
    .bind(Json(arguments))
    .bind(status)
    .bind(result_count)
    .bind(latency_ms)
    .bind(error_kind)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish_run(
    pool: &PgPool,
    ask_run_id: i64,
    status: &str,
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
    .bind(status)
    .bind(error_kind)
    .bind(answer_markdown)
    .execute(pool)
    .await?;
    Ok(())
}
