use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(String);

impl ProfileId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_client_id(client_id: String) -> Self {
        Self(client_id)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: Option<String>,
    pub token_type: String,
    pub scopes: Vec<String>,
    pub expires_at_unix: i64,
    pub earliest_refresh_at_unix: Option<i64>,
}

impl TokenSet {
    pub fn plan_usage_granted(&self) -> bool {
        ["resource.invoke", "chatgpt.tokens.use.direct"]
            .into_iter()
            .all(|required| self.scopes.iter().any(|scope| scope == required))
    }

    pub fn refresh_token(&self) -> &str {
        &self.refresh_token
    }
}

impl fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("id_token", &self.id_token.as_ref().map(|_| "[REDACTED]"))
            .field("token_type", &self.token_type)
            .field("scopes", &self.scopes)
            .field("expires_at_unix", &self.expires_at_unix)
            .field("earliest_refresh_at_unix", &self.earliest_refresh_at_unix)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChatGptProfile {
    pub id: ProfileId,
    pub client_id: String,
    pub host_id: String,
    pub issuer: String,
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub tokens: Option<TokenSet>,
}

impl ChatGptProfile {
    pub fn new_host_id() -> String {
        format!("urn:uuid:{}", Uuid::new_v4())
    }

    pub fn plan_usage_enabled(&self) -> bool {
        self.tokens
            .as_ref()
            .is_some_and(TokenSet::plan_usage_granted)
    }

    pub fn reconnect_id_token_hint(&self) -> Option<&str> {
        self.tokens.as_ref()?.id_token.as_deref()
    }
}

impl fmt::Debug for ChatGptProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatGptProfile")
            .field("id", &self.id)
            .field("client_id", &self.client_id)
            .field("host_id", &self.host_id)
            .field("issuer", &self.issuer)
            .field("subject", &self.subject)
            .field("email", &self.email)
            .field("display_name", &self.display_name)
            .field("tokens", &self.tokens.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_set(scopes: &[&str]) -> TokenSet {
        TokenSet {
            access_token: "access".to_owned(),
            refresh_token: "refresh".to_owned(),
            id_token: None,
            token_type: "Bearer".to_owned(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            expires_at_unix: 1_800_000_000,
            earliest_refresh_at_unix: None,
        }
    }

    #[test]
    fn chatgpt_plan_usage_requires_both_direct_and_resource_scopes() {
        assert!(token_set(&["resource.invoke", "chatgpt.tokens.use.direct"]).plan_usage_granted());
        assert!(!token_set(&["chatgpt.tokens.use.direct"]).plan_usage_granted());
        assert!(!token_set(&["resource.invoke"]).plan_usage_granted());
    }
}
