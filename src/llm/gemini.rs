use async_trait::async_trait;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
use crate::http;
use crate::llm::profiles::{StructuredOutputMode, ThinkingMode};
use crate::llm::types::{LlmClient, LlmRequest, LlmResponse, LlmTransportError};

const GEMINI_API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiClient<'a> {
    api_base_url: &'a str,
    api_key: &'a str,
    proxy_url: Option<&'a str>,
    thinking_budget: u32,
    profile_thinking: Option<ThinkingMode>,
    profile_structured_output: Option<StructuredOutputMode>,
    timeout: Duration,
}

impl<'a> GeminiClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            api_base_url: GEMINI_API_BASE_URL,
            api_key: config.gemini_api_key.trim(),
            proxy_url: config.llm_proxy_url.as_deref().map(str::trim),
            thinking_budget: config.gemini_thinking_budget,
            profile_thinking: None,
            profile_structured_output: None,
            timeout: Duration::from_secs(45),
        }
    }

    pub fn with_profile(
        api_base_url: &'a str,
        api_key: &'a str,
        proxy_url: Option<&'a str>,
        thinking_budget: u32,
        thinking: ThinkingMode,
        structured_output: StructuredOutputMode,
        timeout: Duration,
    ) -> Self {
        Self {
            api_base_url: api_base_url.trim_end_matches('/'),
            api_key: api_key.trim(),
            proxy_url: proxy_url.map(str::trim),
            thinking_budget,
            profile_thinking: Some(thinking),
            profile_structured_output: Some(structured_output),
            timeout,
        }
    }
}

#[async_trait]
impl LlmClient for GeminiClient<'_> {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse> {
        if self.api_key.is_empty() {
            return Err(LlmTransportError::configuration().into());
        }

        let body = GenerateContentRequest {
            system_instruction: request.system_prompt.map(|system_prompt| GeminiContent {
                role: "system",
                parts: vec![GeminiPart::Text {
                    text: system_prompt,
                }],
            }),
            contents: vec![GeminiContent {
                role: "user",
                parts: request_parts(request.prompt, request.image_base64),
            }],
            generation_config: generation_config(
                &request,
                self.thinking_budget,
                self.profile_thinking,
                self.profile_structured_output,
            ),
        };

        let response = http::client_with_proxy(self.timeout, self.proxy_url)?
            .post(format!(
                "{}/models/{}:generateContent",
                self.api_base_url,
                request.model.trim()
            ))
            .header(USER_AGENT, "tg-ai-bot-teloxide/0.1")
            .header("x-goog-api-key", self.api_key)
            .json(&body)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(LlmTransportError::http_status(response.status().as_u16()).into());
        }
        let response = response.json::<GenerateContentResponse>().await?;

        let candidate = response
            .candidates
            .into_iter()
            .next()
            .ok_or_else(LlmTransportError::empty_response)?;

        if candidate.finish_reason.as_deref() == Some("MAX_TOKENS") {
            anyhow::bail!("Gemini response stopped due to MAX_TOKENS");
        }

        let content = candidate
            .content
            .parts
            .into_iter()
            .filter_map(|part| (!part.thought).then_some(part.text).flatten())
            .collect::<Vec<_>>()
            .join("\n");

        if content.trim().is_empty() {
            return Err(LlmTransportError::empty_response().into());
        }

        Ok(LlmResponse { content })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent<'a>>,
    contents: Vec<GeminiContent<'a>>,
    generation_config: GenerationConfig<'a>,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    role: &'a str,
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_json_schema: Option<&'a serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", untagged)]
enum GeminiPart<'a> {
    Text {
        text: &'a str,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData<'a>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InlineData<'a> {
    mime_type: &'a str,
    data: &'a str,
}

#[derive(Deserialize)]
struct GenerateContentResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiResponseContent,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    #[serde(default)]
    parts: Vec<GeminiResponsePart>,
}

#[derive(Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
    #[serde(default)]
    thought: bool,
}

