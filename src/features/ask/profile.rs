use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};

use crate::llm::profiles::{AgentBudgets, AgentMemoryConfig, AgentMemoryWritePolicy, LlmProfiles};

const MAX_PROMPT_FILE_BYTES: u64 = 256 * 1024;
const NEDONEWS_DEFAULT_PROFILE: &str = "nedonews_default";

#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub key: String,
    pub identity: String,
    pub display_name: String,
    pub community_context: String,
    pub system_prompt: String,
    pub response_style: String,
    pub capability_document: Option<String>,
    pub model_route: String,
    pub tool_allowlist: Vec<String>,
    pub budgets: AgentBudgets,
    pub memory: AgentMemoryConfig,
}

impl AgentProfile {
    pub fn resolve(
        profiles: &LlmProfiles,
        profile_path: &str,
        community_id: &str,
        requested_key: Option<&str>,
    ) -> anyhow::Result<Self> {
        let legacy_nedonews_default = requested_key.is_none() && community_id == "nedonews";
        let key = requested_key
            .map(str::to_owned)
            .or_else(|| legacy_nedonews_default.then(|| NEDONEWS_DEFAULT_PROFILE.to_owned()))
            .ok_or_else(|| {
                anyhow::anyhow!("community ask.agent_profile must select a named agent profile")
            })?;

        if legacy_nedonews_default && !profiles.agent_profiles.contains_key(&key) {
            return Self::legacy_nedonews(profile_path, profiles.runtime.ask_total_timeout_sec);
        }
        let profile = profiles
            .agent_profiles
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("selected agent profile {key:?} is not configured"))?;
        let allow_nedonews_builtins = community_id == "nedonews" && key == NEDONEWS_DEFAULT_PROFILE;
        let base_dir = profile_base_dir(profile_path)?;
        let system_prompt = load_prompt_source(
            &profile.system_prompt_file,
            &base_dir,
            allow_nedonews_builtins,
            "system prompt",
        )?;
        let capability_document = profile
            .capability_document_file
            .as_deref()
            .map(|source| {
                load_prompt_source(
                    source,
                    &base_dir,
                    allow_nedonews_builtins,
                    "capability document",
                )
            })
            .transpose()?;

        Ok(Self {
            key,
            identity: profile.identity.clone(),
            display_name: profile.display_name.clone(),
            community_context: profile.community_context.clone(),
            system_prompt,
            response_style: profile.response_style.clone(),
            capability_document,
            model_route: profile.model_route.clone(),
            tool_allowlist: profile.tool_allowlist.clone(),
            budgets: profile.budgets.clone(),
            memory: profile.memory.clone(),
        })
    }

    fn legacy_nedonews(profile_path: &str, total_timeout_sec: u64) -> anyhow::Result<Self> {
        let base_dir = profile_base_dir(profile_path)?;
        Ok(Self {
            key: NEDONEWS_DEFAULT_PROFILE.to_owned(),
            identity: "NedoBot".to_owned(),
            display_name: "NedoBot".to_owned(),
            community_context: "НедоNews Chat (разрешена только его история)".to_owned(),
            system_prompt: load_prompt_source(
                "builtin:nedonews_ask",
                &base_dir,
                true,
                "system prompt",
            )?,
            response_style: String::new(),
            capability_document: Some(load_prompt_source(
                "builtin:nedonews_changelog",
                &base_dir,
                true,
                "capability document",
            )?),
            model_route: "ask".to_owned(),
            tool_allowlist: crate::features::ask::mcp_client::compiled_agent_tool_names(),
            budgets: AgentBudgets {
                max_steps: 64,
                max_tool_calls: 64,
                action_timeout_sec: 180,
                total_timeout_sec,
                max_output_tokens: 16_384,
            },
            memory: AgentMemoryConfig {
                enabled: false,
                write_policy: AgentMemoryWritePolicy::ExplicitOnly,
                ..AgentMemoryConfig::default()
            },
        })
    }
}

fn profile_base_dir(profile_path: &str) -> anyhow::Result<PathBuf> {
    let absolute = std::fs::canonicalize(profile_path)
        .with_context(|| format!("cannot resolve LLM profile file {profile_path:?}"))?;
    absolute
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("LLM profile file has no parent directory"))
}

