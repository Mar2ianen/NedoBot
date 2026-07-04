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
