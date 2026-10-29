//! `knowledge_search`: the manifest's paths, and a small in-memory index
//! of its entries' text, built from verified reads.

use std::collections::HashMap;

use serde::Serialize;

use crate::manifest::VerifiedManifest;

/// The most hits one search returns.
const MAX_LIMIT: usize = 50;
/// A snippet's longest length, in characters.
const SNIPPET_CHARS: usize = 200;

/// The verified text of a manifest's entries.
#[derive(Debug, Default)]
pub(crate) struct TextIndex {
    /// Path, text for snippets, and lowercased text with the same offsets.
    docs: HashMap<String, (String, String)>,
    unindexed: usize,
}

impl TextIndex {
    pub(crate) fn add(&mut self, path: String, text: String) {
        let lower = text.to_lowercase();
        // Snippets come from the original only where lowercasing kept the
        // byte length; otherwise from the lowercased text itself.
        let text = if lower.len() == text.len() {
            text
        } else {
            lower.clone()
        };
        self.docs.insert(path, (text, lower));
    }

    pub(crate) fn skip(&mut self) {
        self.unindexed += 1;
    }

    fn text(&self, path: &str) -> Option<(&str, &str)> {
        self.docs.get(path).map(|(t, l)| (t.as_str(), l.as_str()))
    }
}

/// One search hit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    /// The entry's path.
    pub path: String,
    /// Its size.
    pub bytes: u64,
    /// Higher is better: 10 per query term in the path, 1 per occurrence in
    /// the text (at most 20 per term).
    pub score: u64,
    /// The first line holding a query term, if the text matched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// A search's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResults {
    /// The repository.
    pub repo: String,
    /// The commit searched.
    pub commit: String,
    /// The best hits, best first.
    pub hits: Vec<SearchHit>,
    /// Entries whose text was searched.
    pub indexed: usize,
    /// Entries left out because they could not be read or verified.
    pub unindexed: usize,
}

pub(crate) fn search(
    m: &VerifiedManifest,
    index: &TextIndex,
    query: &str,
    limit: usize,
) -> SearchResults {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|t| !t.is_empty())
        .collect();
    let mut hits = Vec::new();
    if !terms.is_empty() {
        for e in &m.entries {
            let path = e.path.to_lowercase();
            let mut score = 0u64;
            for t in &terms {
                if path.contains(t.as_str()) {
                    score += 10;
                }
            }
            let mut snippet = None;
            if let Some((text, lower)) = index.text(&e.path) {
                for t in &terms {
                    let n = lower.matches(t.as_str()).take(20).count() as u64;
                    score += n;
                    if n > 0 && snippet.is_none() {
                        snippet = snippet_of(text, lower, t);
                    }
                }
            }
            if score > 0 {
                hits.push(SearchHit {
                    path: e.path.clone(),
                    bytes: e.bytes,
                    score,
                    snippet,
                });
            }
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(limit.clamp(1, MAX_LIMIT));
    SearchResults {
        repo: m.repo.clone(),
        commit: m.commit.clone(),
        hits,
        indexed: index.docs.len(),
        unindexed: index.unindexed,
    }
}

/// The line of `text` holding the first occurrence of `term` (found in
/// `lower`, which has the same byte offsets).
fn snippet_of(text: &str, lower: &str, term: &str) -> Option<String> {
    let at = lower.find(term)?;
    let start = text.get(..at)?.rfind('\n').map_or(0, |i| i + 1);
    let end = text.get(at..)?.find('\n').map_or(text.len(), |i| at + i);
    let line: String = text
        .get(start..end)?
        .trim()
        .chars()
        .take(SNIPPET_CHARS)
        .collect();
    Some(line)
}
