use std::{future::Future, time::Duration};

use anyhow::Result;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::CallToolRequestParams,
    schemars, tool, tool_handler, tool_router,
    transport::{
        StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};

const STDIO_CHILD_ENV: &str = "RMCP_STDIO_ECHO_CHILD";
const E2E_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EchoRequest {
    value: String,
}

#[derive(Debug, Clone)]
struct HarnessServer {
    tool_router: ToolRouter<Self>,
}

impl HarnessServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl HarnessServer {
    #[tool(description = "Return the supplied value")]
    fn echo(&self, Parameters(EchoRequest { value }): Parameters<EchoRequest>) -> String {
        value
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for HarnessServer {}

fn echo_arguments(value: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::json!({ "value": value })
        .as_object()
        .expect("echo arguments must be a JSON object")
        .clone()
}

fn result_text(result: &rmcp::model::CallToolResult) -> &str {
    result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.as_str())
        .expect("echo result must contain text")
}

async fn within_e2e_timeout<T>(
    scenario: &str,
    future: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::time::timeout(E2E_TIMEOUT, future)
        .await
        .map_err(|_| anyhow::anyhow!("{scenario} exceeded {E2E_TIMEOUT:?}"))?
}

async fn serve_stdio_echo_child() -> Result<()> {
    HarnessServer::new()
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

async fn start_streamable_http_server() -> Result<(
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<std::io::Result<()>>,
)> {
    let service: StreamableHttpService<HarnessServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(HarnessServer::new()),
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_json_response(true)
                .with_sse_keep_alive(None),
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (shutdown, shutdown_signal) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = shutdown_signal.await;
            })
            .await
    });

    Ok((format!("http://{address}/mcp"), shutdown, server))
}

async fn stop_streamable_http_server(
    shutdown: tokio::sync::oneshot::Sender<()>,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
) -> Result<()> {
    let _ = shutdown.send(());
    within_e2e_timeout("Streamable HTTP server shutdown", async move {
        Ok(server.await??)
    })
    .await
}

#[tokio::test]
async fn stdio_echo_child_server() -> Result<()> {
    if std::env::var_os(STDIO_CHILD_ENV).is_none() {
        return Ok(());
    }

    within_e2e_timeout("stdio child server", serve_stdio_echo_child()).await
}

async fn streamable_response_json(response: reqwest::Response) -> Result<serde_json::Value> {
    let body = response.error_for_status()?.text().await?;
    if let Ok(json) = serde_json::from_str(&body) {
        return Ok(json);
    }

    body.lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim)
        .filter(|data| !data.is_empty())
        .map(serde_json::from_str)
        .next_back()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("Streamable HTTP response did not contain JSON: {body}"))
}

#[tokio::test]
async fn duplex_transport_initializes_lists_calls_and_shuts_down() -> Result<()> {
    within_e2e_timeout("duplex transport lifecycle", async {
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            HarnessServer::new()
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            anyhow::Ok(())
        });

        let mut client = ().serve(client_transport).await?;
        let tools = client.list_tools(None).await?;
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "echo");

        let result = client
            .call_tool(CallToolRequestParams::new("echo").with_arguments(echo_arguments("duplex")))
            .await?;
        assert_eq!(result_text(&result), "duplex");

        client.close().await?;
        server.await??;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn child_process_stdio_transport_initializes_lists_calls_and_closes() -> Result<()> {
    within_e2e_timeout("child-process stdio transport lifecycle", async {
        let current_test_binary = std::env::current_exe()?;
        let mut command = tokio::process::Command::new(current_test_binary);
        command.env(STDIO_CHILD_ENV, "1").args([
            "--exact",
            "stdio_echo_child_server",
            "--nocapture",
        ]);

        let transport = TokioChildProcess::new(command)?;
        let mut client = ().serve(transport).await?;
        let tools = client.list_tools(None).await?;
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "echo");

        let result = client
            .call_tool(
                CallToolRequestParams::new("echo")
                    .with_arguments(echo_arguments("child-process-stdio")),
            )
            .await?;
        assert_eq!(result_text(&result), "child-process-stdio");

        client.close().await?;
        Ok(())
    })
    .await
}

/// Compatibility coverage for the explicit pre-SEP-2567 HTTP lifecycle.
#[tokio::test]
async fn streamable_http_2025_03_26_legacy_lifecycle_remains_compatible() -> Result<()> {
    within_e2e_timeout(
        "2025-03-26 Streamable HTTP compatibility lifecycle",
        async {
            let (endpoint, cancellation, server) = start_streamable_http_server().await?;
            let http = reqwest::Client::new();

            let initialize = http
                .post(&endpoint)
                .header("Accept", "application/json, text/event-stream")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {},
                        "clientInfo": { "name": "rmcp-transport-test", "version": "1.0.0" }
                    }
                }))
                .send()
                .await?;
            assert!(initialize.status().is_success());
            let session_id = initialize
                .headers()
                .get("mcp-session-id")
                .expect("initialize must create a session")
                .to_str()?
                .to_owned();
            let initialize_body = streamable_response_json(initialize).await?;
            assert_eq!(initialize_body["result"]["protocolVersion"], "2025-03-26");

            let initialized = http
                .post(&endpoint)
                .header("Accept", "application/json, text/event-stream")
                .header("mcp-session-id", &session_id)
                .header("mcp-protocol-version", "2025-03-26")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .send()
                .await?;
            assert_eq!(initialized.status(), reqwest::StatusCode::ACCEPTED);

            let tools_response = http
                .post(&endpoint)
                .header("Accept", "application/json, text/event-stream")
                .header("mcp-session-id", &session_id)
                .header("mcp-protocol-version", "2025-03-26")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list"
                }))
                .send()
                .await?;
            let tools = streamable_response_json(tools_response).await?;
            assert_eq!(tools["result"]["tools"][0]["name"], "echo");

            let result_response = http
                .post(&endpoint)
                .header("Accept", "application/json, text/event-stream")
                .header("mcp-session-id", &session_id)
                .header("mcp-protocol-version", "2025-03-26")
                .json(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 3,
                    "method": "tools/call",
                    "params": { "name": "echo", "arguments": { "value": "http" } }
                }))
                .send()
                .await?;
            let result = streamable_response_json(result_response).await?;
            assert_eq!(result["result"]["content"][0]["text"], "http");

            let shutdown = http
                .delete(&endpoint)
                .header("mcp-session-id", &session_id)
                .header("mcp-protocol-version", "2025-03-26")
                .send()
                .await?;
            assert_eq!(shutdown.status(), reqwest::StatusCode::ACCEPTED);

            stop_streamable_http_server(cancellation, server).await?;
            Ok(())
        },
    )
    .await
}

#[tokio::test]
async fn streamable_http_official_client_initializes_lists_calls_and_closes() -> Result<()> {
    within_e2e_timeout("official Streamable HTTP client lifecycle", async {
        let (endpoint, cancellation, server) = start_streamable_http_server().await?;
        let transport = StreamableHttpClientTransport::from_uri(endpoint);
        let mut client = ().serve(transport).await?;

        let tools = client.list_tools(None).await?;
        assert_eq!(tools.tools.len(), 1);
        assert_eq!(tools.tools[0].name, "echo");

        let result = client
            .call_tool(
                CallToolRequestParams::new("echo")
                    .with_arguments(echo_arguments("official-streamable-http")),
            )
            .await?;
        assert_eq!(result_text(&result), "official-streamable-http");

        client.close().await?;
        stop_streamable_http_server(cancellation, server).await?;
        Ok(())
    })
    .await
}
