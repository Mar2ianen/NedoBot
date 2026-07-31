use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmProfiles {
    pub providers: BTreeMap<String, ProviderProfile>,
    pub models: BTreeMap<String, ModelProfile>,
    pub routes: BTreeMap<String, RouteProfile>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub driver: LlmDriver,
    pub base_url: String,
    pub api_key_env: String,
    #[serde(default)]
    pub adapter: Option<GenAiAdapter>,
    #[serde(default)]
    pub egress: Egress,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmDriver {
    Gemini,
    OllamaNative,
    OpenaiCompatible,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GenAiAdapter {
    OpenAi,
    Gemini,
    Groq,
    OpenRouter,
    OllamaCloud,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Egress {
    #[default]
    Direct,
    Proxy,
}

impl ProviderProfile {
    pub fn genai_adapter(&self) -> GenAiAdapter {
        self.adapter.unwrap_or(match self.driver {
            LlmDriver::Gemini => GenAiAdapter::Gemini,
            LlmDriver::OllamaNative => GenAiAdapter::OllamaCloud,
            LlmDriver::OpenaiCompatible => GenAiAdapter::OpenAi,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelProfile {
    pub provider: String,
    pub model: String,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelCapabilities {
    pub supports_images: bool,
    pub supports_tools: bool,
    pub supports_system_prompt: bool,
    pub structured_output: StructuredOutputMode,
    pub context_window_tokens: u32,
    pub max_output_tokens: u32,
    pub request_timeout_sec: u64,
    pub thinking: ThinkingMode,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutputMode {
    JsonSchema,
    JsonObject,
    PromptOnly,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingMode {
    None,
    Budget,
    LevelLow,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteProfile {
    pub models: Vec<String>,
    #[serde(default)]
    pub fallback_on_validation_failure: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RouteRequirements {
    pub requires_images: bool,
    pub requires_tools: bool,
    pub requires_system_prompt: bool,
    pub structured_output: Option<StructuredOutputMode>,
    pub num_predict: Option<u32>,
}

/// Валидированная упорядоченная цель маршрута, готовая к созданию транспорта.
///
/// Ссылки заимствованы из `LlmProfiles`; разрешение маршрута не читает секреты и не обращается
/// к runtime-конфигурации.
#[derive(Debug, Clone, Copy)]
pub struct RouteSelection<'a> {
    pub model: &'a ModelProfile,
    pub provider_key: &'a str,
    pub provider: &'a ProviderProfile,
    pub capabilities: &'a ModelCapabilities,
}

/// Разрешённый маршрут с совместимыми моделями в порядке fallback-цепочки.
#[derive(Debug, Clone)]
pub struct ResolvedRoute<'a> {
    pub selections: Vec<RouteSelection<'a>>,
    pub fallback_on_validation_failure: bool,
}

impl LlmProfiles {
    pub fn from_path(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|err| anyhow::anyhow!("failed to read LLM profiles file {path:?}: {err}"))?;
        Self::from_toml(&content)
    }

    pub fn from_toml(content: &str) -> anyhow::Result<Self> {
        let profiles: Self = toml::from_str(content)
            .map_err(|err| anyhow::anyhow!("failed to parse LLM profiles TOML: {err}"))?;
        profiles.validate()?;
        Ok(profiles)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.providers.is_empty() {
            anyhow::bail!("LLM profiles must define at least one provider");
        }
        if self.models.is_empty() {
            anyhow::bail!("LLM profiles must define at least one model");
        }

        for (name, provider) in &self.providers {
            validate_profile_name("provider", name)?;
            validate_http_url("provider", name, &provider.base_url)?;
            require_non_empty("provider", name, "api_key_env", &provider.api_key_env)?;
        }
        for (name, model) in &self.models {
            validate_profile_name("model", name)?;
            if !self.providers.contains_key(&model.provider) {
                anyhow::bail!(
                    "model profile {name:?} references unknown provider {:?}",
                    model.provider
                );
            }
            require_non_empty("model", name, "model", &model.model)?;
            let provider = &self.providers[&model.provider];
            validate_capabilities(name, provider.driver, &model.capabilities)?;
        }
        for (name, route) in &self.routes {
            validate_profile_name("route", name)?;
            if route.models.is_empty() {
                anyhow::bail!("route profile {name:?} must define at least one model");
            }
            if route.fallback_on_validation_failure && route.models.len() < 2 {
                anyhow::bail!(
                    "route profile {name:?} enables fallback_on_validation_failure but has no fallback model"
                );
            }

            let mut seen = BTreeSet::new();
            for model in &route.models {
                if !self.models.contains_key(model) {
                    anyhow::bail!("route profile {name:?} references unknown model {model:?}");
                }
                if !seen.insert(model) {
                    anyhow::bail!(
                        "route profile {name:?} references model {model:?} more than once"
                    );
                }
            }
            self.resolve_route(name, &RouteRequirements::default())?;
        }

        Ok(())
    }

    pub fn resolve_route<'a>(
        &'a self,
        route_name: &str,
        requirements: &RouteRequirements,
    ) -> anyhow::Result<ResolvedRoute<'a>> {
        let route = self
            .routes
            .get(route_name)
            .ok_or_else(|| anyhow::anyhow!("unknown LLM route {route_name:?}"))?;
        let mut selections = Vec::with_capacity(route.models.len());
        let mut unmet_requirement = None;

        for model_key in &route.models {
            let model = self.models.get(model_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "route profile {route_name:?} references unknown model {model_key:?}"
                )
            })?;
            let provider_key = model.provider.as_str();
            let provider = self.providers.get(provider_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "route profile {route_name:?} model {model_key:?} references unknown provider {provider_key:?}"
                )
            })?;
            if let Err(error) =
                ensure_capabilities(route_name, model_key, &model.capabilities, requirements)
            {
                unmet_requirement.get_or_insert(error);
                continue;
            }
            selections.push(RouteSelection {
                model,
                provider_key,
                provider,
                capabilities: &model.capabilities,
            });
        }

        if selections.is_empty() {
            let unmet_requirement =
                unmet_requirement.expect("routes must contain at least one model");
            anyhow::bail!(
                "route {route_name:?} has no compatible models for the requested requirements: {unmet_requirement}"
            );
        }

        Ok(ResolvedRoute {
            selections,
            fallback_on_validation_failure: route.fallback_on_validation_failure,
        })
    }
}

fn ensure_capabilities(
    route_name: &str,
    model_name: &str,
    capabilities: &ModelCapabilities,
    requirements: &RouteRequirements,
) -> anyhow::Result<()> {
    if requirements.requires_images && !capabilities.supports_images {
        anyhow::bail!(
            "route {route_name:?} requires images but model {model_name:?} does not support them"
        );
    }
    if requirements.requires_tools && !capabilities.supports_tools {
        anyhow::bail!(
            "route {route_name:?} requires tools but model {model_name:?} does not support them"
        );
    }
    if requirements.requires_system_prompt && !capabilities.supports_system_prompt {
        anyhow::bail!(
            "route {route_name:?} requires a system prompt but model {model_name:?} does not support it"
        );
    }
    if let Some(structured_output) = requirements.structured_output
        && capabilities.structured_output != structured_output
    {
        anyhow::bail!(
            "route {route_name:?} requires structured output mode {structured_output:?} but model {model_name:?} declares {:?}",
            capabilities.structured_output
        );
    }
    if let Some(num_predict) = requirements.num_predict
        && num_predict > capabilities.max_output_tokens
    {
        anyhow::bail!(
            "route {route_name:?} requires num_predict {num_predict} but model {model_name:?} allows at most {} output tokens",
            capabilities.max_output_tokens
        );
    }
    Ok(())
}

fn validate_profile_name(kind: &str, name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        anyhow::bail!("{kind} profile name must not be empty");
    }
    Ok(())
}

fn require_non_empty(kind: &str, name: &str, field: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{kind} profile {name:?} requires non-empty {field}");
    }
    Ok(())
}

fn validate_http_url(kind: &str, name: &str, value: &str) -> anyhow::Result<()> {
    let valid = reqwest::Url::parse(value)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
    if !valid {
        anyhow::bail!("{kind} profile {name:?} base_url must be an absolute HTTP(S) URL");
    }
    Ok(())
}

fn validate_capabilities(
    name: &str,
    driver: LlmDriver,
    capabilities: &ModelCapabilities,
) -> anyhow::Result<()> {
    if capabilities.context_window_tokens == 0 {
        anyhow::bail!("model profile {name:?} context_window_tokens must be greater than 0");
    }
    if capabilities.max_output_tokens == 0 {
        anyhow::bail!("model profile {name:?} max_output_tokens must be greater than 0");
    }
    if capabilities.max_output_tokens > capabilities.context_window_tokens {
        anyhow::bail!(
            "model profile {name:?} max_output_tokens must not exceed context_window_tokens"
        );
    }
    if capabilities.request_timeout_sec == 0 {
        anyhow::bail!("model profile {name:?} request_timeout_sec must be greater than 0");
    }
    if capabilities.thinking != ThinkingMode::None && driver != LlmDriver::Gemini {
        anyhow::bail!("model profile {name:?} enables thinking for a non-Gemini provider");
    }
    match capabilities.structured_output {
        StructuredOutputMode::JsonSchema
        | StructuredOutputMode::JsonObject
        | StructuredOutputMode::PromptOnly => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE_PROFILES: &str = include_str!("../../config/llm_profiles.toml.example");

    const VALID_PROFILES: &str = r#"
[providers.ollama_cloud]
driver = "ollama_native"
base_url = "https://ollama.com"
api_key_env = "OLLAMA_API_KEY"

[models.ollama_memory]
provider = "ollama_cloud"
model = "gemma4:31b"

[models.ollama_memory.capabilities]
supports_images = false
supports_tools = true
supports_system_prompt = true
structured_output = "json_object"
context_window_tokens = 32768
max_output_tokens = 4096
request_timeout_sec = 120
thinking = "none"

[routes.memory]
models = ["ollama_memory"]
"#;

    #[test]
    fn committed_example_parses_and_validates() {
        let profiles = LlmProfiles::from_toml(EXAMPLE_PROFILES).unwrap();

        assert!(profiles.routes.contains_key("first_comment"));
        assert!(profiles.routes.contains_key("voice_cleanup"));
    }

    #[test]
    fn parses_and_validates_provider_model_and_route_profiles() {
        let profiles = LlmProfiles::from_toml(VALID_PROFILES).unwrap();

        assert_eq!(profiles.providers.len(), 1);
        assert_eq!(profiles.models.len(), 1);
        assert_eq!(profiles.routes["memory"].models, ["ollama_memory"]);
        assert!(profiles.models["ollama_memory"].capabilities.supports_tools);
    }

    #[test]
    fn route_resolution_returns_ordered_runtime_ready_selections() {
        let profiles = LlmProfiles::from_toml(EXAMPLE_PROFILES).unwrap();

        let selections = profiles
            .resolve_route("voice_cleanup", &RouteRequirements::default())
            .unwrap();

        assert_eq!(selections.selections.len(), 2);
        assert_eq!(
            (
                selections.selections[0].provider_key,
                selections.selections[0].model.model.as_str()
            ),
            ("groq", "llama-3.3-70b-versatile")
        );
        assert_eq!(
            selections.selections[0].provider.driver,
            LlmDriver::OpenaiCompatible
        );
        assert_eq!(
            selections.selections[0].capabilities.structured_output,
            StructuredOutputMode::JsonSchema
        );
        assert_eq!(
            (
                selections.selections[1].provider_key,
                selections.selections[1].model.model.as_str()
            ),
            ("ollama_cloud", "gemma4:31b")
        );
        assert!(!selections.fallback_on_validation_failure);
    }

    #[test]
    fn route_resolution_skips_incompatible_models_and_preserves_text_fallbacks() {
        let profiles = LlmProfiles::from_toml(&EXAMPLE_PROFILES.replace(
            "fallback_on_validation_failure = false",
            "fallback_on_validation_failure = true",
        ))
        .unwrap();

        let image_route = profiles
            .resolve_route(
                "first_comment",
                &RouteRequirements {
                    requires_images: true,
                    ..RouteRequirements::default()
                },
            )
            .unwrap();
        let image_models: Vec<_> = image_route
            .selections
            .iter()
            .map(|selection| (selection.provider_key, selection.model.model.as_str()))
            .collect();

        assert_eq!(
            image_models,
            [
                ("gemini", "gemini-3.6-flash"),
                ("gemini", "gemini-3.5-flash"),
                ("gemini", "gemini-3.5-flash-lite"),
            ]
        );
        assert!(image_route.fallback_on_validation_failure);

        let text_route = profiles
            .resolve_route("first_comment", &RouteRequirements::default())
            .unwrap();
        let text_models: Vec<_> = text_route
            .selections
            .iter()
            .map(|selection| (selection.provider_key, selection.model.model.as_str()))
            .collect();

        assert_eq!(
            text_models,
            [
                ("gemini", "gemini-3.6-flash"),
                ("gemini", "gemini-3.5-flash"),
                ("gemini", "gemini-3.5-flash-lite"),
                ("ollama_cloud", "gemma4:31b"),
            ]
        );
    }

    #[test]
    fn route_resolution_rejects_unsupported_structured_output_mode() {
        let profiles = LlmProfiles::from_toml(VALID_PROFILES).unwrap();
        let requirements = RouteRequirements {
            requires_system_prompt: true,
            structured_output: Some(StructuredOutputMode::JsonSchema),
            ..RouteRequirements::default()
        };

        let error = profiles
            .resolve_route("memory", &requirements)
            .unwrap_err()
            .to_string();

        assert!(error.contains("route \"memory\""));
        assert!(error.contains("model \"ollama_memory\""));
        assert!(error.contains("structured output mode JsonSchema"));
    }

    #[test]
    fn route_resolution_rejects_unsupported_system_prompt() {
        let profiles = LlmProfiles::from_toml(&VALID_PROFILES.replace(
            "supports_system_prompt = true",
            "supports_system_prompt = false",
        ))
        .unwrap();
        let requirements = RouteRequirements {
            requires_system_prompt: true,
            ..RouteRequirements::default()
        };

        let error = profiles
            .resolve_route("memory", &requirements)
            .unwrap_err()
            .to_string();

        assert!(error.contains("route \"memory\""));
        assert!(error.contains("model \"ollama_memory\""));
        assert!(error.contains("requires a system prompt"));
    }

    #[test]
    fn route_resolution_rejects_num_predict_above_model_output_limit() {
        let profiles = LlmProfiles::from_toml(VALID_PROFILES).unwrap();
        let requirements = RouteRequirements {
            num_predict: Some(4097),
            ..RouteRequirements::default()
        };

        let error = profiles
            .resolve_route("memory", &requirements)
            .unwrap_err()
            .to_string();

        assert!(error.contains("route \"memory\""));
        assert!(error.contains("model \"ollama_memory\""));
        assert!(error.contains("num_predict 4097"));
        assert!(error.contains("at most 4096 output tokens"));
    }

    #[test]
    fn loads_profiles_from_a_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), VALID_PROFILES).unwrap();

        let profiles = LlmProfiles::from_path(file.path().to_str().unwrap()).unwrap();

        assert!(profiles.routes.contains_key("memory"));
    }

    #[test]
    fn rejects_route_referencing_unknown_model() {
        let invalid = VALID_PROFILES.replace("ollama_memory\"]", "missing_model\"]");

        let error = LlmProfiles::from_toml(&invalid).unwrap_err().to_string();

        assert!(error.contains("unknown model"));
    }

    #[test]
    fn rejects_unknown_optional_route_field() {
        let invalid = VALID_PROFILES.replace(
            "models = [\"ollama_memory\"]",
            "models = [\"ollama_memory\"]\nfallback_on_validation_faliure = true",
        );

        let error = LlmProfiles::from_toml(&invalid).unwrap_err().to_string();

        assert!(error.contains("unknown field `fallback_on_validation_faliure`"));
    }

    #[test]
    fn rejects_invalid_provider_endpoint() {
        let invalid = VALID_PROFILES.replace("https://ollama.com", "not a URL");

        let error = LlmProfiles::from_toml(&invalid).unwrap_err().to_string();

        assert!(error.contains("base_url must be an absolute HTTP(S) URL"));
    }

    #[test]
    fn rejects_output_limit_larger_than_context_window() {
        let invalid =
            VALID_PROFILES.replace("max_output_tokens = 4096", "max_output_tokens = 65536");

        let error = LlmProfiles::from_toml(&invalid).unwrap_err().to_string();

        assert!(error.contains("max_output_tokens must not exceed context_window_tokens"));
    }

    #[test]
    fn route_resolution_rejects_missing_required_capability() {
        let profiles = LlmProfiles::from_toml(VALID_PROFILES).unwrap();
        let requirements = RouteRequirements {
            requires_images: true,
            ..RouteRequirements::default()
        };

        let error = profiles
            .resolve_route("memory", &requirements)
            .unwrap_err()
            .to_string();

        assert!(error.contains("requires images"));
    }
}
