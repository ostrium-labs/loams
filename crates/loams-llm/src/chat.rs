//! Chat with structured output.

use serde::{Deserialize, Serialize};

use crate::budget::{Budget, estimate_tokens};
use crate::config::ModelRef;
use crate::error::LlmError;
use crate::schema::JsonSchema;

/// Who said a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The caller's turn.
    User,
    /// The model's earlier turn.
    Assistant,
}

/// One message of a conversation. The system prompt is
/// [`ChatRequest::system`], not a message.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Who said it.
    pub role: Role,
    /// The text.
    pub content: String,
}

impl std::fmt::Debug for ChatMessage {
    // Content never reaches a log line, even through `{:?}`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatMessage")
            .field("role", &self.role)
            .field("content_len", &self.content.len())
            .finish()
    }
}

impl ChatMessage {
    /// A user message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    /// An assistant message.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// A structured-output call.
#[derive(Clone, PartialEq)]
pub struct ChatRequest {
    /// The system prompt.
    pub system: String,
    /// The conversation, oldest first.
    pub messages: Vec<ChatMessage>,
    /// The output cap; reserved in full against the budget.
    pub max_output_tokens: u32,
    /// Sampling temperature, where the provider is configured to send it.
    pub temperature: f32,
}

impl std::fmt::Debug for ChatRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatRequest")
            .field("system_len", &self.system.len())
            .field("messages", &self.messages)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("temperature", &self.temperature)
            .finish()
    }
}

impl ChatRequest {
    /// The tokens a call reserves: an estimate of the input (system,
    /// messages and schema) plus `max_output_tokens`.
    #[must_use]
    pub fn estimate(&self, schema: &JsonSchema) -> u64 {
        let input = estimate_tokens(&self.system)
            + self
                .messages
                .iter()
                .map(|m| estimate_tokens(&m.content) + 4)
                .sum::<u64>()
            + estimate_tokens(&schema.schema().to_string());
        input + u64::from(self.max_output_tokens)
    }
}

/// A validated structured answer.
#[derive(Clone, PartialEq)]
pub struct ChatOutput {
    /// The answer, valid against the call's schema.
    pub json: serde_json::Value,
    /// Input tokens the provider reported.
    pub input_tokens: u64,
    /// Output tokens the provider reported.
    pub output_tokens: u64,
    /// The model the provider says served the call.
    pub model: String,
}

impl std::fmt::Debug for ChatOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatOutput")
            .field("input_tokens", &self.input_tokens)
            .field("output_tokens", &self.output_tokens)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

/// Messages in, JSON valid against a schema out, with token counts.
#[async_trait::async_trait]
pub trait ChatModel: Send + Sync {
    /// The model this is.
    fn id(&self) -> &ModelRef;
    /// Reserves against `budget` before sending, validates the answer
    /// against `schema`, and settles the tokens the provider reports.
    async fn complete_json(
        &self,
        req: ChatRequest,
        schema: &JsonSchema,
        budget: &Budget,
    ) -> Result<ChatOutput, LlmError>;
}
