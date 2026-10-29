//! OpenAI-compatible HTTP: chat completions, embeddings and a `/rerank`
//! endpoint. Covers OpenAI, Azure OpenAI's v1 API, DeepSeek, vLLM,
//! LM Studio and Ollama (`http://127.0.0.1:11434/v1`).

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use tracing::Instrument;

use crate::budget::Budget;
use crate::chat::{ChatModel, ChatOutput, ChatRequest, Role};
use crate::config::{LocalPolicy, ModelRef, ProviderKind, Secret, SecretSource};
use crate::embed::{Embedder, estimate_embed};
use crate::error::LlmError;
use crate::http::{Http, HttpOptions, join};
use crate::rerank::{Reranker, estimate_rerank};
use crate::schema::JsonSchema;

const PROVIDER: &str = "openai_compatible";

/// How a chat call asks for structured output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StructuredMode {
    /// `response_format: {type: json_schema}` (OpenAI, vLLM, Ollama,
    /// LM Studio). The answer is still validated.
    #[default]
    JsonSchema,
    /// `response_format: {type: json_object}` plus the schema in the system
    /// prompt (DeepSeek), then validation.
    JsonObject,
    /// The schema in the system prompt only, then validation: for servers
    /// without `response_format`.
    Instruction,
}

/// Which field carries the output cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MaxTokensField {
    /// `max_tokens` (most compatible servers).
    #[default]
    MaxTokens,
    /// `max_completion_tokens` (OpenAI's current models).
    MaxCompletionTokens,
}

/// Options of an OpenAI-compatible provider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenAiOptions {
    /// How structured output is requested.
    pub structured: StructuredMode,
    /// Which field carries `max_output_tokens`.
    pub max_tokens_field: MaxTokensField,
    /// Send `temperature` (some reasoning models refuse it).
    pub send_temperature: bool,
    /// The embedding dimension, for an embedder. Every vector is checked
    /// against it.
    pub embedding_dim: Option<usize>,
    /// Transport options.
    pub http: HttpOptions,
}

impl Default for OpenAiOptions {
    fn default() -> Self {
        Self {
            structured: StructuredMode::JsonSchema,
            max_tokens_field: MaxTokensField::MaxTokens,
            send_temperature: true,
            embedding_dim: None,
            http: HttpOptions::default(),
        }
    }
}

/// The fixed instruction the fallback modes add to the system prompt.
pub(crate) fn schema_instruction(schema: &JsonSchema) -> String {
    format!(
        "\n\nReply with a single JSON value and nothing else. It must be valid against this JSON Schema:\n{}",
        schema.schema()
    )
}

/// An OpenAI-compatible server.
pub struct OpenAiCompatible {
    id: ModelRef,
    key: Option<Secret<String>>,
    http: Http,
    opts: OpenAiOptions,
}

impl std::fmt::Debug for OpenAiCompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatible")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl OpenAiCompatible {
    /// Checks the base URL and `policy`, resolves the key (if the model
    /// names one) and builds the client.
    pub async fn connect(
        id: ModelRef,
        policy: &LocalPolicy,
        secrets: &dyn SecretSource,
        opts: OpenAiOptions,
    ) -> Result<Self, LlmError> {
        if id.provider != ProviderKind::OpenAiCompatible {
            return Err(LlmError::InvalidConfig(
                "model is not OpenAI-compatible".to_string(),
            ));
        }
        id.check(policy)?;
        let key = match &id.key {
            Some(r) => Some(secrets.resolve(r).await?),
            None => None,
        };
        let http = Http::new(&id.base_url, opts.http)?;
        Ok(Self {
            id,
            key,
            http,
            opts,
        })
    }

    fn headers(&self) -> Result<HeaderMap, LlmError> {
        let mut h = HeaderMap::new();
        if let Some(k) = &self.key {
            let mut v = HeaderValue::from_str(&format!("Bearer {}", k.expose()))
                .map_err(|_| LlmError::InvalidConfig("key is not a header value".to_string()))?;
            v.set_sensitive(true);
            h.insert(AUTHORIZATION, v);
        }
        Ok(h)
    }

    fn chat_body(&self, req: &ChatRequest, schema: &JsonSchema) -> Value {
        let mut system = req.system.clone();
        if self.opts.structured != StructuredMode::JsonSchema {
            system.push_str(&schema_instruction(schema));
        }
        let mut messages = vec![json!({"role": "system", "content": system})];
        messages.extend(req.messages.iter().map(|m| {
            json!({
                "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                "content": m.content,
            })
        }));
        let mut body = json!({"model": self.id.model, "messages": messages});
        let field = match self.opts.max_tokens_field {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        };
        body[field] = json!(req.max_output_tokens);
        if self.opts.send_temperature {
            body["temperature"] = json!(req.temperature);
        }
        match self.opts.structured {
            StructuredMode::JsonSchema => {
                body["response_format"] = json!({
                    "type": "json_schema",
                    "json_schema": {
                        "name": schema.name(),
                        "schema": schema.schema(),
                        "strict": schema.strict(),
                    }
                });
            }
            StructuredMode::JsonObject => {
                body["response_format"] = json!({"type": "json_object"});
            }
            StructuredMode::Instruction => {}
        }
        body
    }
}

fn u64_at(v: &Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(Value::as_u64)
}

#[async_trait::async_trait]
impl ChatModel for OpenAiCompatible {
    fn id(&self) -> &ModelRef {
        &self.id
    }

