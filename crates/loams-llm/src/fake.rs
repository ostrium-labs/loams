//! Deterministic fakes for tests: `FakeChat`, `FakeEmbedder` and
//! `FakeReranker`, scripted by JSON fixture files. They make no network
//! call, honour budgets exactly as the providers do, and validate answers
//! against the schema, so a scripted invalid answer fails as a real one.
//!
//! # `FakeChat` fixture
//!
//! ```json
//! { "model": "fake-chat",
//!   "rules": [
//!     { "when": { "system_contains": "extract", "user_contains": "Berlin" },
//!       "times": 1,
//!       "answer": { "facts": [] },
//!       "input_tokens": 120, "output_tokens": 8 },
//!     { "text": "not json" },
//!     { "error": { "status": 503, "retryable": true } } ] }
//! ```
//!
//! A call takes the first rule whose `when` matches (every given condition;
//! `user_contains` matches any user message) and that has uses left
//! (`times`, unlimited when absent). A rule answers `answer` (a JSON value,
//! validated), `text` (parsed as the instruction fallback parses it) or
//! `error` (a provider error). Token counts default to the request's input
//! estimate and the answer's length.
//!
//! # `FakeEmbedder` fixture
//!
//! `{ "model": "fake-embed", "dim": 64, "seed": 7, "vectors": { "text": [ … ] } }`.
//! A text in `vectors` gets that vector; any other text gets a seeded hash
//! projection of its words, L2-normalised, so texts that share words are
//! close.
//!
//! # `FakeReranker` fixture
//!
//! `{ "model": "fake-rerank", "scores": [ { "query": "q", "doc": "d", "score": 0.9 } ] }`.
//! Other pairs score the share of the query's words found in the document.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::budget::Budget;
use crate::chat::{ChatModel, ChatOutput, ChatRequest, Role};
use crate::config::ModelRef;
use crate::embed::{Embedder, estimate_embed};
use crate::error::LlmError;
use crate::rerank::{Reranker, estimate_rerank};
use crate::schema::JsonSchema;

fn read_fixture<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, LlmError> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| LlmError::InvalidConfig("fixture file cannot be read".to_string()))?;
    serde_json::from_str(&text)
        .map_err(|_| LlmError::InvalidConfig("fixture file is not a valid fixture".to_string()))
}

