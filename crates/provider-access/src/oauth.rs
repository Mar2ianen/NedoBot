use crate::{ChatGptProfile, ProviderError, Result, TokenSet};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use uuid::Uuid;

const ISSUER: &str = "https://auth.openai.com";
const OPENAI_RESOURCE: &str = "https://api.openai.com/v1";
const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";
const CALLBACK_PATH: &str = "/auth/callback";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct OAuthClientMetadata {
    pub issuer: Url,
    pub authorization_endpoint: Url,
    pub token_endpoint: Url,
    pub jwks_uri: Url,
    pub revocation_endpoint: Url,
}

#[derive(Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    revocation_endpoint: String,
}

impl OAuthClientMetadata {
    pub async fn discover(client: &reqwest::Client) -> Result<Self> {
        let discovery_url = format!("{ISSUER}/.well-known/openid-configuration");
        let response = client
            .get(discovery_url)
            .timeout(DISCOVERY_TIMEOUT)
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let document: DiscoveryDocument = response.json().await?;
        if document.issuer != ISSUER {
            return Err(ProviderError::InvalidConfiguration);
        }
        let issuer = Url::parse(&document.issuer)?;
        let authorization_endpoint = parse_trusted_endpoint(&document.authorization_endpoint)?;
        let token_endpoint = parse_trusted_endpoint(&document.token_endpoint)?;
        let jwks_uri = parse_trusted_endpoint(&document.jwks_uri)?;
        let revocation_endpoint = parse_trusted_endpoint(&document.revocation_endpoint)?;
        Ok(Self {
            issuer,
            authorization_endpoint,
            token_endpoint,
            jwks_uri,
            revocation_endpoint,
        })
    }
}

#[derive(Clone)]
pub struct AuthorizationAttempt {
    pub(crate) state: String,
    pub(crate) nonce: String,
    pub(crate) pkce_verifier: String,
    pub(crate) redirect_uri: String,
    pub(crate) client_id: String,
    pub(crate) is_new_registration: bool,
    pub(crate) host_id: String,
    pub(crate) expected_subject: Option<String>,
    pub(crate) id_token_hint: Option<String>,
    pub(crate) login_hint: Option<String>,
}

impl fmt::Debug for AuthorizationAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizationAttempt")
            .field("state", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("pkce_verifier", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("client_id", &self.client_id)
            .field("is_new_registration", &self.is_new_registration)
            .field("host_id", &self.host_id)
            .field("expected_subject", &self.expected_subject)
            .field(
                "id_token_hint",
                &self.id_token_hint.as_ref().map(|_| "[REDACTED]"),
            )
            .field("login_hint", &self.login_hint)
            .finish()
    }
}

pub struct AuthorizationStart {
    pub attempt: AuthorizationAttempt,
    pub authorization_url: Url,
}

impl fmt::Debug for AuthorizationStart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthorizationStart")
            .field("attempt", &self.attempt)
            .field(
                "authorization_url",
                &"[REDACTED: may contain id_token_hint]",
            )
            .finish()
    }
}

impl AuthorizationAttempt {
    pub(crate) fn create(
        options: AuthorizationOptions<'_>,
        endpoints: &OAuthClientMetadata,
    ) -> Result<AuthorizationStart> {
        let AuthorizationOptions {
            redirect_uri,
            issued_client_id,
            host_id,
            agent_name,
            expected_subject,
            id_token_hint,
            login_hint,
        } = options;
        validate_redirect_uri(redirect_uri)?;
        if agent_name.trim().is_empty()
            || issued_client_id.is_some_and(|client_id| {
                client_id.trim().is_empty() || client_id == DYNAMIC_CLIENT_ID
            })
        {
            return Err(ProviderError::InvalidConfiguration);
        }
        let state = Uuid::new_v4().simple().to_string();
        let nonce = Uuid::new_v4().simple().to_string();
        let verifier_seed = format!("{}{}", Uuid::new_v4(), Uuid::new_v4());
        let pkce_verifier = URL_SAFE_NO_PAD.encode(verifier_seed.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce_verifier.as_bytes()));
        let is_new_registration = issued_client_id.is_none();
        let client_id = issued_client_id.unwrap_or(DYNAMIC_CLIENT_ID).to_owned();

