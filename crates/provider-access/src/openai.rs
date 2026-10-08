use crate::oauth::{
    AuthorizationAttempt, AuthorizationOptions, AuthorizationStart, CallbackResult,
    OAuthClientMetadata, build_profile, ensure_success, expiry_from, validate_callback_client_id,
    validate_callback_state,
};
use crate::{
    ChatGptProfile, CredentialStore, ModelDescriptor, ModelProvider, Pricing, ProviderError,
    ResponsesFailure, ResponsesRequest, ResponsesStream, Result, TokenSet,
};
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use url::Url;

const OPENAI_RESOURCE: &str = "https://api.openai.com/v1";
const OPENAI_MODELS_URL: &str = "https://api.openai.com/v1/models";
const USAGE_SETTINGS_URL: &str = "https://chatgpt.com/settings/usage";
const OPENAI_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_RESPONSES_TIMEOUT_SECS: u64 = 600;
const DEFAULT_REFRESH_SKEW_SECS: i64 = 180;

#[derive(Clone, Debug)]
pub struct OpenAiSiwcConfig {
    pub agent_name: String,
    pub refresh_skew: Duration,
    pub responses_timeout: Duration,
}

impl OpenAiSiwcConfig {
    pub fn new(agent_name: impl Into<String>) -> Self {
        Self {
            agent_name: agent_name.into(),
            refresh_skew: Duration::from_secs(DEFAULT_REFRESH_SKEW_SECS as u64),
            responses_timeout: Duration::from_secs(DEFAULT_RESPONSES_TIMEOUT_SECS),
        }
    }

    pub fn with_responses_timeout(mut self, timeout: Duration) -> Self {
        self.responses_timeout = timeout;
        self
    }
}

#[derive(Clone)]
pub struct OpenAiSiwc {
    config: OpenAiSiwcConfig,
    http: reqwest::Client,
    store: Arc<dyn CredentialStore>,
}

