use thiserror::Error;

pub type Result<T, E = ProviderError> = std::result::Result<T, E>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponsesFailure {
    pub status: u16,
    pub code: Option<String>,
    pub param: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("HTTP request to provider failed")]
    Http(#[from] reqwest::Error),
    #[error("provider returned HTTP {status} ({code})")]
    ProviderResponse { status: u16, code: String },
    #[error("provider response was invalid")]
    InvalidResponse,
    #[error("OAuth callback was rejected: {0}")]
    InvalidCallback(&'static str),
    #[error("OAuth authorization was declined")]
    AuthorizationDenied,
    #[error("the selected ChatGPT profile must sign in again")]
    ReauthenticationRequired,
    #[error("ChatGPT plan usage permission was not granted")]
    PlanUsageNotGranted,
    #[error("selected ChatGPT profile was not found")]
    ProfileNotFound,
    #[error("profile storage failed")]
    Storage(#[from] std::io::Error),
    #[error("OAuth token validation failed")]
    InvalidIdentityToken,
    #[error("OAuth provider configuration is invalid")]
    InvalidConfiguration,
    #[error("loopback OAuth callback timed out")]
    CallbackTimeout,
    #[error("requested model requirements have no match")]
    NoMatchingModel,
    #[error("provider returned an unsupported authentication error: {0}")]
    OAuthError(String),
    #[error("JSON encoding or decoding failed")]
    Json(#[from] serde_json::Error),
    #[error("URL parsing failed")]
    Url(#[from] url::ParseError),
    #[error("token expiry is outside the supported range")]
    InvalidExpiry,
    #[error(
        "ChatGPT Responses request failed (HTTP {}, code {})",
        .0.status,
        .0.code.as_deref().unwrap_or("unknown")
    )]
    ResponsesRejected(ResponsesFailure),
    #[error("ChatGPT Responses stream ended before a completed response")]
    IncompleteResponse,
    #[error("provider request exceeded its configured timeout")]
    Timeout,
    #[cfg(feature = "genai")]
    #[error("genai request failed")]
    GenAi,
}

pub(crate) fn reqwest_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() {
        ProviderError::Timeout
    } else {
        ProviderError::Http(error)
    }
}

#[cfg(feature = "genai")]
impl From<genai::Error> for ProviderError {
    fn from(_: genai::Error) -> Self {
        // Ошибка genai может содержать тело ответа провайдера в Debug и Display.
        Self::GenAi
    }
}
