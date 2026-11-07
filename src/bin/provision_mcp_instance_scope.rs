//! Explicitly bind one database's MCP views to a validated community profile.

use std::env;

use anyhow::{Context, bail};
use sqlx::{Row, postgres::PgPoolOptions};
use tg_ai_bot_teloxide::{
    config::load_mcp_instance_settings, features::chat_read_api::types::ChatReadScope,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let profile_path =
        env::var("LLM_PROFILES_PATH").unwrap_or_else(|_| "config/llm_profiles.toml.example".into());
    if profile_path.trim().is_empty() {
        bail!("LLM_PROFILES_PATH must not be empty");
    }
    let configured_scope = load_mcp_instance_settings(&profile_path)?.scope;
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let confirm_change = env::args().any(|arg| arg == "--confirm-scope-change");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await
        .context("cannot connect to the instance database")?;

    ensure_scoped_views(&pool).await?;
    let rows = sqlx::query(
        "select instance_id, discussion_chat_id, source_channel_id from mcp_private.instance_scope where scope_key = true",
    )
    .fetch_all(&pool)
    .await
    .context("MCP scope migration is missing; apply database migrations first")?;
    if rows.len() != 1 {
        bail!("MCP database must contain exactly one instance scope row");
    }
    let row = &rows[0];
    let current_scope = ChatReadScope::new(
        row.get::<String, _>("instance_id"),
        row.get("discussion_chat_id"),
        row.get("source_channel_id"),
    )?;
    if current_scope == configured_scope {
        println!(
            "MCP database scope already matches instance {}",
            configured_scope.instance_id()
        );
        return Ok(());
    }
    if !confirm_change {
        bail!(
            "database scope is {current_scope:?}, profile scope is {configured_scope:?}; rerun with --confirm-scope-change to provision this database"
        );
    }

    let can_update: bool = sqlx::query_scalar(
        "select has_table_privilege(current_user, 'mcp_private.instance_scope', 'UPDATE')",
    )
    .fetch_one(&pool)
    .await?;
    if !can_update {
        bail!("current database role cannot provision MCP scope; use the instance database owner");
    }

    let mut tx = pool.begin().await?;
    let changed = sqlx::query(
        "update mcp_private.instance_scope set instance_id = $1, discussion_chat_id = $2, source_channel_id = $3 where scope_key = true",
    )
    .bind(configured_scope.instance_id())
    .bind(configured_scope.discussion_chat_id())
    .bind(configured_scope.source_channel_id())
    .execute(&mut *tx)
    .await?;
    if changed.rows_affected() != 1 {
        bail!("MCP scope provisioning did not update exactly one database row");
    }
    tx.commit().await?;
    println!(
        "provisioned MCP scope for instance {}",
        configured_scope.instance_id()
    );
    Ok(())
}

async fn ensure_scoped_views(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    let view_count: i64 = sqlx::query_scalar(
        "select count(*)::bigint from information_schema.views where table_schema = 'mcp_public'",
    )
    .fetch_one(pool)
    .await?;
    anyhow::ensure!(view_count > 0, "mcp_public views are not installed");
    let unscoped_views: Vec<String> = sqlx::query_scalar(
        r#"
        select viewname
        from pg_views
        where schemaname in ('mcp_public', 'mcp_private')
          and (definition like '%-1001932061163%'
               or definition like '%-1001575496091%')
        order by viewname
        "#,
    )
    .fetch_all(pool)
    .await?;
    anyhow::ensure!(
        unscoped_views.is_empty(),
        "MCP views still contain legacy fixed chat ids: {}",
        unscoped_views.join(", ")
    );
    let unbound_views: Vec<String> = sqlx::query_scalar(
        r#"
        select schemaname || '.' || viewname
        from pg_views
        where schemaname in ('mcp_public', 'mcp_private')
          and definition not like '%mcp_private.instance_scope%'
          and not (schemaname = 'mcp_public'
                   and (definition like '%mcp_public.telegram_chat_users%'
                        or definition like '%mcp_public.telegram_user_profiles%'))
        order by schemaname, viewname
        "#,
    )
    .fetch_all(pool)
    .await?;
    anyhow::ensure!(
        unbound_views.is_empty(),
        "MCP views are not bound to the instance scope: {}",
        unbound_views.join(", ")
    );
    let required_media_view: Option<String> =
        sqlx::query_scalar("select to_regclass('mcp_private.telegram_media')::text")
            .fetch_one(pool)
            .await?;
    anyhow::ensure!(
        required_media_view.as_deref() == Some("mcp_private.telegram_media"),
        "mcp_private.telegram_media is missing"
    );
    Ok(())
}
