//! Проверяет native tool calling маршрута /ask без Telegram и доступа к БД.
use base64::Engine;
use genai::chat::{ChatMessage, ContentPart, MessageContent, Tool};
use serde_json::json;
use std::sync::Arc;
use tg_ai_bot_teloxide::{
    config::Config,
    llm::service::{GenerateChatOptions, generate_chat_audited_checked},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let mut requires_images = false;
    for argument in std::env::args().skip(1) {
        if argument == "--image" {
            requires_images = true
        } else {
            anyhow::bail!("usage: probe_ask_route [--image]")
        }
    }
    let config = Config::from_env()?;
    let mut parts = vec![ContentPart::from_text(
        "Вызови inspect с query=test. Это проверка формата, других действий не требуется.",
    )];
    if requires_images {
        let image = base64::engine::general_purpose::STANDARD
            .encode(include_bytes!("../../tests/fixtures/ask-probe.png"));
        parts.push(ContentPart::from_binary_base64(
            "image/png",
            Arc::<str>::from(image),
            Some("test.png".into()),
        ));
    }
    let tool = Tool::new("inspect").with_description("Проверить тестовую строку")
        .with_schema(json!({"type":"object","additionalProperties":false,"properties":{"query":{"type":"string"}},"required":["query"]})).with_strict(true);
    let generated = generate_chat_audited_checked(
        &config,
        GenerateChatOptions {
            fallback_offset: 0,
            route: "ask",
            system_prompt: Some("Диагностика native tools. Верни вызов inspect с query=test."),
            messages: vec![ChatMessage::user(MessageContent::from_parts(parts))],
            tools: Some(vec![tool]),
            requires_images,
            requires_tools: true,
            previous_response_id: None,
            temperature: 0.0,
            num_predict: 1024,
        },
    )
    .await?;
    let verified = generated
        .response
        .tool_calls()
        .iter()
        .any(|call| call.fn_name == "inspect" && call.fn_arguments["query"] == "test");
    println!(
        "{}",
        json!({"provider":generated.provider,"model":generated.model,"requires_images":requires_images,"native_tool_verified":verified})
    );
    anyhow::ensure!(
        verified,
        "model did not return the requested native tool call"
    );
    Ok(())
}
