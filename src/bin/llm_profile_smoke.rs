use base64::Engine as _;
use tg_ai_bot_teloxide::{
    config::Config,
    features::new_user_audit::{prompt::output_schema, types::NewUserAuditAssessment},
    llm::{
        service::{GenerateTextOptions, generate_text_checked},
        types::StructuredOutput,
    },
};

const ROUTES: &[&str] = &[
    "first_comment",
    "memory",
    "voice_cleanup",
    "search_extract",
    "new_user_audit",
    "ask",
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

    let validator = |content: &str| {
        if content.trim().eq_ignore_ascii_case("ok") {
            Ok(())
        } else {
            anyhow::bail!("smoke response must normalize exactly to `ok`");
        }
    };

    for route in ROUTES {
        let is_new_user_audit = *route == "new_user_audit";
        let image_base64 = match (
            is_new_user_audit,
            std::env::var("LLM_PROFILE_SMOKE_IMAGE_PATH"),
        ) {
            (true, Ok(path)) => {
                Some(base64::engine::general_purpose::STANDARD.encode(std::fs::read(path)?))
            }
            _ => None,
        };
        let has_avatar_input = image_base64.is_some();
        let structured_output = is_new_user_audit.then(|| StructuredOutput {
            name: "new_user_audit_assessment",
            schema: output_schema(),
        });
        let system_prompt = if is_new_user_audit {
            "Верни JSON-объект по переданной схеме. Для smoke используй безопасные пустые признаки риска."
        } else {
            "Ответь ровно одним словом: ok"
        };
        let prompt = if is_new_user_audit {
            format!(
                "Smoke-проверка new_user_audit JSON Schema. В снимке есть первое сообщение `Привет, это тест`. Верни полный валидный JSON-объект по схеме. output_contract: {}",
                serde_json::to_string(output_schema())?
            )
        } else {
            "Smoke-проверка profile router. Ответь ровно: ok".to_string()
        };
        let output_validator = if is_new_user_audit {
            if has_avatar_input {
                Some(&validate_audit_image as &tg_ai_bot_teloxide::llm::service::OutputValidator)
            } else {
                Some(&validate_audit_text as &tg_ai_bot_teloxide::llm::service::OutputValidator)
            }
        } else {
            Some(&validator as &tg_ai_bot_teloxide::llm::service::OutputValidator)
        };
        let num_predict = if is_new_user_audit {
            config.new_user_audit_max_tokens
        } else {
            config.llm_max_tokens
        };
        let generation = generate_text_checked(
            &config,
            GenerateTextOptions {
                route,
                system_prompt: Some(system_prompt),
                prompt: &prompt,
                image_base64: image_base64.as_deref(),
                temperature: 0.0,
                num_predict,
                output_validator,
                structured_output,
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

fn validate_audit_text(output: &str) -> anyhow::Result<()> {
    NewUserAuditAssessment::parse_for_modalities(output, false, true).map(|_| ())
}

fn validate_audit_image(output: &str) -> anyhow::Result<()> {
    NewUserAuditAssessment::parse_for_modalities(output, true, true).map(|_| ())
}