        let mut authorization_url = endpoints.authorization_endpoint.clone();
        {
            let mut query = authorization_url.query_pairs_mut();
            query
                .append_pair("client_id", &client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", redirect_uri)
                .append_pair(
                    "scope",
                    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct",
                )
                .append_pair("resource", OPENAI_RESOURCE)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge_method", "S256")
                .append_pair("code_challenge", &challenge)
                .append_pair("ext_agent_host_id", &host_id);
            if is_new_registration {
                query.append_pair("agent_name_hint", agent_name);
            }
            if let Some(id_token_hint) = id_token_hint.as_deref() {
                query.append_pair("id_token_hint", id_token_hint);
            }
            if let Some(login_hint) = login_hint.as_deref() {
                query.append_pair("login_hint", login_hint);
            }
        }

        Ok(AuthorizationStart {
            attempt: Self {
                state,
                nonce,
                pkce_verifier,
                redirect_uri: redirect_uri.to_owned(),
                client_id,
                is_new_registration,
                host_id,
                expected_subject,
                id_token_hint,
                login_hint,
            },
            authorization_url,
        })
    }

    pub fn callback_uri(&self) -> &str {
        &self.redirect_uri
    }
}

pub(crate) struct AuthorizationOptions<'a> {
    pub redirect_uri: &'a str,
    pub issued_client_id: Option<&'a str>,
    pub host_id: String,
    pub agent_name: &'a str,
    pub expected_subject: Option<String>,
    pub id_token_hint: Option<String>,
    pub login_hint: Option<String>,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CallbackResult {
    pub code: Option<String>,
    pub state: String,
    pub client_id: Option<String>,
    pub error: Option<String>,
}

impl fmt::Debug for CallbackResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackResult")
            .field("code", &self.code.as_ref().map(|_| "[REDACTED]"))
            .field("state", &"[REDACTED]")
            .field("client_id", &self.client_id)
            .field("error", &self.error)
            .finish()
    }
}

pub struct LoopbackListener {
    listener: TcpListener,
    redirect_uri: String,
}

impl fmt::Debug for LoopbackListener {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoopbackListener")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

impl LoopbackListener {
    pub async fn bind() -> Result<Self> {
        let listener =
            TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)).await?;
        let address = listener.local_addr()?;
        let redirect_uri = format!("http://127.0.0.1:{}{CALLBACK_PATH}", address.port());
        Ok(Self {
            listener,
            redirect_uri,
        })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub async fn wait(self, timeout: Duration) -> Result<CallbackResult> {
        tokio::time::timeout(timeout, async move {
            let (stream, peer) = self.listener.accept().await?;
            if !peer.ip().is_loopback() {
                return Err(ProviderError::InvalidCallback(
                    "callback peer was not loopback",
                ));
            }
            read_callback(stream).await
        })
        .await
        .map_err(|_| ProviderError::CallbackTimeout)?
    }
}

