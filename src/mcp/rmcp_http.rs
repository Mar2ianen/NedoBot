//! Public Streamable HTTP adapter backed by the official RMCP service.
//!
//! The adapter owns only listener configuration and HTTP boundary checks; SQL,
//! catalog policy, and tool execution belong to the shared `ChatMcpServer`
//! read-model. Static-avatar lookup is deliberately not exposed because the server
//! has no static-file resolver and must not advertise an unverifiable URL tool.

use std::{env, net::SocketAddr, time::Duration};

use anyhow::{Context, bail};
use axum::{
    Router,
    extract::State,
    http::{StatusCode, header::ORIGIN},
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
// RMCP заменяет несовместимый legacy-контракт, поэтому URL содержит версию API.
const DEFAULT_PATH: &str = "/mcp/nedonews/v2";
const DEFAULT_MANIFEST_PATH: &str = "config/mcp_db_manifest.toml";
const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_ALLOWED_HOSTS: &[&str] = &["localhost", "127.0.0.1", "[::1]"];

struct RmcpHttpConfig {
    bootstrap: RmcpStdioConfig,
    bind: SocketAddr,
    path: String,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
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
        let path =
            parse_static_route(&env::var("MCP_PATH").unwrap_or_else(|_| DEFAULT_PATH.to_owned()))?;
        let allowed_hosts = parse_allowed_hosts(env::var("MCP_ALLOWED_HOSTS").ok().as_deref())?;
        let allowed_origins =
            parse_allowed_origins(env::var("MCP_ALLOWED_ORIGINS").ok().as_deref())?;

        Ok(Self {
            bootstrap: RmcpStdioConfig::new(database_url, manifest_path)?,
            bind,
            path,
            allowed_hosts,
            allowed_origins,
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

fn parse_static_route(value: &str) -> anyhow::Result<String> {
    let path = value.trim();
    if path.is_empty() || !path.starts_with('/') {
        bail!("MCP_PATH must be a non-empty absolute path");
    }
    if path == "/" || path.contains([':', '*', '{', '}', '?', '#']) || path.contains("//") {
        bail!(
            "MCP_PATH must be a static Axum route without parameters, wildcards, queries, or fragments"
        );
    }
    if path.split('/').skip(1).any(str::is_empty) {
        bail!("MCP_PATH must not contain empty path segments");
    }
    Ok(path.to_owned())
}

fn parse_allowed_hosts(value: Option<&str>) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(DEFAULT_ALLOWED_HOSTS
            .iter()
            .map(ToString::to_string)
            .collect());
    };

    let hosts: Vec<_> = value.split(',').map(str::trim).map(str::to_owned).collect();
    if hosts.is_empty() || hosts.iter().any(|host| host.is_empty()) {
        bail!(
            "MCP_ALLOWED_HOSTS must be a non-empty comma-separated list of host or host:port authorities"
        );
    }
    for host in &hosts {
        if host.contains('@') {
            bail!("MCP_ALLOWED_HOSTS must not contain userinfo in authority {host:?}");
        }
        host.parse::<axum::http::uri::Authority>()
            .with_context(|| format!("MCP_ALLOWED_HOSTS contains invalid authority {host:?}"))?;
    }
    Ok(hosts)
}

fn parse_allowed_origins(value: Option<&str>) -> anyhow::Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };

    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Runs the public RMCP Streamable HTTP server.
///
/// The service factory clones a server whose API shares the validated read-only
/// pool. RMCP still creates independent protocol handlers per HTTP session.
pub async fn run_public_http() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = RmcpHttpConfig::from_env()?;
    let server = build_chat_mcp_server(config.bootstrap).await?;
    let app = router(
        server,
        &config.path,
        config.allowed_hosts,
        config.allowed_origins,
    );

    info!(bind = %config.bind, path = %config.path, "NedoNews RMCP HTTP started");
    let listener = tokio::net::TcpListener::bind(config.bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(
    server: ChatMcpServer,
    path: &str,
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
) -> Router {
    let service: StreamableHttpService<ChatMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_allowed_hosts(allowed_hosts)
                .with_allowed_origins(allowed_origins.clone())
                .with_legacy_session_mode(false)
                .with_max_request_body_bytes(MAX_REQUEST_BODY_BYTES)
                .with_json_response(true)
                .with_sse_keep_alive(None),
        );

    Router::new()
        .route_service(path, service)
        .layer(middleware::from_fn_with_state(
            allowed_origins,
            enforce_origin_policy,
        ))
        .layer(middleware::from_fn_with_state((), enforce_request_timeout))
}

async fn enforce_origin_policy(
    State(allowed_origins): State<Vec<String>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(_) = request.headers().get(ORIGIN) else {
        return next.run(request).await;
    };

    // RMCP intentionally disables its origin validation for an empty list. Keep
    // that mode safe for server clients without an Origin header, but reject
    // browser-originated requests before handing them to RMCP.
    if allowed_origins.is_empty() {
        return StatusCode::FORBIDDEN.into_response();
    }

    next.run(request).await
}

async fn enforce_request_timeout(
    State(()): State<()>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    // `next` owns the request body; this timeout therefore covers streaming,
    // parsing, and RMCP handler execution without buffering the body ourselves.
    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => StatusCode::REQUEST_TIMEOUT.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, HashMap},
        sync::Arc,
    };

    use anyhow::Result;
    use reqwest::header::{HeaderName, HeaderValue};
    use rmcp::{
        ServiceExt,
        model::CallToolRequestParams,
        transport::{
            StreamableHttpClientTransport,
            streamable_http_client::StreamableHttpClientTransportConfig,
        },
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
    async fn official_rmcp_client_lists_and_calls_tools_through_http_adapter() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let app = router(
            test_server(),
            "/mcp",
            vec!["127.0.0.1".to_owned()],
            Vec::new(),
        );
        let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
        let http = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_signal.await;
                })
                .await
        });

        let transport = StreamableHttpClientTransport::from_uri(endpoint.clone());
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

        let rejected_host = reqwest::Client::new()
            .post(&endpoint)
            .header("Host", "untrusted.example")
            .body("{}")
            .send()
            .await?;
        assert_eq!(rejected_host.status(), reqwest::StatusCode::FORBIDDEN);

        let wrong_path = reqwest::Client::new()
            .get(format!("{endpoint}/unexpected"))
            .send()
            .await?;
        assert_eq!(wrong_path.status(), reqwest::StatusCode::NOT_FOUND);

        let _ = shutdown.send(());
        http.await??;
        Ok(())
    }

    #[test]
    fn host_allowlist_uses_loopback_only_when_unset_and_rejects_invalid_values() {
        assert_eq!(
            parse_allowed_hosts(None).unwrap(),
            ["localhost", "127.0.0.1", "[::1]"]
        );
        assert!(parse_allowed_hosts(Some("")).is_err());
        assert!(parse_allowed_hosts(Some("localhost,,example.com")).is_err());
        assert!(parse_allowed_hosts(Some("https://public.example")).is_err());
        assert!(parse_allowed_hosts(Some("trusted.example@evil.example")).is_err());
        assert_eq!(
            parse_allowed_hosts(Some("mcp.example.com,mcp.example.com:8443")).unwrap(),
            ["mcp.example.com", "mcp.example.com:8443"]
        );
    }

    #[tokio::test]
    async fn absent_or_empty_origin_allowlist_rejects_untrusted_origin() -> Result<()> {
        for allowed_origins in [
            parse_allowed_origins(None)?,
            parse_allowed_origins(Some("  "))?,
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let endpoint = format!("http://{}/mcp", listener.local_addr()?);
            let app = router(
                test_server(),
                "/mcp",
                vec!["127.0.0.1".to_owned()],
                allowed_origins,
            );
            let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
            let http = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_signal.await;
                    })
                    .await
            });

            let response = reqwest::Client::new()
                .post(&endpoint)
                .header("Origin", "https://untrusted.example")
                .body("{}")
                .send()
                .await?;
            assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

            let _ = shutdown.send(());
            http.await??;
        }
        Ok(())
    }

    #[tokio::test]
    async fn configured_trusted_origin_reaches_rmcp_lifecycle() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let trusted_origin = "https://trusted.example";
        let app = router(
            test_server(),
            "/mcp",
            vec!["127.0.0.1".to_owned()],
            vec![trusted_origin.to_owned()],
        );
        let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
        let http = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_signal.await;
                })
                .await
        });

        let mut headers = HashMap::new();
        headers.insert(
            HeaderName::from_static("origin"),
            HeaderValue::from_static(trusted_origin),
        );
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(endpoint).custom_headers(headers),
        );
        let mut client = ().serve(transport).await?;
        let tools = client.list_tools(None).await?;
        assert!(tools.tools.iter().any(|tool| tool.name == "db.list_tables"));
        client.close().await?;

        let _ = shutdown.send(());
        http.await??;
        Ok(())
    }

    #[tokio::test]
    async fn configured_untrusted_origin_gets_forbidden() -> Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}/mcp", listener.local_addr()?);
        let app = router(
            test_server(),
            "/mcp",
            vec!["127.0.0.1".to_owned()],
            vec!["https://trusted.example".to_owned()],
        );
        let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel::<()>();
        let http = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_signal.await;
                })
                .await
        });

        let response = reqwest::Client::new()
            .post(&endpoint)
            .header("Origin", "https://untrusted.example")
            .body("{}")
            .send()
            .await?;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        let _ = shutdown.send(());
        http.await??;
        Ok(())
    }

    #[test]
    fn static_route_validation_rejects_axum_parameter_syntax_and_prefix_routes() {
        assert_eq!(
            parse_static_route("/mcp/nedonews/v2").unwrap(),
            "/mcp/nedonews/v2"
        );
        for invalid in [
            "",
            "mcp",
            "/",
            "/mcp/:id",
            "/mcp/{id}",
            "/mcp/*rest",
            "/mcp//v1",
        ] {
            assert!(
                parse_static_route(invalid).is_err(),
                "{invalid} must be rejected"
            );
        }
    }
}
