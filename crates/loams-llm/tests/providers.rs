//! Providers against recorded fixtures on a loopback server, and the fakes.

mod common;

use common::{MockServer, Reply, fast_http, fixture, fixture_path, person_request, person_schema};
use loams_llm::{
    Anthropic, AnthropicOptions, Budget, ChatModel, Embedder, FakeChat, FakeEmbedder, FakeReranker,
    LlmError, LocalPolicy, ModelRef, OpenAiCompatible, OpenAiOptions, ProviderKind, Reranker,
    Secret, SecretRef, StaticSecrets, StructuredMode, ToolChoice,
};
use serde_json::json;

fn key_ref() -> SecretRef {
    SecretRef::parse("env/TEST").expect("ref")
}

fn secrets(value: &str) -> StaticSecrets {
    StaticSecrets::default().with(key_ref(), Secret::new(value.to_string()))
}

async fn openai(server: &MockServer, opts: OpenAiOptions) -> OpenAiCompatible {
    let id = ModelRef {
        provider: ProviderKind::OpenAiCompatible,
        model: "gpt-fixture".to_string(),
        base_url: server.url("/v1"),
        key: Some(key_ref()),
    };
    OpenAiCompatible::connect(id, &LocalPolicy::open(), &secrets("sk-test"), opts)
        .await
        .expect("connect")
}

#[tokio::test]
async fn openai_structured_output_round_trip() {
    let server = MockServer::start(vec![Reply::ok(fixture(
        "json/openai-chat-json-schema.json",
    ))])
    .await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let out = model
        .complete_json(
            person_request("Ada Lovelace, 36, mathematician"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("answer");
    assert_eq!(
        out.json,
        json!({"name": "Ada Lovelace", "age": 36, "tags": ["mathematician"]})
    );
    assert_eq!((out.input_tokens, out.output_tokens), (61, 19));
    assert_eq!(out.model, "gpt-fixture-2026-01-01");

    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(seen[0].headers["authorization"], "Bearer sk-test");
    let body = &seen[0].body;
    assert_eq!(body["model"], "gpt-fixture");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["response_format"]["json_schema"]["name"], "person");
    assert_eq!(
        body["response_format"]["json_schema"]["schema"],
        *person_schema().schema()
    );
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "Extract the person."})
    );
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["max_tokens"], 256);
}

