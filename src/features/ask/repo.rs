use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::types::Json;
use teloxide::drafter::DeliveryCertainty;

use crate::features::ask::types::{AskRunStatus, ToolCallAudit};

pub struct CreateAskRunParams<'a> {
    pub chat_id: i64,
    pub command_message_id: i32,
    pub requester_user_id: i64,
    pub question: &'a str,
    pub reply_to_message_id: Option<i32>,
    pub provider: &'a str,
    pub model: Option<&'a str>,
}

#[derive(Default)]
pub struct RenderAudit {
    pub captured_now: Option<DateTime<Utc>>,
    pub dialect: Option<String>,
    pub timezone: Option<String>,
    pub renderer_revision: Option<String>,
    pub rendered_markdown: Option<String>,
    pub version: Option<String>,
    pub delivery_certainty: Option<DeliveryCertainty>,
    pub delivery_outcome: Option<String>,
}

pub async fn create_run(pool: &PgPool, input: CreateAskRunParams<'_>) -> anyhow::Result<i64> {
    let CreateAskRunParams {
        chat_id,
        command_message_id,
        requester_user_id,
        question,
        reply_to_message_id,
        provider,
        model,
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
    .bind(provider)
    .bind(model)
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
    render: RenderAudit,
    error_kind: Option<&str>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update ask_runs
        set status = $2,
            error_kind = $3,
            answer_markdown = $4,
            render_captured_now = $5,
            render_dialect = $6,
            render_timezone = $7,
            renderer_revision = $8,
            rendered_markdown = $9,
            render_version = $10,
            delivery_certainty = $11,
            delivery_outcome = $12,
            tool_call_count = (select count(*) from ask_tool_calls where ask_run_id = $1),
            step_count = coalesce(
                (select max(step_number) from ask_tool_calls where ask_run_id = $1),
                0
            ),
            completed_at = case when $2 in ('completed', 'failed') then now() else null end
        where id = $1
        "#,
    )
    .bind(ask_run_id)
    .bind(status.as_str())
    .bind(error_kind)
    .bind(answer_markdown)
    .bind(render.captured_now)
    .bind(render.dialect.as_deref())
    .bind(render.timezone.as_deref())
    .bind(render.renderer_revision.as_deref())
    .bind(render.rendered_markdown.as_deref())
    .bind(render.version.as_deref())
    .bind(render.delivery_certainty.map(delivery_certainty_as_str))
    .bind(render.delivery_outcome.as_deref())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn finish_delivery(
    pool: &PgPool,
    ask_run_id: i64,
    status: AskRunStatus,
    outcome: &str,
    certainty: Option<DeliveryCertainty>,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        update ask_runs
        set status = $2,
            delivery_outcome = $3,
            delivery_certainty = $4,
            completed_at = now()
        where id = $1
        "#,
    )
    .bind(ask_run_id)
    .bind(status.as_str())
    .bind(outcome)
    .bind(certainty.map(delivery_certainty_as_str))
    .execute(pool)
    .await?;
    Ok(())
}

fn delivery_certainty_as_str(certainty: DeliveryCertainty) -> &'static str {
    match certainty {
        DeliveryCertainty::NotAttempted => "not_attempted",
        DeliveryCertainty::Rejected => "rejected",
        DeliveryCertainty::Unknown => "unknown",
    }
}
