//! Canary public Streamable HTTP adapter backed by the official RMCP service.
//!
//! This intentionally does not replace the legacy handwritten HTTP adapter.
//! Both adapters expose the same reviewed `ChatMcpServer` read-model, while this
//! module owns only listener configuration and HTTP boundary checks. The legacy
//! static-avatar lookup is deliberately not exposed here: `ChatMcpServer` has no
//! static-file resolver, so this canary must not advertise an unverifiable URL tool.

use std::{collections::BTreeSet, env, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tracing::info;

use super::{
    bootstrap::{DATABASE_URL_ENV, MANIFEST_PATH_ENV, RmcpStdioConfig, build_chat_mcp_server},
    server::ChatMcpServer,
};

const DEFAULT_BIND: &str = "127.0.0.1:8787";
const DEFAULT_PATH: &str = "/mcp/nedonews";
const DEFAULT_MANIFEST_PATH: &str = "config/mcp_db_manifest.toml";
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
struct HttpBoundaryConfig {
    allowed_origins: Arc<BTreeSet<String>>,
}

struct RmcpHttpConfig {
    bootstrap: RmcpStdioConfig,
    bind: SocketAddr,
    path: String,
    boundary: HttpBoundaryConfig,
}

impl RmcpHttpConfig {
    fn from_env() -> anyhow::Result<Self> {
        let database_url = required_env(DATABASE_URL_ENV)?;
        let manifest_path =
            env::var(MANIFEST_PATH_ENV).unwrap_or_else(|_| DEFAULT_MANIFEST_PATH.to_owned());
        let bind = env::var("MCP_BIND")
            .unwrap_or_else(|_| DEFAULT_BIND.to_owned())
            .parse()
            .context("MCP_BIND must be a socket address")?;
        let path = env::var("MCP_PATH").unwrap_or_else(|_| DEFAULT_PATH.to_owned());
        if !path.starts_with('/') || path.trim().is_empty() {
            bail!("MCP_PATH must be a non-empty absolute path");
        }

        let allowed_origins = env::var("MCP_ALLOWED_ORIGINS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|origin| !origin.is_empty())
            .map(str::to_owned)
            .collect();

        Ok(Self {
            bootstrap: RmcpStdioConfig::new(database_url, manifest_path)?,
            bind,
            path,
            boundary: HttpBoundaryConfig {
                allowed_origins: Arc::new(allowed_origins),
            },
        })
    }
}

fn required_env(name: &str) -> anyhow::Result<String> {
    let value = env::var(name).with_context(|| format!("{name} is required"))?;
    if value.trim().is_empty() {
        bail!("{name} must not be empty");
    }
    Ok(value)
}

/// Runs the isolated RMCP Streamable HTTP canary.
///
/// The service factory clones a server whose API shares the validated read-only
/// pool. RMCP still creates independent protocol handlers per HTTP session.
pub async fn run_public_http() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = RmcpHttpConfig::from_env()?;
    let server = build_chat_mcp_server(config.bootstrap).await?;
    let app = router(server, &config.path, config.boundary);

    info!(bind = %config.bind, path = %config.path, "NedoNews RMCP HTTP canary started");
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(server: ChatMcpServer, path: &str, boundary: HttpBoundaryConfig) -> Router {
    let service: StreamableHttpService<ChatMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_json_response(true)
                .with_sse_keep_alive(None),
        );

    Router::new()
        .nest_service(path, service)
        .layer(middleware::from_fn_with_state(
            boundary,
            enforce_http_boundary,
        ))
}

async fn enforce_http_boundary(
    State(config): State<HttpBoundaryConfig>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if !origin_allowed(request.headers(), &config.allowed_origins) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > MAX_REQUEST_BODY_BYTES)
    {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }

    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_REQUEST_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let request = axum::http::Request::from_parts(parts, Body::from(body));

    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

fn origin_allowed(headers: &HeaderMap, allowed: &BTreeSet<String>) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    allowed.contains(origin.to_str().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use anyhow::Result;
    use rmcp::{
        ServiceExt, model::CallToolRequestParams, transport::StreamableHttpClientTransport,
    };
    use sqlx::postgres::PgPoolOptions;

    use super::*;
    use crate::{
        features::chat_read_api::{
            ChatReadApi,
            catalog::{CatalogColumn, CatalogScope, CatalogTable, PublicCatalog},
            types::ChatReadScope,
        },
        mcp::server::ChatMcpServer,
    };

    fn test_server() -> ChatMcpServer {
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
                    description: "Public messages".into(),
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
            .expect("lazy test pool must parse");
        let api = ChatReadApi::new(
            pool,
            ChatReadScope {
                discussion_chat_id: -1001932061163,
                source_channel_id: -1001575496091,
            },
            catalog,
        )
        .expect("test catalog must be valid");
        ChatMcpServer::new(Arc::new(api))
    }

    #[tokio::test]
    async fn official_rmcp_client_lists_and_calls_tools_through_canary_adapter() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let app = router(
            test_server(),
            "/mcp",
            HttpBoundaryConfig {
                allowed_origins: Arc::new(BTreeSet::new()),
            },
        );
        let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
        let http = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_signal.await;
                })
                .await
        });

        let transport = StreamableHttpClientTransport::from_uri(endpoint);
        let mut client = ().serve(transport).await?;
        let tools = client.list_tools(None).await?;
        assert!(tools.tools.iter().any(|tool| tool.name == "db.list_tables"));
        assert!(
            !tools
                .tools
                .iter()
                .any(|tool| tool.name == "chat.get_user_avatar")
        );

        let result = client
            .call_tool(CallToolRequestParams::new("db.list_tables"))
            .await?;
        assert!(!result.is_error.unwrap_or(false));

        client.close().await?;
        let _ = shutdown.send(());
        http.await??;
        Ok(())
    }

    #[test]
    fn origin_allowlist_rejects_unlisted_browser_origin() {
        let headers = HeaderMap::from_iter([(
            header::ORIGIN,
            "https://untrusted.example".parse().expect("valid header"),
        )]);
        assert!(!origin_allowed(
            &headers,
            &BTreeSet::from(["https://trusted.example".to_owned()]),
        ));
    }
}