#[tokio::test]
async fn openai_falls_back_to_validation_without_json_schema() {
    let server = MockServer::start(vec![Reply::ok(fixture("json/openai-chat-fenced.json"))]).await;
    let model = openai(
        &server,
        OpenAiOptions {
            structured: StructuredMode::Instruction,
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let out = model
        .complete_json(
            person_request("Ada Lovelace, 36"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("answer");
    assert_eq!(out.json, json!({"name": "Ada Lovelace", "age": 36}));
    let body = &server.seen()[0].body;
    assert!(
        body.get("response_format").is_none(),
        "no response_format in instruction mode"
    );
    let system = body["messages"][0]["content"].as_str().expect("system");
    assert!(system.starts_with("Extract the person."));
    assert!(
        system.contains("\"required\":[\"name\",\"age\"]"),
        "the schema is in the system prompt"
    );

    // The fallback still validates: prose is invalid output.
    let server =
        MockServer::start(vec![Reply::ok(fixture("json/openai-chat-not-json.json"))]).await;
    let model = openai(
        &server,
        OpenAiOptions {
            structured: StructuredMode::Instruction,
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let err = model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("prose");
    assert_eq!(err, LlmError::InvalidOutput("not JSON".to_string()));
}

#[tokio::test]
async fn openai_retries_429_then_succeeds() {
    let server = MockServer::start(vec![
        Reply::status(429, "{}").header("retry-after", "0"),
        Reply::status(503, "{}"),
        Reply::ok(fixture("json/openai-chat-json-schema.json")),
    ])
    .await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("third attempt answers");
    assert_eq!(server.seen().len(), 3);
}

#[tokio::test]
async fn openai_gives_up_after_three_retries_and_on_long_retry_after() {
    let server = MockServer::start(vec![Reply::status(500, "{}"); 5]).await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let err = model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("500s");
    assert_eq!(
        err,
        LlmError::Provider {
            status: 500,
            retryable: true
        }
    );
    assert_eq!(server.seen().len(), 4, "one attempt and three retries");

    let server =
        MockServer::start(vec![Reply::status(429, "{}").header("retry-after", "120")]).await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let err = model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("429");
    assert_eq!(
        err,
        LlmError::Provider {
            status: 429,
            retryable: true
        }
    );
    assert_eq!(
        server.seen().len(),
        1,
        "a Retry-After over 30 s is not waited for"
    );

    let server = MockServer::start(vec![Reply::status(
        400,
        "{\"error\":{\"message\":\"secret prompt\"}}",
    )])
    .await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let err = model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("400");
    assert_eq!(
        err,
        LlmError::Provider {
            status: 400,
            retryable: false
        }
    );
    assert_eq!(server.seen().len(), 1);
}

#[tokio::test]
async fn openai_embeddings_in_input_order() {
    let server = MockServer::start(vec![Reply::ok(fixture("json/openai-embeddings.json"))]).await;
    let model = openai(
        &server,
        OpenAiOptions {
            embedding_dim: Some(4),
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let texts = vec!["first".to_string(), "second".to_string()];
    let v = model
        .embed(&texts, &Budget::unlimited())
        .await
        .expect("vectors");
    assert_eq!(v, vec![vec![1.0, 0.0, 0.0, 0.0], vec![0.0, 1.0, 0.0, 0.0]]);
    assert_eq!(server.seen()[0].path, "/v1/embeddings");
    assert_eq!(server.seen()[0].body["input"], json!(["first", "second"]));

    let server = MockServer::start(vec![Reply::ok(fixture("json/openai-embeddings.json"))]).await;
    let model = openai(
        &server,
        OpenAiOptions {
            embedding_dim: Some(8),
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let err = model
        .embed(&texts, &Budget::unlimited())
        .await
        .expect_err("dimension");
    assert!(matches!(err, LlmError::InvalidOutput(m) if m.contains("dimension 4")));
}

#[tokio::test]
async fn openai_rerank_scores_by_index() {
    let server = MockServer::start(vec![Reply::ok(
        json!({"results": [{"index": 1, "relevance_score": 0.9}, {"index": 0, "relevance_score": 0.1}]}).to_string(),
    )])
    .await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let scores = model
        .rerank(
            "q",
            &["a".to_string(), "b".to_string()],
            &Budget::unlimited(),
        )
        .await
        .expect("scores");
    assert_eq!(scores, vec![0.1, 0.9]);
    assert_eq!(server.seen()[0].path, "/v1/rerank");
}

#[tokio::test]
async fn anthropic_tool_schema_structured_output() {
    let server = MockServer::start(vec![Reply::ok(fixture("json/anthropic-tool-use.json"))]).await;
    let id = ModelRef {
        provider: ProviderKind::Anthropic,
        model: "claude-fixture".to_string(),
        base_url: server.url("/"),
        key: Some(key_ref()),
    };
    let model = Anthropic::connect(
        id,
        &LocalPolicy::open(),
        &secrets("sk-ant-test"),
        AnthropicOptions {
            http: fast_http(),
            ..AnthropicOptions::default()
        },
    )
    .await
    .expect("connect");
    let out = model
        .complete_json(
            person_request("Ada Lovelace, 36"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("answer");
    assert_eq!(
        out.json,
        json!({"name": "Ada Lovelace", "age": 36, "tags": ["mathematician"]})
    );
    assert_eq!((out.input_tokens, out.output_tokens), (410, 57));

    let seen = &server.seen()[0];
    assert_eq!(seen.path, "/v1/messages");
    assert_eq!(seen.headers["x-api-key"], "sk-ant-test");
    assert_eq!(seen.headers["anthropic-version"], "2023-06-01");
    assert!(seen.headers.get("authorization").is_none());
    let body = &seen.body;
    assert_eq!(body["tools"][0]["name"], "person");
    assert_eq!(body["tools"][0]["input_schema"], *person_schema().schema());
    assert_eq!(
        body["tool_choice"],
        json!({"type": "auto", "disable_parallel_tool_use": true})
    );
    assert!(
        body.get("temperature").is_none(),
        "no sampling parameters by default"
    );
    assert!(
        body["system"]
            .as_str()
            .expect("system")
            .contains("`person` tool")
    );
    assert_eq!(body["messages"][0]["role"], "user");
    assert_eq!(body["max_tokens"], 256);
}

#[tokio::test]
async fn anthropic_forced_tool_and_missing_tool_call() {
    let no_tool = json!({
        "id": "msg_fixture_2", "type": "message", "role": "assistant", "model": "claude-fixture",
        "content": [{"type": "text", "text": "Ada Lovelace is 36."}],
        "stop_reason": "end_turn", "usage": {"input_tokens": 10, "output_tokens": 5}
    });
    let server = MockServer::start(vec![Reply::ok(no_tool.to_string())]).await;
    let id = ModelRef {
        provider: ProviderKind::Anthropic,
        model: "claude-fixture".to_string(),
        base_url: server.url("/"),
        key: Some(key_ref()),
    };
    let model = Anthropic::connect(
        id,
        &LocalPolicy::open(),
        &secrets("sk-ant-test"),
        AnthropicOptions {
            tool_choice: ToolChoice::Forced,
            http: fast_http(),
            ..AnthropicOptions::default()
        },
    )
    .await
    .expect("connect");
    let err = model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("no tool call");
    assert_eq!(
        err,
        LlmError::InvalidOutput("no tool call in the answer".to_string())
    );
    assert_eq!(
        server.seen()[0].body["tool_choice"],
        json!({"type": "tool", "name": "person"})
    );
}

#[tokio::test]
async fn anthropic_requires_a_key() {
    let id = ModelRef {
        key: None,
        ..ModelRef::anthropic("claude-fixture", key_ref())
    };
    let err = Anthropic::connect(
        id,
        &LocalPolicy::open(),
        &StaticSecrets::default(),
        AnthropicOptions::default(),
    )
    .await
    .expect_err("no key");
    assert_eq!(err, LlmError::NotConfigured);
    let id = ModelRef::anthropic("claude-fixture", key_ref());
    let err = Anthropic::connect(
        id,
        &LocalPolicy::open(),
        &StaticSecrets::default(),
        AnthropicOptions::default(),
    )
    .await
    .expect_err("unresolved key");
    assert_eq!(err, LlmError::NotConfigured);
}

#[tokio::test]
async fn ollama_preset_is_openai_compatible_on_loopback() {
    let preset = ModelRef::ollama("llama-fixture");
    assert_eq!(preset.provider, ProviderKind::OpenAiCompatible);
    assert_eq!(preset.base_url.as_str(), "http://127.0.0.1:11434/v1");
    assert!(preset.key.is_none());
    preset
        .check(&LocalPolicy::local_only(Vec::new()))
        .expect("loopback passes local_only");

    // The preset's shape against a loopback server: no key, no auth header.
    let server = MockServer::start(vec![Reply::ok(fixture(
        "json/openai-chat-json-schema.json",
    ))])
    .await;
    let mut id = ModelRef::ollama("llama-fixture");
    id.base_url = server.url("/v1");
    let model = OpenAiCompatible::connect(
        id,
        &LocalPolicy::local_only(Vec::new()),
        &StaticSecrets::default(),
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await
    .expect("connect");
    model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("answer");
    let seen = &server.seen()[0];
    assert_eq!(seen.path, "/v1/chat/completions");
    assert!(seen.headers.get("authorization").is_none());
}

#[tokio::test]
async fn invalid_json_is_invalid_output_not_panic() {
    let bad = json!({
        "id": "x", "object": "chat.completion", "model": "m",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "{\"name\": \"Ada\", "}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    });
    let server = MockServer::start(vec![
        Reply::ok(bad.to_string()),
        Reply::ok("this is not json at all"),
        Reply::ok(json!({"choices": []}).to_string()),
    ])
    .await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    for expected in [
        "not JSON",
        "provider answer is not JSON",
        "no choice in the answer",
    ] {
        let err = model
            .complete_json(
                person_request("Ada"),
                &person_schema(),
                &Budget::unlimited(),
            )
            .await
            .expect_err("invalid");
        assert_eq!(err, LlmError::InvalidOutput(expected.to_string()));
    }

    let fake = FakeChat::from_fixture_file(fixture_path("json/fake-chat.json")).expect("fixture");
    let err = fake
        .complete_json(
            person_request("broken"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("invalid");
    assert_eq!(err, LlmError::InvalidOutput("not JSON".to_string()));
}

#[tokio::test]
async fn schema_violation_reported_with_path() {
    let server = MockServer::start(vec![Reply::ok(fixture(
        "json/openai-chat-schema-violation.json",
    ))])
    .await;
    let model = openai(
        &server,
        OpenAiOptions {
            http: fast_http(),
            ..OpenAiOptions::default()
        },
    )
    .await;
    let err = model
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("violation");
    assert_eq!(err, LlmError::InvalidOutput("/tags/1: type".to_string()));

    let fake = FakeChat::from_fixture_file(fixture_path("json/fake-chat.json")).expect("fixture");
    let err = fake
        .complete_json(
            person_request("wrong"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("violation");
    assert_eq!(err, LlmError::InvalidOutput("/age: minimum".to_string()));
    // The value is never in the message.
    assert!(!err.to_string().contains("-1"));
}

#[tokio::test]
async fn fake_chat_follows_its_script() {
    let fake = FakeChat::from_fixture_file(fixture_path("json/fake-chat.json")).expect("fixture");
    assert_eq!(fake.id().provider, ProviderKind::Fake);
    let out = fake
        .complete_json(
            person_request("Ada"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("answer");
    assert_eq!(out.json, json!({"name": "Ada Lovelace", "age": 36}));
    assert_eq!((out.input_tokens, out.output_tokens), (100, 10));
    // `times: 1`: the first Grace call fails, the next one answers.
    let err = fake
        .complete_json(
            person_request("Grace"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect_err("scripted 503");
    assert_eq!(
        err,
        LlmError::Provider {
            status: 503,
            retryable: true
        }
    );
    let out = fake
        .complete_json(
            person_request("Grace"),
            &person_schema(),
            &Budget::unlimited(),
        )
        .await
        .expect("answer");
    assert_eq!(out.json["name"], "Grace Hopper");
    assert_eq!(fake.calls(), 3);
}

#[tokio::test]
async fn fake_embedder_is_deterministic() {
    let a = FakeEmbedder::new("fake-embed", 64, 7).expect("embedder");
    let b = FakeEmbedder::new("fake-embed", 64, 7).expect("embedder");
    let texts = vec![
        "Ada lives in London".to_string(),
        "ada LIVES in london!".to_string(),
        "Grace wrote compilers".to_string(),
    ];
    let va = a
        .embed(&texts, &Budget::unlimited())
        .await
        .expect("vectors");
    let vb = b
        .embed(&texts, &Budget::unlimited())
        .await
        .expect("vectors");
    assert_eq!(va, vb, "same seed, same vectors");
    assert_eq!(va[0], va[1], "case and punctuation do not matter");
    assert_eq!(va[0].len(), 64);
    let norm: f32 = va[0].iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-5);
    let cos = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(a, b)| a * b).sum::<f32>();
    let shared = FakeEmbedder::new("fake-embed", 64, 7)
        .expect("embedder")
        .vector("Ada lives in Paris");
    assert!(
        cos(&va[0], &shared) > cos(&va[0], &va[2]),
        "shared words are closer"
    );

    let other = FakeEmbedder::new("fake-embed", 64, 8).expect("embedder");
    assert_ne!(
        other.vector("Ada lives in London"),
        va[0],
        "the seed matters"
    );

    let fixture =
        FakeEmbedder::from_fixture_file(fixture_path("json/fake-embed.json")).expect("fixture");
    assert_eq!(fixture.dim(), 4);
    assert_eq!(fixture.vector("pinned text"), vec![0.5, 0.5, 0.5, 0.5]);
}

#[tokio::test]
async fn fake_reranker_scores() {
    let r =
        FakeReranker::from_fixture_file(fixture_path("json/fake-rerank.json")).expect("fixture");
    let docs = vec![
        "Ada lives in London".to_string(),
        "Grace wrote compilers".to_string(),
    ];
    let s = r
        .rerank("where does ada live", &docs, &Budget::unlimited())
        .await
        .expect("scores");
    assert_eq!(s, vec![0.97, 0.0]);
    let s = FakeReranker::new("x")
        .rerank("ada london", &docs, &Budget::unlimited())
        .await
        .expect("scores");
    assert_eq!(s, vec![1.0, 0.0]);
}
