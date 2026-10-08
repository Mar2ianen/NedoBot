use crate::{
    ModelDescriptor, ModelProvider, ModelQuery, ModelSelector, Pricing, ProviderError, Result,
};
use serde::Deserialize;
use serde_json::Value;
use std::fmt;
use std::time::Duration;

const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_FREE_ID: &str = "openrouter/free";
const OPENROUTER_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_OPENROUTER_CHAT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct OpenRouterFreeConfig {
    pub api_key: String,
    pub http_referer: Option<String>,
    pub app_name: Option<String>,
    pub request_timeout: Duration,
}

impl OpenRouterFreeConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            http_referer: None,
            app_name: None,
            request_timeout: DEFAULT_OPENROUTER_CHAT_TIMEOUT,
        }
    }

    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }
}

impl fmt::Debug for OpenRouterFreeConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenRouterFreeConfig")
            .field("api_key", &"[REDACTED]")
            .field("http_referer", &self.http_referer)
            .field("app_name", &self.app_name)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

#[derive(Clone)]
pub struct OpenRouterFreeProvider {
    http: reqwest::Client,
    config: OpenRouterFreeConfig,
}

impl fmt::Debug for OpenRouterFreeProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenRouterFreeProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl OpenRouterFreeProvider {
    pub fn new(config: OpenRouterFreeConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    pub fn with_http_client(config: OpenRouterFreeConfig, http: reqwest::Client) -> Self {
        Self { http, config }
    }

    pub fn validate(&self) -> Result<()> {
        if self.config.api_key.is_empty()
            || self
                .config
                .api_key
                .chars()
                .any(|character| character.is_ascii_whitespace() || character.is_ascii_control())
            || self.config.http_referer.as_deref().is_some_and(|value| {
                let Ok(url) = url::Url::parse(value) else {
                    return true;
                };
                !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || value.chars().any(char::is_control)
            })
            || self
                .config
                .app_name
                .as_deref()
                .is_some_and(|value| value.trim().is_empty() || value.chars().any(char::is_control))
            || self.config.request_timeout.is_zero()
        {
            return Err(ProviderError::InvalidConfiguration);
        }
        Ok(())
    }

    /// Возвращает динамический селектор OpenRouter, фильтрующий возможности для каждого запроса.
    pub fn router_model() -> ModelDescriptor {
        ModelDescriptor {
            id: OPENROUTER_FREE_ID.to_owned(),
            display_name: "OpenRouter Free Router".to_owned(),
            provider: ModelProvider::OpenRouter,
            context_length: None,
            supports_text: true,
            supports_images: true,
            supports_tools: true,
            pricing: Pricing {
                input_usd_per_million: Some(0.0),
                output_usd_per_million: Some(0.0),
            },
            is_router: true,
        }
    }

    /// Загружает каталог OpenRouter и оставляет модели с нулевой ценой входа и выхода.
    pub async fn list_free_models(&self) -> Result<Vec<OpenRouterModel>> {
        self.validate()?;
        let response = self
            .http
            .get(format!("{OPENROUTER_API_BASE}/models"))
            .query(&[("max_price", "0")])
            .bearer_auth(&self.config.api_key)
            .timeout(OPENROUTER_REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(crate::error::reqwest_error)?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.json::<Value>().await.unwrap_or_default();
            let code = body
                .pointer("/error/code")
                .or_else(|| body.pointer("/error/type"))
                .and_then(Value::as_str)
                .unwrap_or("provider_error")
                .chars()
                .filter(|character| {
                    character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
                })
                .take(80)
                .collect();
            return Err(ProviderError::ProviderResponse { status, code });
        }
        let document: OpenRouterModelList = response.json().await?;
        let mut models = document
            .data
            .into_iter()
            .filter_map(OpenRouterModel::from_raw)
            .filter(|model| model.is_free())
            .collect::<Vec<_>>();
        models.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(models)
    }

    pub async fn select_free_model(&self, query: &ModelQuery) -> Result<OpenRouterModel> {
        let models = self.list_free_models().await?;
        let descriptors = models
            .iter()
            .map(OpenRouterModel::descriptor)
            .collect::<Vec<_>>();
        let selected = ModelSelector::select(
            &descriptors,
            &ModelQuery {
                free_only: true,
                ..query.clone()
            },
        )
        .ok_or(ProviderError::NoMatchingModel)?;
        models
            .into_iter()
            .find(|model| model.id == selected.model.id)
            .ok_or(ProviderError::NoMatchingModel)
    }

    pub fn api_base(&self) -> &'static str {
        OPENROUTER_API_BASE
    }

    pub fn api_key(&self) -> &str {
        &self.config.api_key
    }

    pub fn request_timeout(&self) -> Duration {
        self.config.request_timeout
    }

    pub fn http_referer(&self) -> Option<&str> {
        self.config.http_referer.as_deref()
    }

