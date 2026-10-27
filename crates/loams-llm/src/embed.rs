//! Embeddings.

use crate::budget::{Budget, estimate_tokens};
use crate::config::ModelRef;
use crate::error::LlmError;

/// Texts in, vectors of a fixed dimension out.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// The model this is.
    fn id(&self) -> &ModelRef;
    /// The dimension of every vector.
    fn dim(&self) -> usize;
    /// One vector per text, in order. A vector of another dimension is
    /// [`LlmError::InvalidOutput`].
    async fn embed(&self, texts: &[String], budget: &Budget) -> Result<Vec<Vec<f32>>, LlmError>;
}

/// The tokens an embedding call reserves.
#[must_use]
pub fn estimate_embed(texts: &[String]) -> u64 {
    texts.iter().map(|t| estimate_tokens(t) + 1).sum()
}