    async fn complete_json(
        &self,
        req: ChatRequest,
        schema: &JsonSchema,
        budget: &Budget,
    ) -> Result<ChatOutput, LlmError> {
        let reservation = budget.reserve(req.estimate(schema))?;
        let span =
            tracing::debug_span!("llm.complete_json", provider = PROVIDER, model = %self.id.model);
        async move {
            let url = join(&self.id.base_url, "chat/completions")?;
            let body = self.chat_body(&req, schema);
            let answer = self
                .http
                .post_json(PROVIDER, &self.id.model, url, self.headers()?, &body)
                .await?;
            let input_tokens = u64_at(&answer, "/usage/prompt_tokens").unwrap_or(0);
            let output_tokens = u64_at(&answer, "/usage/completion_tokens").unwrap_or(0);
            let reported = input_tokens + output_tokens;
            // Tokens were spent whether or not the answer is valid.
            reservation.settle(if reported > 0 {
                reported
            } else {
                req.estimate(schema)
            });
            let message = answer
                .pointer("/choices/0/message")
                .ok_or_else(|| LlmError::InvalidOutput("no choice in the answer".to_string()))?;
            if message.get("refusal").is_some_and(|r| !r.is_null()) {
                return Err(LlmError::InvalidOutput("model refused".to_string()));
            }
            if answer
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                == Some("length")
            {
                return Err(LlmError::InvalidOutput(
                    "output cut off at max_output_tokens".to_string(),
                ));
            }
            let text = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| LlmError::InvalidOutput("no content in the answer".to_string()))?;
            let json = schema.parse_text(text)?;
            Ok(ChatOutput {
                json,
                input_tokens,
                output_tokens,
                model: answer
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or(&self.id.model)
                    .to_string(),
            })
        }
        .instrument(span)
        .await
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAiCompatible {
    fn id(&self) -> &ModelRef {
        &self.id
    }

    /// The configured dimension, or 0 when this provider was not configured
    /// as an embedder.
    fn dim(&self) -> usize {
        self.opts.embedding_dim.unwrap_or(0)
    }

    async fn embed(&self, texts: &[String], budget: &Budget) -> Result<Vec<Vec<f32>>, LlmError> {
        let dim = self.opts.embedding_dim.ok_or(LlmError::NotConfigured)?;
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let estimate = estimate_embed(texts);
        let reservation = budget.reserve(estimate)?;
        let span = tracing::debug_span!("llm.embed", provider = PROVIDER, model = %self.id.model, count = texts.len());
        async move {
            let url = join(&self.id.base_url, "embeddings")?;
            let body = json!({"model": self.id.model, "input": texts, "encoding_format": "float"});
            let answer = self
                .http
                .post_json(PROVIDER, &self.id.model, url, self.headers()?, &body)
                .await?;
            let used = u64_at(&answer, "/usage/prompt_tokens").unwrap_or(estimate);
            reservation.settle(used);
            let data = answer
                .get("data")
                .and_then(Value::as_array)
                .ok_or_else(|| LlmError::InvalidOutput("no data in the answer".to_string()))?;
            let mut out: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
            for (pos, item) in data.iter().enumerate() {
                let index = item
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                    .unwrap_or(pos);
                let vector = item
                    .get("embedding")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        LlmError::InvalidOutput("embedding is not an array".to_string())
                    })?
                    .iter()
                    .map(|x| x.as_f64().map(|f| f as f32))
                    .collect::<Option<Vec<f32>>>()
                    .ok_or_else(|| {
                        LlmError::InvalidOutput("embedding is not numeric".to_string())
                    })?;
                if vector.len() != dim {
                    return Err(LlmError::InvalidOutput(format!(
                        "embedding has dimension {}, expected {dim}",
                        vector.len()
                    )));
                }
                let slot = out.get_mut(index).ok_or_else(|| {
                    LlmError::InvalidOutput("embedding index out of range".to_string())
                })?;
                *slot = Some(vector);
            }
            out.into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| LlmError::InvalidOutput("an embedding is missing".to_string()))
        }
        .instrument(span)
        .await
    }
}

#[async_trait::async_trait]
impl Reranker for OpenAiCompatible {
    /// `POST <base>/rerank` with `{model, query, documents}`, answered with
    /// `results: [{index, relevance_score}]` (the shape vLLM, llama.cpp's
    /// server and Text Embeddings Inference's compatible route serve).
    async fn rerank(
        &self,
        query: &str,
        docs: &[String],
        budget: &Budget,
    ) -> Result<Vec<f32>, LlmError> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        let estimate = estimate_rerank(query, docs);
        let reservation = budget.reserve(estimate)?;
        let span = tracing::debug_span!("llm.rerank", provider = PROVIDER, model = %self.id.model, count = docs.len());
        async move {
            let url = join(&self.id.base_url, "rerank")?;
            let body = json!({"model": self.id.model, "query": query, "documents": docs});
            let answer = self
                .http
                .post_json(PROVIDER, &self.id.model, url, self.headers()?, &body)
                .await?;
            reservation.settle(u64_at(&answer, "/usage/total_tokens").unwrap_or(estimate));
            let results = answer
                .get("results")
                .and_then(Value::as_array)
                .ok_or_else(|| LlmError::InvalidOutput("no results in the answer".to_string()))?;
            let mut scores: Vec<Option<f32>> = vec![None; docs.len()];
            for r in results {
                let index = r
                    .get("index")
                    .and_then(Value::as_u64)
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| LlmError::InvalidOutput("result without index".to_string()))?;
                let score = r
                    .get("relevance_score")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| LlmError::InvalidOutput("result without score".to_string()))?;
                *scores.get_mut(index).ok_or_else(|| {
                    LlmError::InvalidOutput("result index out of range".to_string())
                })? = Some(score as f32);
            }
            scores
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| LlmError::InvalidOutput("a score is missing".to_string()))
        }
        .instrument(span)
        .await
    }
}
