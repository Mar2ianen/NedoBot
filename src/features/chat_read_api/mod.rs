//! Transport-neutral, reviewed read-model for the NedoNews public chat.

use anyhow::bail;
use sqlx::PgPool;

pub mod catalog;
pub mod policy;
pub mod query;
pub mod semantic;
pub mod service;
pub mod types;

use catalog::PublicCatalog;
use types::{
    ChatInteraction, ChatMessage, ChatReadScope, ChatUserProfile, MessageSearchRequest,
    RecentMessagesRequest,
};

/// One scoped read-model shared by every MCP transport.
pub struct ChatReadApi {
    pool: PgPool,
    scope: ChatReadScope,
    catalog: PublicCatalog,
}

impl ChatReadApi {
    pub fn new(pool: PgPool, scope: ChatReadScope, catalog: PublicCatalog) -> anyhow::Result<Self> {
        if catalog.scope() != scope {
            bail!("chat read scope does not match reviewed public catalog");
        }
        Ok(Self {
            pool,
            scope,
            catalog,
        })
    }

    pub async fn validate(&self) -> anyhow::Result<()> {
        self.catalog.validate_views(&self.pool).await
    }

    pub fn scope(&self) -> ChatReadScope {
        self.scope
    }

    pub fn catalog(&self) -> &PublicCatalog {
        &self.catalog
    }

    /// Lists only views reviewed in the public MCP manifest.
    pub fn list_public_tables(&self) -> Vec<serde_json::Value> {
        self.catalog.list_tables()
    }

    /// Describes one reviewed public view, if it is in the manifest.
    pub fn describe_public_table(&self, table: &str) -> Option<serde_json::Value> {
        self.catalog.describe_table(table)
    }

    pub async fn search_messages(
        &self,
        request: &MessageSearchRequest,
    ) -> anyhow::Result<Vec<ChatMessage>> {
        service::search_messages(&self.pool, self.scope.discussion_chat_id, request).await
    }

    pub async fn recent_messages(
        &self,
        request: &RecentMessagesRequest,
    ) -> anyhow::Result<Vec<ChatMessage>> {
        service::recent_messages(&self.pool, self.scope.discussion_chat_id, request).await
    }

    pub async fn message_context(
        &self,
        message_id: i32,
        before: i64,
        after: i64,
    ) -> anyhow::Result<Vec<ChatMessage>> {
        service::message_context(
            &self.pool,
            self.scope.discussion_chat_id,
            message_id,
            before,
            after,
        )
        .await
    }

    pub async fn reply_thread(&self, message_id: i32) -> anyhow::Result<Vec<ChatMessage>> {
        service::reply_thread(&self.pool, self.scope.discussion_chat_id, message_id).await
    }

    pub async fn user_interactions(
        &self,
        first_user_id: i64,
        second_user_id: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<ChatInteraction>> {
        service::user_interactions(
            &self.pool,
            self.scope.discussion_chat_id,
            first_user_id,
            second_user_id,
            limit,
        )
        .await
    }

    pub async fn user_profile(
        &self,
        telegram_user_id: i64,
    ) -> anyhow::Result<Option<ChatUserProfile>> {
        service::user_profile(&self.pool, self.scope.discussion_chat_id, telegram_user_id).await
    }

    pub async fn resolve_users(
        &self,
        telegram_user_id: Option<i64>,
        query: Option<&str>,
    ) -> anyhow::Result<Vec<semantic::ResolvedUser>> {
        semantic::resolve_users(
            &self.pool,
            self.scope.discussion_chat_id,
            telegram_user_id,
            query,
        )
        .await
    }

    pub async fn chat_notes(&self) -> anyhow::Result<Vec<semantic::Note>> {
        semantic::chat_notes(&self.pool, self.scope.discussion_chat_id).await
    }

    pub async fn user_notes(&self, telegram_user_id: i64) -> anyhow::Result<Vec<semantic::Note>> {
        semantic::user_notes(&self.pool, self.scope.discussion_chat_id, telegram_user_id).await
    }

    pub async fn select_public(
        &self,
        request: query::SelectRequest,
    ) -> anyhow::Result<query::Page> {
        query::select(&self.pool, &self.catalog, request).await
    }

    pub async fn count_public(
        &self,
        table: String,
        filters: Vec<query::Filter>,
    ) -> anyhow::Result<i64> {
        query::count(&self.pool, &self.catalog, table, filters).await
    }

    pub async fn aggregate_public(
        &self,
        request: query::AggregateRequest,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        query::aggregate(&self.pool, &self.catalog, request).await
    }
}
