//! `local_only` and the base URL rules.

use loams_llm::{
    Anthropic, AnthropicOptions, Cidr, LlmError, LocalPolicy, ModelRef, OpenAiCompatible,
    OpenAiOptions, ProviderKind, Secret, SecretRef, StaticSecrets, connect_chat,
};

fn model(url: &str) -> ModelRef {
    ModelRef {
        provider: ProviderKind::OpenAiCompatible,
        model: "m".to_string(),
        base_url: url::Url::parse(url).expect("url"),
        key: None,
    }
}

#[tokio::test]
async fn local_only_refuses_public_host() {
    let policy = LocalPolicy::local_only(vec![Cidr::parse("192.168.10.0/24").expect("cidr")]);
    for url in [
        "https://api.openai.com/v1",
        "https://203.0.113.7/v1",
        "https://[2001:db8::1]/v1",
        "https://192.168.11.5/v1",
        // A name is refused even if it would resolve to a private address.
        "https://ollama.internal/v1",
        "https://localhost.example.com/v1",
    ] {
        assert_eq!(model(url).check(&policy), Err(LlmError::NotLocal), "{url}");
    }
    let err = connect_chat(
        &model("https://api.openai.com/v1"),
        &policy,
        &StaticSecrets::default(),
    )
    .await
    .err()
    .expect("refused");
    assert_eq!(err, LlmError::NotLocal);
    let key = SecretRef::parse("env/K").expect("ref");
    let err = Anthropic::connect(
        ModelRef::anthropic("claude-fixture", key.clone()),
        &policy,
        &StaticSecrets::default().with(key, Secret::new("k".to_string())),
        AnthropicOptions::default(),
    )
    .await
    .expect_err("refused");
    assert_eq!(err, LlmError::NotLocal);
    // Without local_only the same hosts pass.
    model("https://api.openai.com/v1")
        .check(&LocalPolicy::open())
        .expect("open policy");
}

#[tokio::test]
async fn local_only_allows_loopback_and_allowlist() {
    let policy = LocalPolicy::local_only(vec![
        Cidr::parse("192.168.10.0/24").expect("cidr"),
        Cidr::parse("fd00::/8").expect("cidr"),
    ]);
    for url in [
        "http://127.0.0.1:11434/v1",
        "http://127.8.9.10:8000/v1",
        "http://localhost:1234/v1",
        "http://[::1]:8000/v1",
        "https://192.168.10.20/v1",
        "https://[fd12::5]:8443/v1",
    ] {
        model(url)
            .check(&policy)
            .unwrap_or_else(|e| panic!("{url}: {e}"));
    }
    OpenAiCompatible::connect(
        model("http://localhost:11434/v1"),
        &policy,
        &StaticSecrets::default(),
        OpenAiOptions::default(),
    )
    .await
    .expect("loopback connects");
    // A public block cannot be put on the allow-list.
    assert!(matches!(
        Cidr::parse("8.8.8.0/24"),
        Err(LlmError::InvalidConfig(_))
    ));
    assert!(matches!(
        Cidr::parse("0.0.0.0/0"),
        Err(LlmError::InvalidConfig(_))
    ));
}

#[test]
fn base_url_with_userinfo_refused() {
    for url in [
        "https://user:pass@api.openai.com/v1",
        "https://user@api.openai.com/v1",
        "http://user:pass@127.0.0.1:11434/v1",
    ] {
        let err = model(url).check(&LocalPolicy::open()).expect_err(url);
        assert!(
            matches!(&err, LlmError::InvalidConfig(m) if m.contains("userinfo")),
            "{url}"
        );
        assert!(!err.to_string().contains("pass"), "the URL is not echoed");
    }
}

#[test]
fn plain_http_off_loopback_refused() {
    for url in [
        "http://api.openai.com/v1",
        "http://192.168.10.20/v1",
        "http://10.0.0.5:11434/v1",
    ] {
        let err = model(url).check(&LocalPolicy::open()).expect_err(url);
        assert!(
            matches!(&err, LlmError::InvalidConfig(m) if m.contains("loopback")),
            "{url}"
        );
    }
    for url in ["ftp://127.0.0.1/v1", "file:///tmp/x"] {
        assert!(
            matches!(
                model(url).check(&LocalPolicy::open()),
                Err(LlmError::InvalidConfig(_))
            ),
            "{url}"
        );
    }
    model("http://127.0.0.1:11434/v1")
        .check(&LocalPolicy::open())
        .expect("loopback http");
    model("https://api.openai.com/v1")
        .check(&LocalPolicy::open())
        .expect("https");
}
