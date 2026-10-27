//! Anthropic Messages: structured output through one tool whose
//! `input_schema` is the schema. No embeddings and no reranking.
//!
//! The current model families refuse a forced `tool_choice` (`tool`, `any`)
//! and sampling parameters, so by default the call uses `tool_choice: auto`
//! with parallel tool use off, a fixed instruction to call the tool, and no
//! `temperature`; both are options for models that accept them (ruling
//! R2.7). The tool input is validated against the schema either way.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};
use tracing::Instrument;

use crate::budget::Budget;
use crate::chat::{ChatModel, ChatOutput, ChatRequest, Role};
use crate::config::{LocalPolicy, ModelRef, ProviderKind, Secret, SecretSource};
use crate::error::LlmError;
use crate::http::{Http, HttpOptions, join};
use crate::schema::JsonSchema;

const PROVIDER: &str = "anthropic";

/// The `anthropic-version` header the crate speaks.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// How the structured-output tool is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolChoice {
    /// `{type: auto, disable_parallel_tool_use: true}` plus an instruction.
    #[default]
    Auto,
    /// `{type: tool, name}`, for models that accept a forced tool.
    Forced,
}

/// Options of the Anthropic provider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnthropicOptions {
    /// How the structured-output tool is chosen.
    pub tool_choice: ToolChoice,
    /// Send `temperature` (refused by the current model families).
    pub send_temperature: bool,
    /// Transport options.
    pub http: HttpOptions,
}

impl Default for AnthropicOptions {
    fn default() -> Self {
        Self {
            tool_choice: ToolChoice::Auto,
            send_temperature: false,
            http: HttpOptions::default(),
        }
    }
}

/// Anthropic's Messages API.
pub struct Anthropic {
    id: ModelRef,
    key: Secret<String>,
    http: Http,
    opts: AnthropicOptions,
}

impl std::fmt::Debug for Anthropic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Anthropic")
            .field("id", &self.id)
            .field("key", &self.key)
            .field("opts", &self.opts)
            .finish_non_exhaustive()
    }
}

impl Anthropic {
    /// Checks the base URL and `policy`, resolves the key (required) and
    /// builds the client.
    pub async fn connect(
        id: ModelRef,
        policy: &LocalPolicy,
        secrets: &dyn SecretSource,
        opts: AnthropicOptions,
    ) -> Result<Self, LlmError> {
        if id.provider != ProviderKind::Anthropic {
            return Err(LlmError::InvalidConfig(
                "model is not Anthropic".to_string(),
            ));
        }
        id.check(policy)?;
        let key = secrets
            .resolve(id.key.as_ref().ok_or(LlmError::NotConfigured)?)
            .await?;
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
        let mut key = HeaderValue::from_str(self.key.expose())
            .map_err(|_| LlmError::InvalidConfig("key is not a header value".to_string()))?;
        key.set_sensitive(true);
        h.insert(HeaderName::from_static("x-api-key"), key);
        h.insert(
            HeaderName::from_static("anthropic-version"),
            HeaderValue::from_static(ANTHROPIC_VERSION),
        );
        Ok(h)
    }

    fn body(&self, req: &ChatRequest, schema: &JsonSchema) -> Result<Value, LlmError> {
        if schema.schema().get("type").and_then(Value::as_str) != Some("object") {
            return Err(LlmError::InvalidConfig(
                "a tool input schema must have type object".to_string(),
            ));
        }
        let mut tool = json!({
            "name": schema.name(),
            "description": "Record the answer as this tool's input.",
            "input_schema": schema.schema(),
        });
        if schema.strict() {
            tool["strict"] = json!(true);
        }
        let mut system = req.system.clone();
        let tool_choice = match self.opts.tool_choice {
            ToolChoice::Auto => {
                system.push_str(&format!(
                    "\n\nAnswer only by calling the `{}` tool, exactly once.",
                    schema.name()
                ));
                json!({"type": "auto", "disable_parallel_tool_use": true})
            }
            ToolChoice::Forced => json!({"type": "tool", "name": schema.name()}),
        };
        let messages: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role { Role::User => "user", Role::Assistant => "assistant" },
                    "content": m.content,
                })
            })
            .collect();
        let mut body = json!({
            "model": self.id.model,
            "max_tokens": req.max_output_tokens,
            "system": system,
            "messages": messages,
            "tools": [tool],
            "tool_choice": tool_choice,
        });
        if self.opts.send_temperature {
            body["temperature"] = json!(req.temperature);
        }
        Ok(body)
    }
}

fn u64_at(v: &Value, ptr: &str) -> u64 {
    v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0)
}

#[async_trait::async_trait]
impl ChatModel for Anthropic {
    fn id(&self) -> &ModelRef {
        &self.id
    }

    async fn complete_json(
        &self,
        req: ChatRequest,
        schema: &JsonSchema,
        budget: &Budget,
    ) -> Result<ChatOutput, LlmError> {
        let body = self.body(&req, schema)?;
        let reservation = budget.reserve(req.estimate(schema))?;
        let span =
            tracing::debug_span!("llm.complete_json", provider = PROVIDER, model = %self.id.model);
        async move {
            let url = join(&self.id.base_url, "v1/messages")?;
            let answer = self
                .http
                .post_json(PROVIDER, &self.id.model, url, self.headers()?, &body)
                .await?;
            // Cache writes and reads are input tokens too.
            let input_tokens = u64_at(&answer, "/usage/input_tokens")
                + u64_at(&answer, "/usage/cache_creation_input_tokens")
                + u64_at(&answer, "/usage/cache_read_input_tokens");
            let output_tokens = u64_at(&answer, "/usage/output_tokens");
            let reported = input_tokens + output_tokens;
            reservation.settle(if reported > 0 {
                reported
            } else {
                req.estimate(schema)
            });
            match answer.get("stop_reason").and_then(Value::as_str) {
                Some("refusal") => {
                    return Err(LlmError::InvalidOutput("model refused".to_string()));
                }
                Some("max_tokens") => {
                    return Err(LlmError::InvalidOutput(
                        "output cut off at max_output_tokens".to_string(),
                    ));
                }
                _ => {}
            }
            let input = answer
                .get("content")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks.iter().find(|b| {
                        b.get("type").and_then(Value::as_str) == Some("tool_use")
                            && b.get("name").and_then(Value::as_str) == Some(schema.name())
                    })
                })
                .and_then(|b| b.get("input"))
                .ok_or_else(|| LlmError::InvalidOutput("no tool call in the answer".to_string()))?;
            schema.validate(input)?;
            Ok(ChatOutput {
                json: input.clone(),
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