async fn read_callback(mut stream: TcpStream) -> Result<CallbackResult> {
    let mut request = Vec::with_capacity(2048);
    let mut chunk = [0_u8; 1024];
    while request.len() < 16 * 1024 {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let page = b"<!doctype html><title>Signed in</title><p>You can close this window and return to the app.</p>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        page.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(page).await;

    let request = std::str::from_utf8(&request)
        .map_err(|_| ProviderError::InvalidCallback("invalid HTTP request"))?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or(ProviderError::InvalidCallback("missing request target"))?;
    let callback_url = Url::parse(&format!("http://127.0.0.1{target}"))?;
    if callback_url.path() != CALLBACK_PATH {
        return Err(ProviderError::InvalidCallback("unexpected callback path"));
    }
    let mut values = callback_url.query_pairs();
    let mut callback = CallbackResult {
        code: None,
        state: String::new(),
        client_id: None,
        error: None,
    };
    for (key, value) in values.by_ref() {
        match key.as_ref() {
            "code" => callback.code = Some(value.into_owned()),
            "state" => callback.state = value.into_owned(),
            "client_id" => callback.client_id = Some(value.into_owned()),
            "error" => callback.error = Some(value.into_owned()),
            _ => {}
        }
    }
    if callback.state.is_empty() {
        return Err(ProviderError::InvalidCallback("missing state"));
    }
    Ok(callback)
}

pub(crate) fn validate_callback_state(expected: &str, received: &str) -> bool {
    use subtle::ConstantTimeEq;
    expected.as_bytes().ct_eq(received.as_bytes()).into()
}

pub(crate) fn validate_callback_client_id(
    attempt: &AuthorizationAttempt,
    callback: &CallbackResult,
) -> Result<String> {
    if attempt.is_new_registration {
        let issued = callback
            .client_id
            .as_deref()
            .filter(|value| !value.is_empty() && *value != DYNAMIC_CLIENT_ID)
            .ok_or(ProviderError::InvalidCallback(
                "new registration did not return an issued client ID",
            ))?;
        Ok(issued.to_owned())
    } else {
        if callback
            .client_id
            .as_deref()
            .is_some_and(|value| value != attempt.client_id)
        {
            return Err(ProviderError::InvalidCallback("callback client ID changed"));
        }
        Ok(attempt.client_id.clone())
    }
}

pub(crate) fn build_profile(
    attempt: &AuthorizationAttempt,
    client_id: String,
    issuer: String,
    subject: String,
    email: Option<String>,
    display_name: Option<String>,
    token_set: TokenSet,
) -> Result<ChatGptProfile> {
    if attempt
        .expected_subject
        .as_deref()
        .is_some_and(|expected| expected != subject)
    {
        return Err(ProviderError::InvalidIdentityToken);
    }
    let id = crate::ProfileId::from_client_id(client_id.clone());
    Ok(ChatGptProfile {
        id,
        client_id,
        host_id: attempt.host_id.clone(),
        issuer,
        subject,
        email,
        display_name,
        tokens: Some(token_set),
    })
}

fn validate_redirect_uri(value: &str) -> Result<()> {
    let url = Url::parse(value)?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != CALLBACK_PATH
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderError::InvalidConfiguration);
    }
    Ok(())
}

fn parse_trusted_endpoint(value: &str) -> Result<Url> {
    let url = Url::parse(value)?;
    if url.scheme() != "https"
        || url.host_str() != Some("auth.openai.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderError::InvalidConfiguration);
    }
    Ok(url)
}

pub(crate) async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let body = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_default();
    let code = body
        .get("error")
        .and_then(|error| {
            error
                .as_str()
                .or_else(|| error.get("code").and_then(serde_json::Value::as_str))
        })
        .unwrap_or("provider_error")
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
        })
        .take(80)
        .collect::<String>();
    Err(ProviderError::ProviderResponse { status, code })
}