impl OpenAiSiwc {
    pub fn new(config: OpenAiSiwcConfig, store: Arc<dyn CredentialStore>) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
            store,
        }
    }

    pub fn with_http_client(
        config: OpenAiSiwcConfig,
        store: Arc<dyn CredentialStore>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            config,
            http,
            store,
        }
    }

    pub async fn start_sign_in(
        &self,
        redirect_uri: &str,
        existing: Option<&ChatGptProfile>,
    ) -> Result<AuthorizationStart> {
        let host_id = self.store.host_id_or_create().await?;
        if !is_supported_host_id(&host_id) {
            return Err(ProviderError::InvalidConfiguration);
        }
        if existing.is_some_and(|profile| profile.issuer != "https://auth.openai.com") {
            return Err(ProviderError::InvalidConfiguration);
        }
        if existing.is_some_and(|profile| profile.host_id != host_id) {
            return Err(ProviderError::InvalidConfiguration);
        }
        let endpoints = OAuthClientMetadata::discover(&self.http).await?;
        let attempt = AuthorizationAttempt::create(
            AuthorizationOptions {
                redirect_uri,
                issued_client_id: existing.map(|profile| profile.client_id.as_str()),
                host_id,
                agent_name: &self.config.agent_name,
                expected_subject: existing.map(|profile| profile.subject.clone()),
                id_token_hint: existing
                    .and_then(|profile| profile.reconnect_id_token_hint().map(str::to_owned)),
                login_hint: existing.and_then(|profile| profile.email.clone()),
            },
            &endpoints,
        )?;
        Ok(attempt)
    }

    pub async fn complete_sign_in(
        &self,
        start: AuthorizationStart,
        callback: CallbackResult,
    ) -> Result<ChatGptProfile> {
        let attempt = start.attempt;
        if !validate_callback_state(&attempt.state, &callback.state) {
            return Err(ProviderError::InvalidCallback("state did not match"));
        }
        if let Some(error) = callback.error.as_deref() {
            return if error == "access_denied" {
                Err(ProviderError::AuthorizationDenied)
            } else {
                Err(ProviderError::OAuthError(safe_error_code(error)))
            };
        }
        let code = callback
            .code
            .as_deref()
            .filter(|code| !code.is_empty())
            .ok_or(ProviderError::InvalidCallback(
                "authorization code was missing",
            ))?;
        let client_id = validate_callback_client_id(&attempt, &callback)?;
        let endpoints = OAuthClientMetadata::discover(&self.http).await?;
        let token_response = self
            .exchange_authorization_code(&endpoints, &attempt, code, &client_id)
            .await?;
        let verified = self
            .verify_identity_token(
                &endpoints,
                token_response.id_token.as_deref(),
                &client_id,
                &attempt.nonce,
            )
            .await?;
        let token_set = token_response.into_token_set()?;
        if !token_set.plan_usage_granted() {
            return Err(ProviderError::PlanUsageNotGranted);
        }
        let profile = build_profile(
            &attempt,
            client_id,
            endpoints.issuer.as_str().trim_end_matches('/').to_owned(),
            verified.subject,
            verified.email,
            verified.display_name,
            token_set,
        )?;
        self.store.save_profile(&profile).await?;
        Ok(profile)
    }

    pub async fn list_models(&self, profile_id: &crate::ProfileId) -> Result<Vec<OpenAiModel>> {
        let access_token = self.access_token(profile_id).await?;
        let response = self
            .http
            .get(OPENAI_MODELS_URL)
            .bearer_auth(access_token)
            .timeout(OPENAI_REQUEST_TIMEOUT)
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let document: OpenAiModelList = response.json().await?;
        Ok(document
            .models
            .into_iter()
            .filter(|model| model.visibility.as_deref() == Some("list"))
            .map(OpenAiModel::from)
            .collect())
    }

    pub async fn stream_responses(
        &self,
        profile_id: &crate::ProfileId,
        request: ResponsesRequest,
    ) -> Result<ResponsesStream> {
        if self.config.responses_timeout.is_zero() {
            return Err(ProviderError::InvalidConfiguration);
        }
        let payload = request.payload()?;
        let access_token = self.access_token(profile_id).await?;
        let response = self
            .http
            .post("https://api.openai.com/v1/responses")
            .bearer_auth(access_token)
            .json(&payload)
            .timeout(self.config.responses_timeout)
            .send()
            .await
            .map_err(crate::error::reqwest_error)?;
        if !response.status().is_success() {
            let failure = self.failure_from_response(response).await;
            return Err(ProviderError::ResponsesRejected(failure));
        }
        ResponsesStream::from_response(response)
    }

    pub async fn access_token(&self, profile_id: &crate::ProfileId) -> Result<String> {
        let _refresh_lock = self.store.lock_profile(profile_id).await?;
        let mut profile = self
            .store
            .profile(profile_id)
            .await?
            .ok_or(ProviderError::ProfileNotFound)?;
        let Some(tokens) = profile.tokens.as_ref() else {
            return Err(ProviderError::ReauthenticationRequired);
        };
        if !tokens.plan_usage_granted() {
            return Err(ProviderError::PlanUsageNotGranted);
        }
        let now = unix_now();
        let refresh_at = tokens
            .expires_at_unix
            .saturating_sub(i64::try_from(self.config.refresh_skew.as_secs()).unwrap_or(i64::MAX));
        if now < refresh_at {
            return Ok(tokens.access_token.clone());
        }
        let endpoints = OAuthClientMetadata::discover(&self.http).await?;
        let refreshed = self.refresh_token(&endpoints, &profile).await;
        let token_set = match refreshed {
            Ok(tokens) => tokens,
            Err(ProviderError::ReauthenticationRequired) => {
                profile.tokens = None;
                self.store.save_profile(&profile).await?;
                return Err(ProviderError::ReauthenticationRequired);
            }
            Err(error) => return Err(error),
        };
        if !token_set.plan_usage_granted() {
            profile.tokens = None;
            self.store.save_profile(&profile).await?;
            return Err(ProviderError::PlanUsageNotGranted);
        }
        profile.tokens = Some(token_set);
        self.store.save_profile(&profile).await?;
        profile
            .tokens
            .map(|tokens| tokens.access_token)
            .ok_or(ProviderError::ReauthenticationRequired)
    }

    pub async fn revoke_and_sign_out(&self, profile_id: &crate::ProfileId) -> Result<()> {
        let _refresh_lock = self.store.lock_profile(profile_id).await?;
        let mut profile = self
            .store
            .profile(profile_id)
            .await?
            .ok_or(ProviderError::ProfileNotFound)?;
        if let Some(tokens) = profile.tokens.as_ref() {
            let endpoints = OAuthClientMetadata::discover(&self.http).await?;
            let response = self
                .http
                .post(endpoints.revocation_endpoint)
                .form(&[
                    ("token", tokens.refresh_token()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", profile.client_id.as_str()),
                ])
                .timeout(OPENAI_REQUEST_TIMEOUT)
                .send()
                .await?;
            ensure_success(response).await?;
        }
        profile.tokens = None;
        self.store.save_profile(&profile).await
    }

    pub async fn failure_from_response(&self, response: reqwest::Response) -> ResponsesFailure {
        let status = response.status().as_u16();
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response.json::<Value>().await.unwrap_or_default();
        let error = body.get("error").unwrap_or(&body);
        ResponsesFailure {
            status,
            code: error
                .get("code")
                .and_then(Value::as_str)
                .map(safe_error_code),
            param: error
                .get("param")
                .and_then(Value::as_str)
                .map(safe_error_code),
            request_id: request_id.map(|value| safe_error_code(&value)),
        }
    }

    async fn exchange_authorization_code(
        &self,
        endpoints: &OAuthClientMetadata,
        attempt: &AuthorizationAttempt,
        code: &str,
        client_id: &str,
    ) -> Result<TokenResponse> {
        let response = self
            .http
            .post(endpoints.token_endpoint.clone())
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("code_verifier", attempt.pkce_verifier.as_str()),
                ("redirect_uri", attempt.redirect_uri.as_str()),
                ("resource", OPENAI_RESOURCE),
            ])
            .timeout(OPENAI_REQUEST_TIMEOUT)
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let response: TokenResponse = response.json().await?;
        response.validate_bearer()?;
        Ok(response)
    }

    async fn refresh_token(
        &self,
        endpoints: &OAuthClientMetadata,
        profile: &ChatGptProfile,
    ) -> Result<TokenSet> {
        let tokens = profile
            .tokens
            .as_ref()
            .ok_or(ProviderError::ReauthenticationRequired)?;
        let response = self
            .http
            .post(endpoints.token_endpoint.clone())
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", profile.client_id.as_str()),
                ("refresh_token", tokens.refresh_token()),
                ("resource", OPENAI_RESOURCE),
            ])
            .timeout(OPENAI_REQUEST_TIMEOUT)
            .send()
            .await?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.json::<Value>().await.unwrap_or_default();
            let code = oauth_error_code(&body).unwrap_or("provider_error");
            if unusable_refresh_token(code) {
                return Err(ProviderError::ReauthenticationRequired);
            }
            return Err(ProviderError::ProviderResponse {
                status,
                code: safe_error_code(code),
            });
        }
        let response: TokenResponse = response.json().await?;
        response.validate_bearer()?;
        if response.refresh_token.is_none() {
            return Err(ProviderError::InvalidResponse);
        }
        response.into_refreshed_token_set(tokens)
    }

    async fn verify_identity_token(
        &self,
        endpoints: &OAuthClientMetadata,
        id_token: Option<&str>,
        client_id: &str,
        expected_nonce: &str,
    ) -> Result<VerifiedIdentity> {
        let id_token = id_token.ok_or(ProviderError::InvalidIdentityToken)?;
        let header = decode_header(id_token).map_err(|_| ProviderError::InvalidIdentityToken)?;
        if !is_supported_identity_algorithm(header.alg) {
            return Err(ProviderError::InvalidIdentityToken);
        }
        let kid = header
            .kid
            .as_deref()
            .ok_or(ProviderError::InvalidIdentityToken)?;
        let response = self
            .http
            .get(endpoints.jwks_uri.clone())
            .timeout(OPENAI_REQUEST_TIMEOUT)
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let keys: JwkSet = response
            .json()
            .await
            .map_err(|_| ProviderError::InvalidIdentityToken)?;
        let jwk = keys.find(kid).ok_or(ProviderError::InvalidIdentityToken)?;
        let key = DecodingKey::from_jwk(jwk).map_err(|_| ProviderError::InvalidIdentityToken)?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[endpoints.issuer.as_str()]);
        validation.set_audience(&[client_id]);
        validation.set_required_spec_claims(&["iss", "sub", "aud", "exp"]);
        validation.validate_nbf = true;
        let token = decode::<IdentityClaims>(id_token, &key, &validation)
            .map_err(|_| ProviderError::InvalidIdentityToken)?;
        if token.claims.nonce.as_deref() != Some(expected_nonce)
            || token.claims.sub.is_empty()
            || !token.claims.valid_audience(client_id)
        {
            return Err(ProviderError::InvalidIdentityToken);
        }
        Ok(VerifiedIdentity {
            subject: token.claims.sub,
            email: token.claims.email,
            display_name: token.claims.name,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct OpenAiModel {
    pub slug: String,
    pub display_name: String,
    pub description: Option<String>,
    pub context_length: Option<u64>,
    pub supports_images: bool,
    pub supports_tools: bool,
}

impl OpenAiModel {
    pub fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            id: self.slug.clone(),
            display_name: self.display_name.clone(),
            provider: ModelProvider::OpenAiChatGpt,
            context_length: self.context_length,
            supports_text: true,
            supports_images: self.supports_images,
            supports_tools: self.supports_tools,
            pricing: Pricing::default(),
            is_router: false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiModelList {
    #[serde(default)]
    models: Vec<OpenAiModelRaw>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelRaw {
    slug: String,
    display_name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    input_modalities: Vec<String>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

impl From<OpenAiModelRaw> for OpenAiModel {
    fn from(raw: OpenAiModelRaw) -> Self {
        Self {
            slug: raw.slug,
            display_name: raw.display_name,
            description: raw.description,
            context_length: raw.context_length,
            supports_images: raw
                .input_modalities
                .iter()
                .any(|modality| modality == "image"),
            supports_tools: raw
                .supported_parameters
                .iter()
                .any(|parameter| parameter == "tools"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatGptPlanStatus {
    Ready,
    /// Достигнут лимит плана либо лимит этого приложения; API не сообщает, какой именно.
    UsageLimitReached,
    TemporarilyUnavailable,
    NotEligible,
    UnsupportedCapability,
    PermissionDenied,
    ReauthenticationRequired,
    RateLimited,
    UnknownFailure,
}

impl ChatGptPlanStatus {
    pub fn usage_settings_url(self) -> Option<Url> {
        matches!(self, Self::UsageLimitReached)
            .then(|| Url::parse(USAGE_SETTINGS_URL).expect("constant URL"))
    }
}

pub fn classify_responses_failure(failure: &ResponsesFailure) -> ChatGptPlanStatus {
    match failure.code.as_deref() {
        Some("subscription_sharing_usage_limit_exceeded") => ChatGptPlanStatus::UsageLimitReached,
        Some(
            "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable",
        ) => ChatGptPlanStatus::TemporarilyUnavailable,
        Some("subscription_sharing_user_not_eligible") => ChatGptPlanStatus::NotEligible,
        Some("subscription_sharing_unsupported_capability") => {
            ChatGptPlanStatus::UnsupportedCapability
        }
        Some("subscription_sharing_invalid_user") => ChatGptPlanStatus::ReauthenticationRequired,
        Some("invalid_token" | "access_token_expired") => {
            ChatGptPlanStatus::ReauthenticationRequired
        }
        Some("chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context") => {
            ChatGptPlanStatus::PermissionDenied
        }
        _ if failure.status == 429 => ChatGptPlanStatus::RateLimited,
        _ if failure.status == 401 => ChatGptPlanStatus::ReauthenticationRequired,
        _ => ChatGptPlanStatus::UnknownFailure,
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    token_type: String,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    earliest_refresh_at: Option<Value>,
}

impl TokenResponse {
    fn validate_bearer(&self) -> Result<()> {
        if self.access_token.is_empty() || !self.token_type.eq_ignore_ascii_case("bearer") {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(())
    }

    fn into_token_set(self) -> Result<TokenSet> {
        let refresh_token = self
            .refresh_token
            .filter(|token| !token.is_empty())
            .ok_or(ProviderError::InvalidResponse)?;
        let now = unix_now();
        Ok(TokenSet {
            access_token: self.access_token,
            refresh_token,
            id_token: self.id_token,
            token_type: self.token_type,
            scopes: parse_scopes(self.scope),
            expires_at_unix: expiry_from(self.expires_in, now)?,
            earliest_refresh_at_unix: parse_epoch(self.earliest_refresh_at),
        })
    }

    fn into_refreshed_token_set(self, previous: &TokenSet) -> Result<TokenSet> {
        let now = unix_now();
        Ok(TokenSet {
            access_token: self.access_token,
            refresh_token: self
                .refresh_token
                .filter(|token| !token.is_empty())
                .ok_or(ProviderError::InvalidResponse)?,
            id_token: self.id_token.or_else(|| previous.id_token.clone()),
            token_type: self.token_type,
            scopes: self
                .scope
                .map(parse_scope_string)
                .unwrap_or_else(|| previous.scopes.clone()),
            expires_at_unix: expiry_from(self.expires_in, now)?,
            earliest_refresh_at_unix: parse_epoch(self.earliest_refresh_at),
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct IdentityClaims {
    sub: String,
    aud: Audience,
    azp: Option<String>,
    nonce: Option<String>,
    email: Option<String>,
    name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum Audience {
    Single(String),
    Multiple(Vec<String>),
}

impl IdentityClaims {
    fn valid_audience(&self, client_id: &str) -> bool {
        match &self.aud {
            Audience::Single(audience) => {
                audience == client_id && self.azp.as_deref().is_none_or(|azp| azp == client_id)
            }
            Audience::Multiple(audiences) => {
                audiences.iter().any(|audience| audience == client_id)
                    && self.azp.as_deref() == Some(client_id)
            }
        }
    }
}

#[derive(Debug)]
struct VerifiedIdentity {
    subject: String,
    email: Option<String>,
    display_name: Option<String>,
}

fn is_supported_host_id(value: &str) -> bool {
    value.starts_with("urn:uuid:")
        && uuid::Uuid::parse_str(value.trim_start_matches("urn:uuid:")).is_ok()
}

fn is_supported_identity_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::EdDSA
    )
}

fn parse_scopes(scopes: Option<String>) -> Vec<String> {
    scopes.map(parse_scope_string).unwrap_or_default()
}

fn parse_scope_string(scopes: String) -> Vec<String> {
    scopes.split_whitespace().map(str::to_owned).collect()
}

fn parse_epoch(value: Option<Value>) -> Option<i64> {
    match value? {
        Value::Number(value) => value.as_i64(),
        Value::String(value) => value.parse::<i64>().ok().or_else(|| {
            value
                .parse::<jiff::Timestamp>()
                .ok()
                .map(|timestamp| timestamp.as_second())
        }),
        _ => None,
    }
}

fn unusable_refresh_token(code: &str) -> bool {
    matches!(
        code,
        "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
    )
}

fn oauth_error_code(body: &Value) -> Option<&str> {
    body.get("error")
        .and_then(|error| {
            error
                .as_str()
                .or_else(|| error.get("code").and_then(Value::as_str))
        })
        .or_else(|| body.get("code").and_then(Value::as_str))
}

fn safe_error_code(error: &str) -> String {
    error
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
        })
        .take(80)
        .collect()
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_capability_is_conservative_when_catalog_omits_metadata() {
        let model = OpenAiModel::from(OpenAiModelRaw {
            slug: "model-a".to_owned(),
            display_name: "Model A".to_owned(),
            description: None,
            visibility: Some("list".to_owned()),
            context_length: Some(32_000),
            input_modalities: vec!["text".to_owned()],
            supported_parameters: Vec::new(),
        });
        assert!(!model.supports_images);
        assert!(!model.supports_tools);
        assert_eq!(model.descriptor().context_length, Some(32_000));
    }

    #[test]
    fn identity_audience_requires_authorized_party_for_multiple_audiences() {
        let one = IdentityClaims {
            sub: "subject".to_owned(),
            aud: Audience::Single("client-1".to_owned()),
            azp: None,
            nonce: None,
            email: None,
            name: None,
        };
        let many_without_azp = IdentityClaims {
            aud: Audience::Multiple(vec!["other".to_owned(), "client-1".to_owned()]),
            ..one.clone()
        };
        let many_with_azp = IdentityClaims {
            azp: Some("client-1".to_owned()),
            ..many_without_azp.clone()
        };

        assert!(one.valid_audience("client-1"));
        assert!(!one.valid_audience("client-2"));
        assert!(!many_without_azp.valid_audience("client-1"));
        assert!(many_with_azp.valid_audience("client-1"));
    }

    #[test]
    fn identity_signature_algorithm_must_be_asymmetric() {
        assert!(is_supported_identity_algorithm(Algorithm::RS256));
        assert!(is_supported_identity_algorithm(Algorithm::EdDSA));
        assert!(!is_supported_identity_algorithm(Algorithm::HS256));
    }

    #[test]
    fn usage_limit_is_not_given_an_inferred_reset_time() {
        let failure = ResponsesFailure {
            status: 429,
            code: Some("subscription_sharing_usage_limit_exceeded".to_owned()),
            param: None,
            request_id: Some("request-1".to_owned()),
        };
        assert_eq!(
            classify_responses_failure(&failure),
            ChatGptPlanStatus::UsageLimitReached
        );
        assert_eq!(
            classify_responses_failure(&failure)
                .usage_settings_url()
                .unwrap()
                .as_str(),
            USAGE_SETTINGS_URL
        );
    }

    #[test]
    fn stale_or_ineligible_requests_are_not_misreported_as_quota_exhaustion() {
        let failure = ResponsesFailure {
            status: 403,
            code: Some("subscription_sharing_user_not_eligible".to_owned()),
            param: None,
            request_id: None,
        };
        assert_eq!(
            classify_responses_failure(&failure),
            ChatGptPlanStatus::NotEligible
        );
    }

    #[test]
    fn recognizes_oauth_error_codes_in_string_and_object_shapes() {
        assert_eq!(
            oauth_error_code(&serde_json::json!({"error": "invalid_grant"})),
            Some("invalid_grant")
        );
        assert_eq!(
            oauth_error_code(&serde_json::json!({"error": {"code": "invalid_grant"}})),
            Some("invalid_grant")
        );
    }

    #[test]
    fn successful_refresh_replaces_rotating_token_and_preserves_omitted_scope() {
        let previous = TokenSet {
            access_token: "old-access".to_owned(),
            refresh_token: "old-refresh".to_owned(),
            id_token: Some("old-id".to_owned()),
            token_type: "Bearer".to_owned(),
            scopes: vec![
                "resource.invoke".to_owned(),
                "chatgpt.tokens.use.direct".to_owned(),
            ],
            expires_at_unix: 1,
            earliest_refresh_at_unix: None,
        };
        let refreshed = TokenResponse {
            access_token: "new-access".to_owned(),
            refresh_token: Some("new-refresh".to_owned()),
            id_token: None,
            token_type: "Bearer".to_owned(),
            expires_in: 3600,
            scope: None,
            earliest_refresh_at: None,
        }
        .into_refreshed_token_set(&previous)
        .unwrap();

        assert_eq!(refreshed.access_token, "new-access");
        assert_eq!(refreshed.refresh_token, "new-refresh");
        assert_eq!(refreshed.id_token.as_deref(), Some("old-id"));
        assert_eq!(refreshed.scopes, previous.scopes);
        assert!(refreshed.plan_usage_granted());
        assert!(refreshed.expires_at_unix > unix_now());
    }

    #[test]
    fn refresh_without_rotated_token_is_rejected() {
        let previous = TokenSet {
            access_token: "old-access".to_owned(),
            refresh_token: "old-refresh".to_owned(),
            id_token: None,
            token_type: "Bearer".to_owned(),
            scopes: vec![
                "resource.invoke".to_owned(),
                "chatgpt.tokens.use.direct".to_owned(),
            ],
            expires_at_unix: 1,
            earliest_refresh_at_unix: None,
        };
        let response = TokenResponse {
            access_token: "new-access".to_owned(),
            refresh_token: None,
            id_token: None,
            token_type: "Bearer".to_owned(),
            expires_in: 3600,
            scope: None,
            earliest_refresh_at: None,
        };

        assert!(matches!(
            response.into_refreshed_token_set(&previous),
            Err(ProviderError::InvalidResponse)
        ));
    }
}