    pub fn app_name(&self) -> Option<&str> {
        self.config.app_name.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct OpenRouterModel {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    pub input_modalities: Vec<String>,
    pub supports_tools: bool,
    pub pricing: Pricing,
}

impl OpenRouterModel {
    pub fn is_free(&self) -> bool {
        self.pricing.is_free()
    }

    pub fn descriptor(&self) -> ModelDescriptor {
        ModelDescriptor {
            id: self.id.clone(),
            display_name: self.name.clone(),
            provider: ModelProvider::OpenRouter,
            context_length: self.context_length,
            supports_text: self
                .input_modalities
                .iter()
                .any(|modality| modality == "text"),
            supports_images: self
                .input_modalities
                .iter()
                .any(|modality| modality == "image"),
            supports_tools: self.supports_tools,
            pricing: self.pricing.clone(),
            is_router: false,
        }
    }

    fn from_raw(raw: OpenRouterModelRaw) -> Option<Self> {
        let price = Pricing {
            input_usd_per_million: parse_price(raw.pricing.prompt.as_ref()),
            output_usd_per_million: parse_price(raw.pricing.completion.as_ref()),
        };
        let input_modalities = raw
            .architecture
            .input_modalities
            .into_iter()
            .collect::<Vec<_>>();
        let supports_tools = raw
            .supported_parameters
            .iter()
            .any(|value| value == "tools");
        Some(Self {
            id: raw.id,
            name: raw.name,
            context_length: raw.context_length,
            input_modalities,
            supports_tools,
            pricing: price,
        })
    }
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelList {
    #[serde(default)]
    data: Vec<OpenRouterModelRaw>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterModelRaw {
    id: String,
    name: String,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    architecture: OpenRouterArchitecture,
    #[serde(default)]
    pricing: OpenRouterPrice,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenRouterPrice {
    #[serde(default)]
    prompt: Option<Value>,
    #[serde(default)]
    completion: Option<Value>,
}

fn parse_price(value: Option<&Value>) -> Option<f64> {
    let parsed = match value? {
        Value::String(value) => value.parse::<f64>().ok()?,
        Value::Number(value) => value.as_f64()?,
        _ => return None,
    };
    if !parsed.is_finite() || parsed < 0.0 {
        return None;
    }
    let per_million = parsed * 1_000_000.0;
    per_million.is_finite().then_some(per_million)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(prompt: &str, completion: &str, capabilities: &[&str]) -> OpenRouterModelRaw {
        OpenRouterModelRaw {
            id: "some/model:free".to_owned(),
            name: "Some Model".to_owned(),
            context_length: Some(64_000),
            architecture: OpenRouterArchitecture {
                input_modalities: vec!["text".to_owned(), "image".to_owned()],
            },
            pricing: OpenRouterPrice {
                prompt: Some(Value::String(prompt.to_owned())),
                completion: Some(Value::String(completion.to_owned())),
            },
            supported_parameters: capabilities
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        }
    }

    #[test]
    fn free_model_requires_zero_cost_for_both_input_and_output() {
        let free = OpenRouterModel::from_raw(raw("0", "0.0", &["tools"])).unwrap();
        let paid_output = OpenRouterModel::from_raw(raw("0", "0.0000002", &["tools"])).unwrap();

        assert!(free.is_free());
        assert!(!paid_output.is_free());
        assert_eq!(free.pricing.output_usd_per_million, Some(0.0));
        assert!(free.descriptor().supports_images);
        assert!(free.descriptor().supports_tools);
    }

    #[test]
    fn router_advertises_dynamic_tool_and_image_filtering() {
        let router = OpenRouterFreeProvider::router_model();
        assert!(router.is_router);
        assert!(router.supports_images);
        assert!(router.supports_tools);
        assert_eq!(router.context_length, None);
        assert!(router.is_free());
    }

    #[test]
    fn router_rejects_empty_or_header_injection_credentials() {
        let empty = OpenRouterFreeProvider::new(OpenRouterFreeConfig {
            api_key: "".to_owned(),
            http_referer: None,
            app_name: None,
            request_timeout: DEFAULT_OPENROUTER_CHAT_TIMEOUT,
        });
        let invalid_header = OpenRouterFreeProvider::new(OpenRouterFreeConfig {
            api_key: "key".to_owned(),
            http_referer: None,
            app_name: Some("name\r\nAuthorization: Bearer stolen".to_owned()),
            request_timeout: DEFAULT_OPENROUTER_CHAT_TIMEOUT,
        });

        assert!(empty.validate().is_err());
        assert!(invalid_header.validate().is_err());
    }

    #[test]
    fn catalog_rejects_non_finite_or_negative_prices() {
        assert_eq!(parse_price(Some(&Value::String("NaN".to_owned()))), None);
        assert_eq!(parse_price(Some(&Value::String("-0.01".to_owned()))), None);
        assert_eq!(
            parse_price(Some(&Value::String("0.000001".to_owned()))),
            Some(1.0)
        );
    }
}