fn generation_config<'a>(
    request: &LlmRequest<'a>,
    thinking_budget: u32,
    profile_thinking: Option<ThinkingMode>,
    profile_structured_output: Option<StructuredOutputMode>,
) -> GenerationConfig<'a> {
    let legacy_gemini_3 = request.model.trim().starts_with("gemini-3.");
    let legacy_thinking = if legacy_gemini_3 {
        ThinkingMode::LevelLow
    } else if thinking_budget > 0 {
        ThinkingMode::Budget
    } else {
        ThinkingMode::None
    };
    let thinking = profile_thinking.unwrap_or(legacy_thinking);
    let thinking_config = match thinking {
        ThinkingMode::None => None,
        ThinkingMode::Budget => Some(ThinkingConfig {
            thinking_budget: Some(thinking_budget),
            thinking_level: None,
        }),
        ThinkingMode::LevelLow => Some(ThinkingConfig {
            thinking_budget: None,
            thinking_level: Some("low"),
        }),
    };

    let adds_thinking_budget = thinking == ThinkingMode::Budget;
    let structured_mode = profile_structured_output.unwrap_or(StructuredOutputMode::JsonSchema);
    let has_structured_output = request.structured_output.is_some();

    GenerationConfig {
        temperature: (thinking != ThinkingMode::LevelLow).then_some(request.temperature),
        max_output_tokens: if adds_thinking_budget {
            request.num_predict.saturating_add(thinking_budget)
        } else {
            request.num_predict
        },
        thinking_config,
        response_mime_type: (has_structured_output
            && structured_mode != StructuredOutputMode::PromptOnly)
            .then_some("application/json"),
        response_json_schema: (has_structured_output
            && structured_mode == StructuredOutputMode::JsonSchema)
            .then(|| {
                request
                    .structured_output
                    .expect("structured output exists")
                    .schema
            }),
    }
}

fn request_parts<'a>(prompt: &'a str, image_base64: Option<&'a str>) -> Vec<GeminiPart<'a>> {
    let mut parts = vec![GeminiPart::Text { text: prompt }];
    if let Some(image_base64) = image_base64 {
        parts.push(GeminiPart::InlineData {
            inline_data: InlineData {
                mime_type: "image/jpeg",
                data: image_base64,
            },
        });
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_parts_match_gemini_api_shape() {
        let parts = request_parts("hello", Some("base64-image"));

        assert_eq!(
            serde_json::to_value(parts).unwrap(),
            json!([
                {"text": "hello"},
                {"inlineData": {"mimeType": "image/jpeg", "data": "base64-image"}}
            ])
        );
    }

    #[test]
    fn legacy_generation_config_keeps_thinking_budget_separate_from_answer_budget() {
        let output_budget = 90;
        let thinking_budget = 256;
        let request = LlmRequest {
            model: "gemini-2.5-flash",
            system_prompt: None,
            prompt: "hello",
            image_base64: None,
            temperature: 0.35,
            num_predict: output_budget,
            structured_output: None,
        };
        let config = generation_config(&request, thinking_budget, None, None);

        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["maxOutputTokens"], json!(346));
        assert_eq!(value["thinkingConfig"]["thinkingBudget"], json!(256));
        assert!(
            value["temperature"]
                .as_f64()
                .is_some_and(|value| (value - 0.35).abs() < 0.001)
        );
    }

    #[test]
    fn profile_generation_config_honors_declared_thinking_and_structured_modes() {
        let schema = json!({"type": "object"});
        let request = LlmRequest {
            model: "arbitrary-profile-model",
            system_prompt: None,
            prompt: "hello",
            image_base64: None,
            temperature: 0.35,
            num_predict: 180,
            structured_output: Some(crate::llm::types::StructuredOutput {
                name: "result",
                schema: &schema,
            }),
        };
        let config = generation_config(
            &request,
            1024,
            Some(ThinkingMode::None),
            Some(StructuredOutputMode::PromptOnly),
        );

        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["maxOutputTokens"], json!(180));
        assert!(value.get("thinkingConfig").is_none());
        assert!(value.get("responseMimeType").is_none());
        assert!(value.get("responseJsonSchema").is_none());
        assert!(
            value["temperature"]
                .as_f64()
                .is_some_and(|value| (value - 0.35).abs() < 0.001)
        );
    }

    #[test]
    fn gemini_3_generation_config_uses_current_api_parameters() {
        let schema = json!({"type": "object", "properties": {"comment": {"type": "string"}}});
        let request = LlmRequest {
            model: "gemini-3.6-flash",
            system_prompt: None,
            prompt: "hello",
            image_base64: None,
            temperature: 0.35,
            num_predict: 180,
            structured_output: Some(crate::llm::types::StructuredOutput {
                name: "comment",
                schema: &schema,
            }),
        };
        let config = generation_config(&request, 1024, None, None);

        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["maxOutputTokens"], json!(180));
        assert_eq!(value["thinkingConfig"]["thinkingLevel"], json!("low"));
        assert!(value.get("temperature").is_none());
        assert!(value["thinkingConfig"].get("thinkingBudget").is_none());
        assert_eq!(value["responseMimeType"], json!("application/json"));
        assert_eq!(value["responseJsonSchema"], schema);
    }

    #[test]
    fn response_parts_skip_thought_summaries() {
        let response: GenerateContentResponse = serde_json::from_value(json!({
            "candidates": [{
                "content": {"parts": [
                    {"text": "internal", "thought": true},
                    {"text": "answer"}
                ]}
            }]
        }))
        .unwrap();

        let content = response.candidates[0]
            .content
            .parts
            .iter()
            .filter_map(|part| (!part.thought).then_some(part.text.as_deref()).flatten())
            .collect::<Vec<_>>();
        assert_eq!(content, ["answer"]);
    }
}
