//! Loams' model providers (design §52 §10, D887; MM1 Task 2).
//!
//! Three traits: [`ChatModel`] (messages in, JSON valid against a schema
//! out, with token counts), [`Embedder`] (texts in, fixed-dimension vectors
//! out) and [`Reranker`] (query and texts in, scores out). Providers:
//! [`OpenAiCompatible`] (OpenAI, DeepSeek, vLLM, LM Studio, Ollama),
//! [`Anthropic`] (Messages, structured output through a tool) and the
//! deterministic fakes in [`fake`]. Raw HTTP over `reqwest`, no vendor SDK.
//!
//! Every call reserves against a [`Budget`] before anything is sent. A
//! store's [`LocalPolicy`] refuses providers that are not loopback or in its
//! private allow-list. Keys are [`SecretRef`]s resolved through a
//! [`SecretSource`] into [`Secret`]s, which never print. Prompts, outputs,
//! keys and provider error bodies never reach a log line, a span or an
//! error message.

#![warn(missing_docs)]

pub mod anthropic;
pub mod budget;
pub mod chat;
pub mod config;
pub mod embed;
pub mod error;
pub mod fake;
mod http;
pub mod openai;
pub mod rerank;
pub mod schema;

use std::sync::Arc;

pub use anthropic::{Anthropic, AnthropicOptions, ToolChoice};
pub use budget::{Budget, Clock, DailyBudget, ManualClock, Reservation, RunBudget, SystemClock};
pub use chat::{ChatMessage, ChatModel, ChatOutput, ChatRequest, Role};
pub use config::{
    Cidr, EnvSecretSource, LocalPolicy, ModelRef, ProviderKind, Secret, SecretRef, SecretSource,
    StaticSecrets,
};
pub use embed::Embedder;
pub use error::LlmError;
pub use fake::{FakeChat, FakeEmbedder, FakeReranker};
pub use http::{HttpOptions, RetryPolicy};
pub use openai::{MaxTokensField, OpenAiCompatible, OpenAiOptions, StructuredMode};
pub use rerank::Reranker;
pub use schema::JsonSchema;

/// A chat model for `id` with each provider's default options. Fakes are
/// built from fixtures ([`FakeChat`]), not here.
pub async fn connect_chat(
    id: &ModelRef,
    policy: &LocalPolicy,
    secrets: &dyn SecretSource,
) -> Result<Arc<dyn ChatModel>, LlmError> {
    match id.provider {
        ProviderKind::OpenAiCompatible => Ok(Arc::new(
            OpenAiCompatible::connect(id.clone(), policy, secrets, OpenAiOptions::default())
                .await?,
        )),
        ProviderKind::Anthropic => Ok(Arc::new(
            Anthropic::connect(id.clone(), policy, secrets, AnthropicOptions::default()).await?,
        )),
        ProviderKind::Fake => Err(LlmError::InvalidConfig(
            "fake models are built from fixtures".to_string(),
        )),
    }
}

/// An embedder of dimension `dim` for `id`. Anthropic has no embeddings.
pub async fn connect_embedder(
    id: &ModelRef,
    dim: usize,
    policy: &LocalPolicy,
    secrets: &dyn SecretSource,
) -> Result<Arc<dyn Embedder>, LlmError> {
    match id.provider {
        ProviderKind::OpenAiCompatible => {
            let opts = OpenAiOptions {
                embedding_dim: Some(dim),
                ..OpenAiOptions::default()
            };
            Ok(Arc::new(
                OpenAiCompatible::connect(id.clone(), policy, secrets, opts).await?,
            ))
        }
        ProviderKind::Anthropic => Err(LlmError::NotConfigured),
        ProviderKind::Fake => Err(LlmError::InvalidConfig(
            "fake models are built from fixtures".to_string(),
        )),
    }
}