pub(crate) fn expiry_from(expires_in: i64, now: i64) -> Result<i64> {
    if expires_in <= 0 || expires_in > 366 * 24 * 60 * 60 {
        return Err(ProviderError::InvalidExpiry);
    }
    now.checked_add(expires_in)
        .ok_or(ProviderError::InvalidExpiry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn endpoints() -> OAuthClientMetadata {
        OAuthClientMetadata {
            issuer: Url::parse(ISSUER).unwrap(),
            authorization_endpoint: Url::parse("https://auth.openai.com/api/accounts/authorize")
                .unwrap(),
            token_endpoint: Url::parse("https://auth.openai.com/api/accounts/oauth/token").unwrap(),
            jwks_uri: Url::parse("https://auth.openai.com/.well-known/jwks.json").unwrap(),
            revocation_endpoint: Url::parse("https://auth.openai.com/api/accounts/oauth/revoke")
                .unwrap(),
        }
    }

    #[test]
    fn new_registration_uses_pkce_plan_scopes_and_real_agent_name() {
        let start = AuthorizationAttempt::create(
            AuthorizationOptions {
                redirect_uri: "http://127.0.0.1:1455/auth/callback",
                issued_client_id: None,
                host_id: "urn:uuid:stable-host".to_owned(),
                agent_name: "NedoBot Local Agent",
                expected_subject: None,
                id_token_hint: None,
                login_hint: None,
            },
            &endpoints(),
        )
        .unwrap();
        let query = start.authorization_url.query_pairs().collect::<Vec<_>>();
        let value = |key: &str| {
            query
                .iter()
                .find(|(name, _)| name == key)
                .unwrap()
                .1
                .as_ref()
        };

        assert_eq!(value("client_id"), DYNAMIC_CLIENT_ID);
        assert_eq!(value("agent_name_hint"), "NedoBot Local Agent");
        assert_eq!(value("ext_agent_host_id"), "urn:uuid:stable-host");
        assert!(value("scope").contains("chatgpt.tokens.use.direct"));
        assert_eq!(value("code_challenge_method"), "S256");
        assert!(!start.authorization_url.as_str().contains("pkce_verifier"));
    }

    #[test]
    fn returning_authorization_reuses_client_id_and_rejects_callback_swaps() {
        let start = AuthorizationAttempt::create(
            AuthorizationOptions {
                redirect_uri: "http://127.0.0.1:1455/auth/callback",
                issued_client_id: Some("oaiapp_saved"),
                host_id: "urn:uuid:stable-host".to_owned(),
                agent_name: "ignored on reconnect",
                expected_subject: Some("subject-1".to_owned()),
                id_token_hint: Some("id-token".to_owned()),
                login_hint: Some("person@example.com".to_owned()),
            },
            &endpoints(),
        )
        .unwrap();
        let mut callback = CallbackResult {
            code: Some("code".to_owned()),
            state: start.attempt.state.clone(),
            client_id: Some("oaiapp_other".to_owned()),
            error: None,
        };

        assert_eq!(
            validate_callback_client_id(&start.attempt, &callback)
                .unwrap_err()
                .to_string(),
            "OAuth callback was rejected: callback client ID changed"
        );
        callback.client_id = None;
        assert_eq!(
            validate_callback_client_id(&start.attempt, &callback).unwrap(),
            "oaiapp_saved"
        );
        assert!(!start.authorization_url.as_str().contains("agent_name_hint"));
        assert!(start.authorization_url.as_str().contains("id_token_hint"));
    }

    #[test]
    fn only_loopback_callback_path_is_accepted() {
        assert!(validate_redirect_uri("http://127.0.0.1:1000/auth/callback").is_ok());
        assert!(validate_redirect_uri("http://localhost:1000/auth/callback").is_err());
        assert!(validate_redirect_uri("http://user@127.0.0.1:1000/auth/callback").is_err());
        assert!(validate_redirect_uri("http://127.0.0.1:1000/callback").is_err());
        assert!(validate_redirect_uri("https://127.0.0.1:1000/auth/callback").is_err());
    }

    #[test]
    fn metadata_endpoints_must_remain_on_the_trusted_openai_origin() {
        assert!(parse_trusted_endpoint("https://auth.openai.com/oauth/token").is_ok());
        assert!(parse_trusted_endpoint("https://auth.openai.com:444/oauth/token").is_err());
        assert!(parse_trusted_endpoint("https://auth.openai.com.evil.test/token").is_err());
        assert!(parse_trusted_endpoint("https://user@auth.openai.com/token").is_err());
    }

    #[tokio::test]
    async fn loopback_listener_reads_oauth_values_and_returns_a_static_page() {
        let listener = LoopbackListener::bind().await.unwrap();
        let port = Url::parse(listener.redirect_uri()).unwrap().port().unwrap();
        let task = tokio::spawn(listener.wait(Duration::from_secs(3)));
        let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        socket
            .write_all(b"GET /auth/callback?code=code-1&state=state-1&client_id=oaiapp_1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .unwrap();
        let callback = task.await.unwrap().unwrap();

        assert_eq!(callback.code.as_deref(), Some("code-1"));
        assert_eq!(callback.state, "state-1");
        assert_eq!(callback.client_id.as_deref(), Some("oaiapp_1"));
    }
}
