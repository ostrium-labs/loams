//! The one error type of the crate.
//!
//! No variant carries content: prompts, model outputs, keys and provider
//! error bodies never reach an error message (the plan's "no content in
//! logs" rule). `InvalidOutput` names a JSON pointer and a schema keyword,
//! never the offending value.

/// A model call's failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LlmError {
    /// No model, no key, or a key reference that does not resolve
    /// (`memory_model_not_configured`).
    #[error("model not configured")]
    NotConfigured,
    /// `local_only` refused a provider that is not loopback or allow-listed
    /// (`memory_model_not_local`).
    #[error("model provider is not local")]
    NotLocal,
    /// A budget refused the call before it was sent
    /// (`memory_token_budget_exhausted`).
    #[error("token budget exhausted")]
    BudgetExhausted,
    /// The model answered, but not with JSON valid against the schema. The
    /// text is a pointer and a keyword (`/facts/0/kind: enum`), or a fixed
    /// description.
    #[error("invalid model output: {0}")]
    InvalidOutput(String),
    /// The provider answered with an error status. The body is discarded.
    #[error("provider answered {status}")]
    Provider {
        /// The HTTP status.
        status: u16,
        /// Whether a later attempt may succeed (408, 429, 5xx).
        retryable: bool,
    },
    /// The request did not complete (connect, timeout, body).
    #[error("transport: {0}")]
    Transport(String),
    /// A configuration the crate refuses (a base URL with userinfo, plain
    /// `http` off loopback, an invalid schema). Fixed descriptions only
    /// (ruling R2.2).
    #[error("invalid model configuration: {0}")]
    InvalidConfig(String),
}

impl LlmError {
    /// Whether a later attempt may succeed (rate limits, server errors,
    /// transport failures).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            LlmError::Provider { retryable, .. } => *retryable,
            LlmError::Transport(_) => true,
            _ => false,
        }
    }
}
