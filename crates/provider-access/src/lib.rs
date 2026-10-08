//! Общие средства подключения к провайдерам для локальных и self-hosted агентских клиентов.
//!
//! Учётные данные провайдеров не связаны с Telegram или состоянием конкретного приложения.

mod error;
mod model;
mod oauth;
mod openai;
mod openrouter;
mod profile;
mod responses;
mod store;

#[cfg(feature = "genai")]
pub mod genai_adapter;

pub use error::{ProviderError, ResponsesFailure, Result};
pub use model::{
    ModelDescriptor, ModelProvider, ModelQuery, ModelSelector, ModelSort, Pricing, SelectedModel,
};
pub use oauth::{
    AuthorizationAttempt, AuthorizationStart, CallbackResult, LoopbackListener, OAuthClientMetadata,
};
pub use openai::{
    ChatGptPlanStatus, OpenAiModel, OpenAiSiwc, OpenAiSiwcConfig, classify_responses_failure,
};
pub use openrouter::{OpenRouterFreeConfig, OpenRouterFreeProvider, OpenRouterModel};
pub use profile::{ChatGptProfile, ProfileId, TokenSet};
pub use responses::{ResponsesEvent, ResponsesRequest, ResponsesStream};
pub use store::{CredentialStore, FileCredentialStore, MemoryCredentialStore};
