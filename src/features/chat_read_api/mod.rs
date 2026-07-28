//! Transport-neutral, reviewed read-model for the NedoNews public chat.

use anyhow::bail;
use sqlx::PgPool;

pub mod catalog;
pub mod policy;
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
}
