//! Reranking.

use crate::budget::{Budget, estimate_tokens};
use crate::error::LlmError;

/// A query and documents in, one relevance score per document out (in the
/// documents' order; higher is more relevant).
#[async_trait::async_trait]
pub trait Reranker: Send + Sync {
    /// One score per document, in the documents' order.
    async fn rerank(
        &self,
        query: &str,
        docs: &[String],
        budget: &Budget,
    ) -> Result<Vec<f32>, LlmError>;
}

/// The tokens a rerank call reserves: the query once per document plus
/// the documents.
#[must_use]
pub fn estimate_rerank(query: &str, docs: &[String]) -> u64 {
    docs.iter()
        .map(|d| estimate_tokens(query) + estimate_tokens(d) + 2)
        .sum()
}