/// The lower-cased alphanumeric words of `text`.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct When {
    system_contains: Option<String>,
    user_contains: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptedError {
    status: u16,
    #[serde(default)]
    retryable: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    #[serde(default)]
    when: When,
    times: Option<u32>,
    answer: Option<Value>,
    text: Option<String>,
    error: Option<ScriptedError>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatFixture {
    model: String,
    rules: Vec<Rule>,
}

/// A scripted chat model.
#[derive(Debug)]
pub struct FakeChat {
    id: ModelRef,
    rules: Vec<Rule>,
    used: Mutex<Vec<u32>>,
    calls: AtomicU64,
}

impl FakeChat {
    /// Loads a fixture file.
    pub fn from_fixture_file(path: impl AsRef<Path>) -> Result<Self, LlmError> {
        Self::from_fixture(read_fixture(path.as_ref())?)
    }

    /// Parses a fixture.
    pub fn from_json(json: &str) -> Result<Self, LlmError> {
        Self::from_fixture(
            serde_json::from_str(json)
                .map_err(|_| LlmError::InvalidConfig("not a valid fixture".to_string()))?,
        )
    }

    fn from_fixture(f: ChatFixture) -> Result<Self, LlmError> {
        for r in &f.rules {
            let outcomes = usize::from(r.answer.is_some())
                + usize::from(r.text.is_some())
                + usize::from(r.error.is_some());
            if outcomes != 1 {
                return Err(LlmError::InvalidConfig(
                    "a rule has exactly one of answer, text, error".to_string(),
                ));
            }
        }
        Ok(Self {
            id: ModelRef::fake(&f.model),
            used: Mutex::new(vec![0; f.rules.len()]),
            rules: f.rules,
            calls: AtomicU64::new(0),
        })
    }

    /// Calls that reached the script (budget refusals do not).
    #[must_use]
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    fn pick(&self, req: &ChatRequest) -> Option<Rule> {
        let mut used = self
            .used
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let idx = self.rules.iter().enumerate().position(|(i, r)| {
            let left = r.times.is_none_or(|t| used[i] < t);
            let sys = r
                .when
                .system_contains
                .as_ref()
                .is_none_or(|s| req.system.contains(s.as_str()));
            let user = r.when.user_contains.as_ref().is_none_or(|s| {
                req.messages
                    .iter()
                    .any(|m| m.role == Role::User && m.content.contains(s.as_str()))
            });
            left && sys && user
        })?;
        used[idx] += 1;
        Some(self.rules[idx].clone())
    }
}

#[async_trait::async_trait]
impl ChatModel for FakeChat {
    fn id(&self) -> &ModelRef {
        &self.id
    }

    async fn complete_json(
        &self,
        req: ChatRequest,
        schema: &JsonSchema,
        budget: &Budget,
    ) -> Result<ChatOutput, LlmError> {
        let estimate = req.estimate(schema);
        let reservation = budget.reserve(estimate)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        let rule = self.pick(&req).ok_or_else(|| {
            LlmError::InvalidConfig("no scripted answer matches the request".to_string())
        })?;
        if let Some(e) = rule.error {
            return Err(LlmError::Provider {
                status: e.status,
                retryable: e.retryable,
            });
        }
        let raw_len = match (&rule.answer, &rule.text) {
            (Some(a), _) => a.to_string().len(),
            (_, Some(t)) => t.len(),
            _ => 0,
        };
        let input_tokens = rule
            .input_tokens
            .unwrap_or_else(|| estimate - u64::from(req.max_output_tokens));
        let output_tokens = rule
            .output_tokens
            .unwrap_or_else(|| (raw_len as u64).div_ceil(4));
        reservation.settle(input_tokens + output_tokens);
        let json = match (rule.answer, rule.text) {
            (Some(a), _) => {
                schema.validate(&a)?;
                a
            }
            (_, Some(t)) => schema.parse_text(&t)?,
            _ => return Err(LlmError::InvalidOutput("empty scripted answer".to_string())),
        };
        Ok(ChatOutput {
            json,
            input_tokens,
            output_tokens,
            model: self.id.model.clone(),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmbedFixture {
    model: String,
    dim: usize,
    #[serde(default)]
    seed: u64,
    #[serde(default)]
    vectors: HashMap<String, Vec<f32>>,
}

/// A deterministic embedder: a seeded hash projection of the words.
#[derive(Debug, Clone)]
pub struct FakeEmbedder {
    id: ModelRef,
    dim: usize,
    seed: u64,
    vectors: HashMap<String, Vec<f32>>,
}

impl FakeEmbedder {
    /// `dim` must be at least 1.
    pub fn new(model: &str, dim: usize, seed: u64) -> Result<Self, LlmError> {
        if dim == 0 {
            return Err(LlmError::InvalidConfig(
                "dimension must be at least 1".to_string(),
            ));
        }
        Ok(Self {
            id: ModelRef::fake(model),
            dim,
            seed,
            vectors: HashMap::new(),
        })
    }

    /// Loads a fixture file.
    pub fn from_fixture_file(path: impl AsRef<Path>) -> Result<Self, LlmError> {
        let f: EmbedFixture = read_fixture(path.as_ref())?;
        let mut e = Self::new(&f.model, f.dim, f.seed)?;
        if f.vectors.values().any(|v| v.len() != f.dim) {
            return Err(LlmError::InvalidConfig(
                "a fixture vector has the wrong dimension".to_string(),
            ));
        }
        e.vectors = f.vectors;
        Ok(e)
    }

    /// The vector of one text.
    #[must_use]
    pub fn vector(&self, text: &str) -> Vec<f32> {
        if let Some(v) = self.vectors.get(text) {
            return v.clone();
        }
        let mut v = vec![0f32; self.dim];
        let mut any = false;
        for w in words(text) {
            any = true;
            self.project(&w, &mut v);
        }
        if !any {
            self.project(text, &mut v);
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            v.iter_mut().for_each(|x| *x /= norm);
        }
        v
    }

    /// Adds ±1 at two hashed positions for `token`.
    fn project(&self, token: &str, v: &mut [f32]) {
        let mut h = Sha256::new();
        h.update(self.seed.to_le_bytes());
        h.update(token.as_bytes());
        let d = h.finalize();
        for chunk in d.chunks_exact(8).take(2) {
            let mut b = [0u8; 8];
            b.copy_from_slice(chunk);
            let n = u64::from_le_bytes(b);
            let idx = usize::try_from(n % self.dim as u64).unwrap_or(0);
            let sign = if n >> 63 == 0 { 1.0 } else { -1.0 };
            v[idx] += sign;
        }
    }
}

#[async_trait::async_trait]
impl Embedder for FakeEmbedder {
    fn id(&self) -> &ModelRef {
        &self.id
    }

    fn dim(&self) -> usize {
        self.dim
    }

    async fn embed(&self, texts: &[String], budget: &Budget) -> Result<Vec<Vec<f32>>, LlmError> {
        let estimate = estimate_embed(texts);
        budget.reserve(estimate)?.settle(estimate);
        Ok(texts.iter().map(|t| self.vector(t)).collect())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreRow {
    query: String,
    doc: String,
    score: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RerankFixture {
    model: String,
    #[serde(default)]
    scores: Vec<ScoreRow>,
}

/// A deterministic reranker: scripted scores, else word overlap.
#[derive(Debug, Clone)]
pub struct FakeReranker {
    id: ModelRef,
    scores: HashMap<(String, String), f32>,
}

impl FakeReranker {
    /// A reranker with no scripted scores.
    #[must_use]
    pub fn new(model: &str) -> Self {
        Self {
            id: ModelRef::fake(model),
            scores: HashMap::new(),
        }
    }

    /// Loads a fixture file.
    pub fn from_fixture_file(path: impl AsRef<Path>) -> Result<Self, LlmError> {
        let f: RerankFixture = read_fixture(path.as_ref())?;
        let mut r = Self::new(&f.model);
        r.scores = f
            .scores
            .into_iter()
            .map(|s| ((s.query, s.doc), s.score))
            .collect();
        Ok(r)
    }

    /// The model this is.
    #[must_use]
    pub fn id(&self) -> &ModelRef {
        &self.id
    }

    fn score(&self, query: &str, doc: &str) -> f32 {
        if let Some(s) = self.scores.get(&(query.to_string(), doc.to_string())) {
            return *s;
        }
        let q: BTreeSet<String> = words(query).collect();
        if q.is_empty() {
            return 0.0;
        }
        let d: BTreeSet<String> = words(doc).collect();
        q.intersection(&d).count() as f32 / q.len() as f32
    }
}

#[async_trait::async_trait]
impl Reranker for FakeReranker {
    async fn rerank(
        &self,
        query: &str,
        docs: &[String],
        budget: &Budget,
    ) -> Result<Vec<f32>, LlmError> {
        let estimate = estimate_rerank(query, docs);
        budget.reserve(estimate)?.settle(estimate);
        Ok(docs.iter().map(|d| self.score(query, d)).collect())
    }
}
