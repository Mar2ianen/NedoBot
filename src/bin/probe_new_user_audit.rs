//! Ограниченный live probe реального audit route, без Telegram, БД и chat data.
use anyhow::{Context, ensure};
use base64::Engine;
use serde_json::{Value, json};
use teloxide_antispam::assessment::NewUserAuditAssessment;
use tg_ai_bot_teloxide::{
    config::Config,
    features::new_user_audit::prompt,
    llm::{
        service::{GenerateTextOptions, generate_text_checked},
        types::StructuredOutput,
    },
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(
        args.len() == 2 && args[0] == "--provider",
        "usage: probe_new_user_audit --provider <configured-provider>"
    );
    let expected = &args[1];
    let mut config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    let profiles = config
        .llm_profiles
        .as_mut()
        .context("LLM profiles missing")?;
    let models = profiles
        .models
        .iter()
        .filter(|(_, m)| &m.provider == expected)
        .map(|(k, _)| k.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let route = profiles
        .routes
        .get_mut("new_user_audit")
        .context("audit route missing")?;
    route.models.retain(|m| models.contains(m));
    ensure!(
        !route.models.is_empty(),
        "provider not configured in audit route"
    );
    // Probe не должен маскировать отказ проверяемого провайдера чужим fallback.
    route.fallback_on_validation_failure = false;
    profiles.validate()?;
    let image = base64::engine::general_purpose::STANDARD;
    let schema = json!({"type":"object","additionalProperties":false,"properties":{
        "left":{"type":"string","enum":["red","blue"]},"right":{"type":"string","enum":["red","blue"]}
    },"required":["left","right"]});
    for (bytes, left, right) in [
        (
            include_bytes!("../../tests/fixtures/vision-red-blue.jpg").as_slice(),
            "red",
            "blue",
        ),
        (
            include_bytes!("../../tests/fixtures/vision-blue-red.jpg").as_slice(),
            "blue",
            "red",
        ),
    ] {
        let encoded = image.encode(bytes);
        let expected_value = json!({"left":left,"right":right});
        let validator = move |text: &str| -> anyhow::Result<()> {
            let parsed: Value = serde_json::from_str(text)?;
            ensure!(parsed == expected_value, "image colors do not match");
            Ok(())
        };
        let output=generate_text_checked(&config,GenerateTextOptions {
            route:"new_user_audit",system_prompt:Some("Identify the colors on the left and right halves of the image. Return JSON only."),
            prompt:"Inspect the attached image.",image_base64:Some(&encoded),
            temperature:0.0,num_predict:128,output_validator:Some(&validator),
            structured_output:Some(StructuredOutput{name:"vision_probe",schema:&schema}),
        }).await?;
        ensure!(
            &output.provider == expected && output.image_used,
            "probe used another provider or omitted image"
        );
        println!(
            "{}",
            json!({"provider":output.provider,"model":output.model,"probe":"vision_colors","expected_left":left,"passed":true})
        );
    }
    for (has_image, has_message) in [(false, true), (true, true), (true, false), (false, false)] {
        let encoded = image.encode(include_bytes!("../../tests/fixtures/vision-red-blue.jpg"));
        let input = prompt::build_input(&json!({
            "schema_version":"synthetic-provider-probe",
            "profile":{"display_name":"Synthetic Fixture","is_bot":false,"avatar_image_available":has_image},
            "text":{"first_message_preview":has_message.then_some("В Rust вышло обновление компилятора. Какие изменения в проверке типов?"),"first_message_reply_context_preview":has_message.then_some("Обсуждение Rust.")},
            "personal_channel":{"present":false},"activity":{"message_count":1,"chat_age_sec":60}
        }))?;
        let validator = move |text: &str| -> anyhow::Result<()> {
            NewUserAuditAssessment::parse_for_modalities(text, has_image, has_message)?;
            Ok(())
        };
        let output = generate_text_checked(
            &config,
            GenerateTextOptions {
                route: "new_user_audit",
                system_prompt: Some(prompt::system_prompt()),
                prompt: &input,
                image_base64: has_image.then_some(encoded.as_str()),
                temperature: 0.0,
                num_predict: config.new_user_audit_max_tokens,
                output_validator: Some(&validator),
                structured_output: Some(StructuredOutput {
                    name: "new_user_audit_assessment",
                    schema: prompt::output_schema(),
                }),
            },
        )
        .await?;
        ensure!(
            &output.provider == expected && output.image_used == has_image,
            "unexpected provider/modality"
        );
        println!(
            "{}",
            json!({"provider":output.provider,"model":output.model,"probe":"full_audit_schema","image":has_image,"first_message":has_message,"passed":true})
        );
    }
    Ok(())
}