fn load_prompt_source(
    source: &str,
    base_dir: &Path,
    allow_nedonews_builtins: bool,
    kind: &str,
) -> anyhow::Result<String> {
    let content = match source {
        "builtin:nedonews_ask" if allow_nedonews_builtins => {
            include_str!("../../../prompts/ask.md").to_owned()
        }
        "builtin:nedonews_changelog" if allow_nedonews_builtins => {
            include_str!("../../../docs/BOT_CHANGELOG.md").to_owned()
        }
        value if value.starts_with("builtin:") => {
            bail!("{kind} uses a built-in resource unavailable to this community")
        }
        value => {
            let path = Path::new(value);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                base_dir.join(path)
            };
            let metadata = std::fs::metadata(&path)
                .with_context(|| format!("{kind} file is missing or unreadable: {path:?}"))?;
            ensure!(metadata.is_file(), "{kind} path is not a file: {path:?}");
            ensure!(
                metadata.len() <= MAX_PROMPT_FILE_BYTES,
                "{kind} file exceeds the {} byte limit",
                MAX_PROMPT_FILE_BYTES
            );
            std::fs::read_to_string(&path)
                .with_context(|| format!("{kind} file is not valid UTF-8: {path:?}"))?
        }
    };
    ensure!(!content.trim().is_empty(), "{kind} must not be empty");
    ensure!(!content.contains('\0'), "{kind} contains a NUL byte");
    Ok(content)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn legacy_agent_uses_the_configured_total_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("profile.toml");
        fs::write(&path, "# fixture").unwrap();
        let mut profiles =
            LlmProfiles::from_toml(include_str!("../../../config/llm_profiles.toml.example"))
                .unwrap();
        profiles.agent_profiles.clear();
        profiles.runtime.ask_total_timeout_sec = 2345;
        let profile =
            AgentProfile::resolve(&profiles, path.to_str().unwrap(), "nedonews", None).unwrap();
        assert_eq!(profile.budgets.total_timeout_sec, 2345);
    }

    #[test]
    fn named_profiles_resolve_independent_prompts_routes_and_identities() {
        let temp = tempfile::tempdir().unwrap();
        let profile_path = temp.path().join("llm_profiles.toml");
        fs::write(&profile_path, "# runtime profile fixture\n").unwrap();
        fs::write(temp.path().join("one.md"), "trusted prompt one").unwrap();
        fs::write(temp.path().join("two.md"), "trusted prompt two").unwrap();
        let profiles = synthetic_profiles();

        let one = AgentProfile::resolve(
            &profiles,
            profile_path.to_str().unwrap(),
            "community_a",
            Some("agent_one"),
        )
        .unwrap();
        let two = AgentProfile::resolve(
            &profiles,
            profile_path.to_str().unwrap(),
            "community_b",
            Some("agent_two"),
        )
        .unwrap();

        assert_eq!(one.identity, "identity one");
        assert_eq!(one.system_prompt, "trusted prompt one");
        assert_eq!(one.model_route, "route_one");
        assert_eq!(two.identity, "identity two");
        assert_eq!(two.system_prompt, "trusted prompt two");
        assert_eq!(two.model_route, "route_two");
    }

    #[test]
    fn missing_private_prompt_fails_closed_and_never_uses_another_profile() {
        let temp = tempfile::tempdir().unwrap();
        let profile_path = temp.path().join("llm_profiles.toml");
        fs::write(&profile_path, "# runtime profile fixture\n").unwrap();
        let profiles = synthetic_profiles();
        let mut config = profiles.agent_profiles["agent_one"].clone();
        config.system_prompt_file = "missing-private-prompt.md".to_owned();
        let mut profiles = profiles;
        profiles
            .agent_profiles
            .insert("agent_one".to_owned(), config);

        let error = AgentProfile::resolve(
            &profiles,
            profile_path.to_str().unwrap(),
            "community_a",
            Some("agent_one"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("system prompt file is missing"));
        assert!(
            AgentProfile::resolve(
                &profiles,
                profile_path.to_str().unwrap(),
                "community_b",
                Some("agent_missing"),
            )
            .is_err()
        );
    }

    #[test]
    fn only_legacy_nedonews_selection_can_use_compiled_compatibility_resources() {
        let temp = tempfile::tempdir().unwrap();
        let profile_path = temp.path().join("llm_profiles.toml");
        fs::write(&profile_path, "# runtime profile fixture\n").unwrap();
        let profiles = synthetic_profiles();

        let legacy =
            AgentProfile::resolve(&profiles, profile_path.to_str().unwrap(), "nedonews", None)
                .unwrap();
        assert!(legacy.system_prompt.contains("НедоNews Chat"));
        assert!(legacy.capability_document.is_some());
        assert!(
            AgentProfile::resolve(
                &profiles,
                profile_path.to_str().unwrap(),
                "community_a",
                None,
            )
            .is_err()
        );
    }

    fn synthetic_profiles() -> LlmProfiles {
        let toml = r#"
            [providers.local]
            driver = "ollama_native"
            base_url = "http://127.0.0.1:11434"
            api_key_env = "LOCAL_KEY"

            [models.agent]
            provider = "local"
            model = "synthetic-model"

            [models.agent.capabilities]
            supports_images = true
            supports_tools = true
            supports_system_prompt = true
            structured_output = "prompt_only"
            context_window_tokens = 8192
            max_output_tokens = 2048
            request_timeout_sec = 30
            thinking = "none"

            [routes.route_one]
            models = ["agent"]

            [routes.route_two]
            models = ["agent"]

            [agent_profiles.agent_one]
            identity = "identity one"
            display_name = "Agent One"
            community_context = "Community A"
            system_prompt_file = "one.md"
            model_route = "route_one"
            tool_allowlist = ["chat.search_messages"]

            [agent_profiles.agent_one.budgets]
            max_steps = 8
            max_tool_calls = 8
            action_timeout_sec = 10
            total_timeout_sec = 30
            max_output_tokens = 1024

            [agent_profiles.agent_two]
            identity = "identity two"
            display_name = "Agent Two"
            community_context = "Community B"
            system_prompt_file = "two.md"
            model_route = "route_two"
            tool_allowlist = ["chat.search_messages"]

            [agent_profiles.agent_two.budgets]
            max_steps = 4
            max_tool_calls = 5
            action_timeout_sec = 20
            total_timeout_sec = 60
            max_output_tokens = 512

            [runtime]
        "#;
        LlmProfiles::from_toml(toml).unwrap()
    }
}
