use std::collections::HashMap;

use crate::config_file::{ChatConfig, CommunityConfig};

#[derive(Debug, Clone)]
pub struct ChatRef<'a> {
    pub key: String,
    pub config: &'a ChatConfig,
}

#[derive(Debug, Clone)]
pub struct ChatRegistry {
    by_id: HashMap<i64, String>,
}

impl ChatRegistry {
    pub fn new(community: &CommunityConfig) -> anyhow::Result<Self> {
        let mut by_id = HashMap::with_capacity(community.chats.len());
        for (key, chat) in &community.chats {
            if key.trim().is_empty() {
                anyhow::bail!("managed chat key must not be empty");
            }
            if by_id.insert(chat.id, key.clone()).is_some() {
                anyhow::bail!("managed chat IDs must be unique; duplicate ID {}", chat.id);
            }
        }
        Ok(Self { by_id })
    }

    pub fn chat_by_id<'a>(
        &'a self,
        community: &'a CommunityConfig,
        id: i64,
    ) -> Option<ChatRef<'a>> {
        let key = self.by_id.get(&id)?;
        let config = community.chats.get(key)?;
        Some(ChatRef {
            key: key.clone(),
            config,
        })
    }

    pub fn chat_by_key<'a>(
        &self,
        community: &'a CommunityConfig,
        key: &str,
    ) -> Option<ChatRef<'a>> {
        let config = community.chats.get(key)?;
        Some(ChatRef {
            key: key.to_string(),
            config,
        })
    }

    pub fn managed_chat_ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.by_id.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::config_file::{
        AskConfig, FirstCommentConfig, InstanceConfig, ModerationConfig, PublicMcpConfig,
        SpamReputationConfig, TelegramConfig, UnknownChatPolicy, VoiceConfig,
    };

    fn community(chats: BTreeMap<String, ChatConfig>) -> CommunityConfig {
        CommunityConfig {
            instance: InstanceConfig {
                id: "test".into(),
                display_name: "Test".into(),
                timezone: "Europe/Moscow".into(),
            },
            telegram: TelegramConfig {
                unknown_chat_policy: UnknownChatPolicy::Ignore,
                owners: vec![],
            },
            chats,
            moderation: ModerationConfig::default(),
            spam_reputation: SpamReputationConfig::default(),
            voice: VoiceConfig::default(),
            ask: AskConfig::default(),
            first_comment: FirstCommentConfig::default(),
            public_mcp: PublicMcpConfig::default(),
            risk_profiles: BTreeMap::new(),
        }
    }

    #[test]
    fn indexes_managed_chat_ids_without_multi_tenant_state() {
        let chats = BTreeMap::from([(
            "general".into(),
            ChatConfig {
                id: -1001,
                ingest: true,
                moderation: true,
                stats: true,
                voice: false,
                ask: false,
                review_destination: false,
                invite_url_env: None,
                invite_label: None,
            },
        )]);
        let community = community(chats);
        let registry = ChatRegistry::new(&community).unwrap();

        assert_eq!(
            registry.chat_by_id(&community, -1001).unwrap().key,
            "general"
        );
        assert_eq!(
            registry
                .chat_by_key(&community, "general")
                .unwrap()
                .config
                .id,
            -1001
        );
        assert_eq!(registry.managed_chat_ids().collect::<Vec<_>>(), vec![-1001]);
    }

    #[test]
    fn rejects_duplicate_chat_ids() {
        let chats = BTreeMap::from([
            (
                "one".into(),
                ChatConfig {
                    id: -1001,
                    ingest: true,
                    moderation: false,
                    stats: false,
                    voice: false,
                    ask: false,
                    review_destination: false,
                    invite_url_env: None,
                    invite_label: None,
                },
            ),
            (
                "two".into(),
                ChatConfig {
                    id: -1001,
                    ingest: true,
                    moderation: false,
                    stats: false,
                    voice: false,
                    ask: false,
                    review_destination: false,
                    invite_url_env: None,
                    invite_label: None,
                },
            ),
        ]);

        let error = ChatRegistry::new(&community(chats))
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be unique"));
    }
}
