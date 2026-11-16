use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

#[derive(Clone, Eq)]
struct ClientKey {
    timeout_secs: u64,
    proxy_url: Option<String>,
}

impl PartialEq for ClientKey {
    fn eq(&self, other: &Self) -> bool {
        self.timeout_secs == other.timeout_secs && self.proxy_url == other.proxy_url
    }
}

impl Hash for ClientKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.timeout_secs.hash(state);
        self.proxy_url.hash(state);
    }
}

static CLIENTS: LazyLock<Mutex<HashMap<ClientKey, reqwest::Client>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    client_with_proxy(timeout, None)
}

/// Extracts an HTTP status from a typed reqwest error anywhere in an error chain.
/// Callers can map provider failures without parsing display strings that may
/// change or contain transport details.
pub fn status_code(error: &anyhow::Error) -> Option<u16> {
    error.chain().find_map(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .and_then(reqwest::Error::status)
            .map(|status| status.as_u16())
    })
}

pub fn is_timeout(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(reqwest::Error::is_timeout)
    })
}

pub fn client_with_proxy(
    timeout: Duration,
    proxy_url: Option<&str>,
) -> anyhow::Result<reqwest::Client> {
    let proxy_url = proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let key = ClientKey {
        timeout_secs: timeout.as_secs(),
        proxy_url,
    };

    let mut clients = CLIENTS
        .lock()
        .map_err(|err| anyhow::anyhow!("HTTP client cache lock poisoned: {err}"))?;
    if let Some(client) = clients.get(&key) {
        return Ok(client.clone());
    }

    let mut builder = reqwest::Client::builder().timeout(timeout);
    if let Some(proxy_url) = key.proxy_url.as_deref() {
        builder = builder.proxy(reqwest::Proxy::all(proxy_url)?);
    }
    let client = builder.build()?;
    clients.insert(key, client.clone());
    Ok(client)
}

#[cfg(test)]
mod tests {
    use axum::{Router, http::StatusCode, routing::get};

    use super::{is_timeout, status_code};

    async fn rate_limited() -> StatusCode {
        StatusCode::TOO_MANY_REQUESTS
    }

    #[tokio::test]
    async fn reads_status_from_typed_reqwest_error_chain() {
        let app = Router::new().route("/", get(rate_limited));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test server should bind");
        let address = listener.local_addr().expect("test server address is known");
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let error = reqwest::Client::new()
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("mock server responds")
            .error_for_status()
            .expect_err("429 must be an error");
        server.abort();

        let error = anyhow::Error::new(error);
        assert_eq!(status_code(&error), Some(429));
        assert!(!is_timeout(&error));
    }
}
