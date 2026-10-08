//! Адаптеры провайдеров на `genai` для OpenAI-compatible chat endpoints.

use crate::{OpenRouterFreeProvider, ProviderError, Result};
use genai::adapter::AdapterKind;
use genai::chat::{ChatOptions, ChatRequest, ChatResponse};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};

const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1/";

/// Выполняет chat completion через динамический бесплатный маршрут OpenRouter.
pub async fn chat_openrouter_free(
    http: reqwest::Client,
    provider: &OpenRouterFreeProvider,
    request: ChatRequest,
) -> Result<ChatResponse> {
    provider.validate()?;
    let client = Client::builder().with_reqwest(http).build();
    let target = ServiceTarget {
        endpoint: Endpoint::from_static(OPENROUTER_BASE_URL),
        auth: AuthData::from_single(provider.api_key()),
        model: ModelIden::new(AdapterKind::OpenAI, "openrouter/free"),
    };
    let mut options = ChatOptions::default();
    let headers = provider
        .http_referer()
        .map(|value| ("HTTP-Referer".to_owned(), value.to_owned()))
        .into_iter()
        .chain(
            provider
                .app_name()
                .map(|value| ("X-Title".to_owned(), value.to_owned())),
        )
        .collect::<Vec<_>>();
    if !headers.is_empty() {
        options.extra_headers = Some(genai::Headers::from(headers));
    }
    tokio::time::timeout(
        provider.request_timeout(),
        client.exec_chat(target, request, Some(&options)),
    )
    .await
    .map_err(|_| ProviderError::Timeout)?
    .map_err(map_openrouter_error)
}

fn map_openrouter_error(error: genai::Error) -> ProviderError {
    match error {
        genai::Error::WebModelCall { webc_error, .. }
        | genai::Error::WebAdapterCall { webc_error, .. } => map_web_error(webc_error),
        genai::Error::HttpError { status, body, .. } => provider_response(status.as_u16(), &body),
        _ => ProviderError::GenAi,
    }
}

fn map_web_error(error: genai::webc::Error) -> ProviderError {
    match error {
        genai::webc::Error::ResponseFailedStatus { status, body, .. } => {
            provider_response(status.as_u16(), &body)
        }
        genai::webc::Error::Reqwest(error) if error.is_timeout() => ProviderError::Timeout,
        _ => ProviderError::GenAi,
    }
}

fn provider_response(status: u16, body: &str) -> ProviderError {
    let body = serde_json::from_str::<serde_json::Value>(body).unwrap_or_default();
    let code = body
        .pointer("/error/code")
        .or_else(|| body.pointer("/error/type"))
        .and_then(serde_json::Value::as_str)
        .map(safe_error_code)
        .unwrap_or_else(|| "provider_error".to_owned());
    ProviderError::ProviderResponse { status, code }
}

fn safe_error_code(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
        })
        .take(80)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrouter_error_preserves_code_and_status_but_hides_body() {
        let error = map_web_error(genai::webc::Error::ResponseFailedStatus {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            body: r#"{"error":{"code":"rate_limit_exceeded","message":"private prompt text"}}"#
                .to_owned(),
            headers: Box::default(),
        });

        assert_eq!(
            error.to_string(),
            "provider returned HTTP 429 (rate_limit_exceeded)"
        );
        assert!(!format!("{error:?}").contains("private prompt text"));
    }
}
