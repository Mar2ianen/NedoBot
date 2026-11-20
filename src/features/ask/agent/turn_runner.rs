use super::*;

pub(super) struct TurnRunner;

impl TurnRunner {
    pub(super) async fn generate(
        config: &Config,
        profile: &AgentProfile,
        messages: &[ChatMessage],
        system_prompt: &str,
        tools: Option<Vec<Tool>>,
        has_image: bool,
    ) -> Result<GeneratedChat, AgentGenerationError> {
        generate_turn_impl(config, profile, messages, system_prompt, tools, has_image).await
    }
}

async fn generate_turn_impl(
    config: &Config,
    profile: &AgentProfile,
    messages: &[ChatMessage],
    system_prompt: &str,
    tools: Option<Vec<Tool>>,
    requires_images: bool,
) -> Result<GeneratedChat, AgentGenerationError> {
    let mut attempt = 0;
    retry_once_on_timeout(
        Duration::from_secs(profile.budgets.action_timeout_sec),
        || {
            let fallback_offset = attempt;
            attempt += 1;
            generate_chat_audited_checked(
                config,
                GenerateChatOptions {
                    fallback_offset,
                    route: &profile.model_route,
                    system_prompt: Some(system_prompt),
                    messages: messages.to_vec(),
                    tools: tools.clone(),
                    requires_images,
                    requires_tools: tools.is_some(),
                    // Передаём полную native историю. Continuation ID добавил бы
                    // ту же историю повторно и мог уйти другому fallback provider.
                    previous_response_id: None,
                    temperature: config.ask_llm_temperature,
                    num_predict: profile.budgets.max_output_tokens,
                    output_budget_policy: OutputBudgetPolicy::AdaptToModel,
                },
            )
        },
    )
    .await
}

pub(super) async fn retry_once_on_timeout<T, F, Fut>(
    timeout_duration: Duration,
    mut generate: F,
) -> Result<T, AgentGenerationError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    match timeout(timeout_duration, generate()).await {
        Ok(Ok(generated)) => Ok(generated),
        Ok(Err(err)) => Err(AgentGenerationError::Request(err)),
        Err(_) => {
            tracing::warn!(
                timeout_secs = timeout_duration.as_secs(),
                "ask LLM action timed out; retrying once"
            );
            match timeout(timeout_duration, generate()).await {
                Ok(Ok(generated)) => Ok(generated),
                Ok(Err(err)) => Err(AgentGenerationError::Request(err)),
                Err(_) => Err(AgentGenerationError::Request(AskFailureTag::wrap(
                    AskFailureKind::Timeout,
                    anyhow::anyhow!("ask LLM timed out twice"),
                ))),
            }
        }
    }
}
