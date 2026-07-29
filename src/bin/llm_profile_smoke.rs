use tg_ai_bot_teloxide::{
    config::Config,
    llm::service::{GenerateTextOptions, generate_text_with_provider_checked},
};

const ROUTES: &[&str] = &[
    "first_comment",
    "memory",
    "voice_cleanup",
    "search_extract",
    "avatar_analysis",
    "first_message_spam",
    "ask",
    "legacy_default",
];

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    ensure_no_arguments()?;

    let config = Config::from_env()?;
    config.validate_runtime_secrets()?;
    if config.llm_profiles.is_none() {
        anyhow::bail!("LLM_PROFILES_PATH must be configured for profile smoke testing");
    }

    for route in ROUTES {
        let generation = generate_text_with_provider_checked(
            &config,
            GenerateTextOptions {
                route: Some(route),
                provider_override: None,
                model_override: None,
                system_prompt: Some("Ответь ровно одним словом: ok"),
                prompt: "Smoke-проверка profile router. Ответь ровно: ok",
                image_base64: None,
                temperature: 0.0,
                num_predict: 32,
                output_validator: None,
                structured_output: None,
            },
        )
        .await?;

        let attempts = generation
            .attempts
            .iter()
            .map(|attempt| format!("{}/{}/{}", attempt.provider, attempt.model, attempt.outcome))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "route={route} provider={} model={} image_used={} response_chars={} attempts={attempts}",
            generation.provider,
            generation.model,
            generation.image_used,
            generation.content.chars().count(),
        );
    }

    Ok(())
}

fn ensure_no_arguments() -> anyhow::Result<()> {
    if std::env::args().nth(1).is_some() {
        anyhow::bail!("Usage: llm_profile_smoke");
    }
    Ok(())
}
