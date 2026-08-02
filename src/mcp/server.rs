//! RMCP server foundation for the reviewed public chat read-model.
//!
//! Transport adapters deliberately do not live here. RMCP-05 and RMCP-08 will
//! attach stdio and Streamable HTTP adapters to this `ServerHandler`.

use std::sync::Arc;

use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    tool, tool_router,
};
use serde_json::Value;

use crate::features::chat_read_api::ChatReadApi;

use super::tools::{catalog, chat, db, semantic};

/// A tools-only RMCP server over the reviewed, scoped chat read-model.
///
/// The server has no database handle of its own: every data operation is
/// delegated to `ChatReadApi`, which owns scope enforcement and read policy.
#[derive(Clone)]
pub struct ChatMcpServer {
    api: Arc<ChatReadApi>,
}

impl ChatMcpServer {
    pub fn new(api: Arc<ChatReadApi>) -> Self {
        Self { api }
    }

    pub fn api(&self) -> &Arc<ChatReadApi> {
        &self.api
    }
}

#[tool_router(server_handler)]
impl ChatMcpServer {
    #[tool(
        name = "db.list_tables",
        description = "Возвращает каталог разрешённых публичных view и их primary key. Не принимает SQL."
    )]
    fn list_tables(
        &self,
        Parameters(_): Parameters<catalog::ListTablesInput>,
    ) -> Json<catalog::ListTablesOutput> {
        Json(catalog::list_tables(&self.api))
    }

    #[tool(
        name = "db.describe_table",
        description = "Показывает колонки, типы и primary key одной разрешённой публичной view."
    )]
    fn describe_table(
        &self,
        Parameters(input): Parameters<catalog::DescribeTableInput>,
    ) -> Result<Json<catalog::DescribeTableOutput>, rmcp::ErrorData> {
        catalog::describe_table(&self.api, input).map(Json)
    }

    #[tool(
        name = "db.select",
        description = "Выполняет read-only выборку из manifest-approved mcp_public view. SQL не принимается."
    )]
    async fn select(
        &self,
        Parameters(input): Parameters<db::SelectInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        db::select(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "db.fetch_row",
        description = "Возвращает строку разрешённой mcp_public view по полному primary key."
    )]
    async fn fetch_row(
        &self,
        Parameters(input): Parameters<db::FetchRowInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        db::fetch_row(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "db.count",
        description = "Считает строки разрешённой mcp_public view по typed-фильтрам."
    )]
    async fn count(
        &self,
        Parameters(input): Parameters<db::CountInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        db::count(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "db.aggregate",
        description = "Выполняет allowlisted count/min/max/sum/avg агрегацию по mcp_public view."
    )]
    async fn aggregate(
        &self,
        Parameters(input): Parameters<db::AggregateInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        db::aggregate(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "db.search_text",
        description = "Ищет текст в одной manifest-approved text-колонке."
    )]
    async fn search_text(
        &self,
        Parameters(input): Parameters<db::SearchTextInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        db::search_text(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.search_messages",
        description = "Ищет сообщения публичного чата."
    )]
    async fn search_messages(
        &self,
        Parameters(input): Parameters<chat::SearchMessagesInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::search_messages(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.count_messages",
        description = "Точно считает сообщения публичного чата по тем же фильтрам, что и chat.search_messages. Используй для вопросов 'сколько раз' и 'сколько сообщений'."
    )]
    async fn count_messages(
        &self,
        Parameters(input): Parameters<chat::CountMessagesInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::count_messages(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.search_messages_batch",
        description = "Выполняет до шести поисков сообщений публичного чата."
    )]
    async fn search_messages_batch(
        &self,
        Parameters(input): Parameters<chat::SearchMessagesBatchInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::search_messages_batch(&self.api, input)
            .await
            .map(Json)
    }

    #[tool(
        name = "chat.get_recent_messages",
        description = "Возвращает последние сообщения публичного чата с typed-фильтрами."
    )]
    async fn recent_messages(
        &self,
        Parameters(input): Parameters<chat::RecentMessagesInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::recent_messages(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.get_message",
        description = "Возвращает сообщение по его ID."
    )]
    async fn get_message(
        &self,
        Parameters(input): Parameters<chat::MessageIdInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::get_message(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.get_message_context",
        description = "Возвращает сообщение и соседние сообщения публичного чата."
    )]
    async fn message_context(
        &self,
        Parameters(input): Parameters<chat::MessageContextInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::message_context(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.get_reply_thread",
        description = "Возвращает reply thread сообщения."
    )]
    async fn reply_thread(
        &self,
        Parameters(input): Parameters<chat::MessageIdInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::reply_thread(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.get_user_interactions",
        description = "Возвращает публичные взаимодействия двух разных участников."
    )]
    async fn user_interactions(
        &self,
        Parameters(input): Parameters<chat::UserInteractionsInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::user_interactions(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.get_user_profile",
        description = "Возвращает безопасную typed-проекцию публичного профиля участника."
    )]
    async fn user_profile(
        &self,
        Parameters(input): Parameters<chat::UserProfileInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        chat::user_profile(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "chat.resolve_user",
        description = "Находит участника публичного чата по ID, username или имени."
    )]
    async fn resolve_user(
        &self,
        Parameters(input): Parameters<semantic::ResolveUserInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::resolve_user(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "notes.list_chat",
        description = "Возвращает активные общие заметки публичного чата."
    )]
    async fn list_chat_notes(
        &self,
        Parameters(input): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_chat_notes(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "notes.list_user",
        description = "Возвращает активные заметки указанного участника публичного чата."
    )]
    async fn list_user_notes(
        &self,
        Parameters(input): Parameters<semantic::UserNotesInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_user_notes(&self.api, input).await.map(Json)
    }

    #[tool(
        name = "moderation.list_spammers",
        description = "Возвращает размеченных спамеров публичного чата."
    )]
    async fn list_spammers(
        &self,
        Parameters(_): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_view(
            &self.api,
            "telegram_chat_users",
            "spam_score",
            vec![crate::features::chat_read_api::query::Filter {
                column: "is_spammer".into(),
                op: crate::features::chat_read_api::query::FilterOp::Eq,
                value: Some(Value::Bool(true)),
                values: vec![],
                case_sensitive: false,
            }],
        )
        .await
        .map(Json)
    }

    #[tool(
        name = "ask.list_runs",
        description = "Возвращает последние публичные запуски /ask."
    )]
    async fn list_ask_runs(
        &self,
        Parameters(_): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_view(&self.api, "ask_runs", "created_at", vec![])
            .await
            .map(Json)
    }

    #[tool(
        name = "voice.list_transcripts",
        description = "Возвращает расшифровки голосовых публичного чата."
    )]
    async fn list_voice_transcripts(
        &self,
        Parameters(_): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_view(&self.api, "voice_transcription_jobs", "created_at", vec![])
            .await
            .map(Json)
    }

    #[tool(
        name = "memory.list_notes",
        description = "Возвращает атомарные RAG-карточки публичных постов."
    )]
    async fn list_memory_notes(
        &self,
        Parameters(_): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_view(&self.api, "post_history_entries", "created_at", vec![])
            .await
            .map(Json)
    }

    #[tool(
        name = "search.list_runs",
        description = "Возвращает публичные запуски поиска для комментариев."
    )]
    async fn list_search_runs(
        &self,
        Parameters(_): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_view(&self.api, "search_runs", "created_at", vec![])
            .await
            .map(Json)
    }

    #[tool(
        name = "llm.list_generations",
        description = "Возвращает публичные генерации комментариев."
    )]
    async fn list_generations(
        &self,
        Parameters(_): Parameters<semantic::EmptyInput>,
    ) -> Result<Json<Value>, rmcp::ErrorData> {
        semantic::list_view(&self.api, "llm_generations", "created_at", vec![])
            .await
            .map(Json)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::features::chat_read_api::{
        catalog::{CatalogColumn, CatalogScope, CatalogTable, PublicCatalog},
        types::ChatReadScope,
    };

    fn server_with_catalog() -> ChatMcpServer {
        let catalog = PublicCatalog {
            version: 1,
            source_schema: "public".into(),
            public_schema: "mcp_public".into(),
            scope: CatalogScope {
                discussion_chat_id: -1001932061163,
                source_channel_id: -1001575496091,
            },
            tables: BTreeMap::from([(
                "telegram_messages".into(),
                CatalogTable {
                    description: "Public chat messages".into(),
                    primary_key: vec!["chat_id".into(), "message_id".into()],
                    approximate_rows: Some(1),
                    columns: BTreeMap::from([(
                        "message_id".into(),
                        CatalogColumn {
                            pg_type: "integer".into(),
                            nullable: false,
                        },
                    )]),
                },
            )]),
        };
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@localhost/unused")
            .unwrap();
        let api = ChatReadApi::new(
            pool,
            ChatReadScope {
                discussion_chat_id: -1001932061163,
                source_channel_id: -1001575496091,
            },
            catalog,
        )
        .unwrap();
        ChatMcpServer::new(Arc::new(api))
    }

    #[tokio::test]
    async fn catalog_tool_call_succeeds_without_a_database_connection() {
        let server = server_with_catalog();
        let Json(result) = server.list_tables(Parameters(catalog::ListTablesInput {}));
        assert_eq!(result.tables[0]["name"], "telegram_messages");
    }

    #[tokio::test]
    async fn catalog_tool_call_returns_invalid_params_for_unknown_table() {
        let server = server_with_catalog();
        let result = server.describe_table(Parameters(catalog::DescribeTableInput {
            table: "not_reviewed".into(),
        }));
        assert!(matches!(result, Err(error) if error.message == "unknown table"));
    }

    #[test]
    fn public_rmcp_tool_set_is_exact() {
        let mut actual = ChatMcpServer::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        actual.sort_unstable();
        assert_eq!(
            actual,
            [
                "ask.list_runs",
                "chat.count_messages",
                "chat.get_message",
                "chat.get_message_context",
                "chat.get_recent_messages",
                "chat.get_reply_thread",
                "chat.get_user_interactions",
                "chat.get_user_profile",
                "chat.resolve_user",
                "chat.search_messages",
                "chat.search_messages_batch",
                "db.aggregate",
                "db.count",
                "db.describe_table",
                "db.fetch_row",
                "db.list_tables",
                "db.search_text",
                "db.select",
                "llm.list_generations",
                "memory.list_notes",
                "moderation.list_spammers",
                "notes.list_chat",
                "notes.list_user",
                "search.list_runs",
                "voice.list_transcripts",
            ]
        );
    }
}
