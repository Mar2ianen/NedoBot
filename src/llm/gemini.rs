use async_trait::async_trait;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::Config;
use crate::http;
use crate::llm::types::{LlmClient, LlmRequest, LlmResponse};

const GEMINI_API_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub struct GeminiClient<'a> {
    api_key: &'a str,
    proxy_url: Option<&'a str>,
    thinking_budget: u32,
}

impl<'a> GeminiClient<'a> {
    pub fn new(config: &'a Config) -> Self {
        Self {
            api_key: config.gemini_api_key.trim(),
            proxy_url: config.llm_proxy_url.as_deref().map(str::trim),
            thinking_budget: config.gemini_thinking_budget,
        }
    }
}

#[async_trait]
impl LlmClient for GeminiClient<'_> {
    async fn generate(&self, request: LlmRequest<'_>) -> anyhow::Result<LlmResponse> {
        if self.api_key.is_empty() {
            anyhow::bail!("GEMINI_API_KEY is empty");
        }

        let body = GenerateContentRequest {
            contents: vec![GeminiContent {
                role: "user",
                parts: request_parts(request.prompt, request.image_base64),
            }],
            generation_config: GenerationConfig {
                temperature: request.temperature,
                max_output_tokens: request.num_predict.saturating_add(self.thinking_budget),
                thinking_config: (self.thinking_budget > 0).then_some(ThinkingConfig {
                    thinking_budget: self.thinking_budget,
                }),
            },
        };

        let response = http::client_with_proxy(Duration::from_secs(45), self.proxy_url)?
            .post(format!(
                "{}/models/{}:generateContent",
                GEMINI_API_BASE_URL,
                request.model.trim()
            ))
            .header(USER_AGENT, "tg-ai-bot-teloxide/0.1")
            .header("x-goog-api-key", self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json::<GenerateContentResponse>()
            .await?;

        let content = response
            .candidates
            .into_iter()
            .next()
            .map(|candidate| {
                candidate
                    .content
                    .parts
                    .into_iter()
                    .filter_map(|part| part.text)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();

        if content.trim().is_empty() {
            anyhow::bail!("empty Gemini response");
        }

        Ok(LlmResponse { content })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateContentRequest<'a> {
    contents: Vec<GeminiContent<'a>>,
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    role: &'a str,
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationConfig {
    temperature: f32,
    max_output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<ThinkingConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThinkingConfig {
    thinking_budget: u32,
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
}

#[derive(Deserialize)]
struct GeminiResponseContent {
    #[serde(default)]
    parts: Vec<GeminiResponsePart>,
}

#[derive(Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
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
    fn generation_config_keeps_thinking_budget_separate_from_answer_budget() {
        let output_budget = 90;
        let thinking_budget = 256;
        let config = GenerationConfig {
            temperature: 0.35,
            max_output_tokens: output_budget + thinking_budget,
            thinking_config: Some(ThinkingConfig { thinking_budget }),
        };

        let value = serde_json::to_value(config).unwrap();
        assert_eq!(value["maxOutputTokens"], json!(346));
        assert_eq!(value["thinkingConfig"]["thinkingBudget"], json!(256));
    }
}
