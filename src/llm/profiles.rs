use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct LlmProfiles {
    pub providers: BTreeMap<String, ProviderProfile>,
    pub models: BTreeMap<String, ModelProfile>,
    pub routes: BTreeMap<String, RouteProfile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderProfile {
    pub driver: LlmDriver,
    pub base_url: String,
    pub api_key_env: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LlmDriver {
    Gemini,
    OllamaNative,
    OpenaiCompatible,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelProfile {
    pub provider: String,
    pub model: String,
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, Deserialize)]
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
    ) -> anyhow::Result<Vec<&'a ModelProfile>> {
        let route = self
            .routes
            .get(route_name)
            .ok_or_else(|| anyhow::anyhow!("unknown LLM route {route_name:?}"))?;
        let mut models = Vec::with_capacity(route.models.len());
        for model_name in &route.models {
            let model = self.models.get(model_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "route profile {route_name:?} references unknown model {model_name:?}"
                )
            })?;
            ensure_capabilities(route_name, model_name, &model.capabilities, requirements)?;
            models.push(model);
        }
        Ok(models)
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
            "route {route_name:?} requires {structured_output:?} but model {model_name:?} uses {:?}",
            capabilities.structured_output
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
