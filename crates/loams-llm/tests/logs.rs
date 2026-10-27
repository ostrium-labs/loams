//! No content and no key in logs, spans or error messages: every path runs
//! at TRACE with span events, and the captured output must hold neither
//! canary.

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::{MockServer, Reply, fast_http, person_schema};
use loams_llm::{
    Anthropic, AnthropicOptions, Budget, ChatMessage, ChatModel, ChatRequest, Embedder, FakeChat,
    LocalPolicy, ModelRef, OpenAiCompatible, OpenAiOptions, ProviderKind, Reranker, Secret,
    SecretRef, StaticSecrets, StructuredMode,
};
use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::FmtSpan;

const KEY: &str = "sk-CANARY-KEY-5f1c9a";
const CONTENT: &str = "CANARY-CONTENT-8d2e7b";

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("lock")).into_owned()
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn request() -> ChatRequest {
    ChatRequest {
        system: format!("System prompt {CONTENT}"),
        messages: vec![ChatMessage::user(format!("Episode {CONTENT}"))],
        max_output_tokens: 128,
        temperature: 0.2,
    }
}

fn chat_answer(content: &str) -> String {
    json!({
        "id": "x", "object": "chat.completion", "model": "m",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5}
    })
    .to_string()
}

/// Runs every provider path once and answers what it printed by `Debug`
/// and `Display` (models, outputs, errors).
async fn exercise() -> Vec<String> {
    let mut printed = Vec::new();
    let key_ref = SecretRef::parse("env/CANARY").expect("ref");
    let secrets = StaticSecrets::default().with(key_ref.clone(), Secret::new(KEY.to_string()));
    let canary_error =
        json!({"error": {"message": format!("bad request {CONTENT} {KEY}")}}).to_string();
    let server = MockServer::start(vec![
        // A valid answer whose content carries the canary.
        Reply::ok(chat_answer(&json!({"name": CONTENT, "age": 3}).to_string())),
        // A schema violation whose value is the canary.
        Reply::ok(chat_answer(&json!({"name": "x", "age": CONTENT}).to_string())),
        // Prose with the canary.
        Reply::ok(chat_answer(&format!("I think {CONTENT}"))),
        // A retry, then an error body with both canaries.
        Reply::status(503, canary_error.clone()),
        Reply::status(400, canary_error.clone()),
        // Embeddings and rerank.
        Reply::ok(json!({"data": [{"index": 0, "embedding": [1.0, 0.0]}], "usage": {"prompt_tokens": 3}}).to_string()),
        Reply::ok(json!({"results": [{"index": 0, "relevance_score": 0.5}]}).to_string()),
        // Anthropic.
        Reply::ok(
            json!({
                "id": "m", "type": "message", "role": "assistant", "model": "c",
                "content": [{"type": "tool_use", "id": "t", "name": "person", "input": {"name": CONTENT, "age": 1}}],
                "stop_reason": "tool_use", "usage": {"input_tokens": 5, "output_tokens": 5}
            })
            .to_string(),
        ),
        Reply::status(401, canary_error),
    ])
    .await;

    let id = ModelRef {
        provider: ProviderKind::OpenAiCompatible,
        model: "gpt-fixture".to_string(),
        base_url: server.url("/v1"),
        key: Some(key_ref.clone()),
    };
    let openai = OpenAiCompatible::connect(
        id,
        &LocalPolicy::open(),
        &secrets,
        OpenAiOptions {
            structured: StructuredMode::JsonObject,
            embedding_dim: Some(2),
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await
    .expect("connect");
    printed.push(format!("{openai:?}"));
    for _ in 0..4 {
        let r = openai
            .complete_json(request(), &person_schema(), &Budget::unlimited())
            .await;
        printed.push(format!("{r:?}"));
        if let Err(e) = &r {
            printed.push(e.to_string());
        }
    }
    let e = openai
        .embed(&[format!("text {CONTENT}")], &Budget::unlimited())
        .await;
    printed.push(format!("{:?}", e.map(|v| v.len())));
    let r = openai
        .rerank(CONTENT, &[format!("doc {CONTENT}")], &Budget::unlimited())
        .await;
    printed.push(format!("{r:?}"));

    let anthropic = Anthropic::connect(
        ModelRef {
            provider: ProviderKind::Anthropic,
            model: "claude-fixture".to_string(),
            base_url: server.url("/"),
            key: Some(key_ref),
        },
        &LocalPolicy::open(),
        &secrets,
        AnthropicOptions {
            http: fast_http(),
            ..AnthropicOptions::default()
        },
    )
    .await
    .expect("connect");
    printed.push(format!("{anthropic:?}"));
    for _ in 0..2 {
        let r = anthropic
            .complete_json(request(), &person_schema(), &Budget::unlimited())
            .await;
        printed.push(format!("{r:?}"));
        if let Err(e) = &r {
            printed.push(e.to_string());
        }
    }

    let fake = FakeChat::from_json(
        &json!({"model": "f", "rules": [{"answer": {"name": CONTENT, "age": "x"}}]}).to_string(),
    )
    .expect("fake");
    let r = fake
        .complete_json(request(), &person_schema(), &Budget::unlimited())
        .await;
    printed.push(format!("{r:?}"));
    printed.push(format!("{:?}", request()));
    printed.push(format!("{secrets:?}"));
    printed
}

async fn run_captured() -> (String, Vec<String>) {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_span_events(FmtSpan::FULL)
        .with_ansi(false)
        .with_writer(capture.clone())
        .finish();
    let printed = {
        let _guard = tracing::subscriber::set_default(subscriber);
        exercise().await
    };
    (capture.text(), printed)
}

#[tokio::test]
async fn provider_key_never_logged() {
    let (logs, printed) = run_captured().await;
    assert!(
        logs.contains("llm.complete_json"),
        "spans were captured: {logs}"
    );
    assert!(logs.contains("llm call answered"), "events were captured");
    assert!(!logs.contains(KEY), "the key reached a log line or span");
    for p in &printed {
        assert!(!p.contains(KEY), "the key reached Debug/Display: {p}");
    }
}

#[tokio::test]
async fn no_content_in_logs() {
    let (logs, printed) = run_captured().await;
    assert!(logs.contains("status=503"), "the retry was logged");
    assert!(
        !logs.contains(CONTENT),
        "content reached a log line or span"
    );
    for p in &printed {
        assert!(!p.contains(CONTENT), "content reached Debug/Display: {p}");
    }
}
