//! The configuration surface: what TOML and the environment may set, and what
//! is refused.
use std::collections::HashMap;

use loams_web_bridge::{ConfigError, ProviderKind, WebBridgeConfig};

const SERVICE_TOML: &str = r#"
provider = "service"

allowed_domains = ["*.example.com"]
artifact_dir = "/var/lib/loams/browser"

[service]
url = "ws://browser:9222/devtools/browser"
token_secret_ref = "env:browser-token"

[budgets]
max_concurrent_sessions = 8
daily_budget_ms = 3600000

[recording]
enabled = true

[local]
default_profile = "acme-staging/zulip"
"#;

#[test]
fn a_toml_section_becomes_a_checked_configuration() {
    let config = WebBridgeConfig::load(SERVICE_TOML).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(config.provider, ProviderKind::Service);
    assert_eq!(
        config.service.token_secret_ref.as_deref(),
        Some("env:browser-token")
    );
    assert_eq!(config.budgets.max_concurrent_sessions, 8);
    assert_eq!(
        config.budgets.daily_budget(),
        Some(std::time::Duration::from_secs(3600))
    );
    assert_eq!(config.budgets.request_timeout_ms, 60_000, "a default");
    assert!(config.recording.enabled);
    assert_eq!(
        config.local.default_profile.as_deref(),
        Some("acme-staging/zulip")
    );
    let egress = config.egress().unwrap_or_else(|error| panic!("{error}"));
    assert!(egress.is_allow_listed());
}

#[test]
fn the_environment_names_the_service_and_a_token_reference_but_never_the_token() {
    let environment: HashMap<&str, &str> = HashMap::from([
        ("LOAMS_BROWSER_PROVIDER", "service"),
        (
            "LOAMS_BROWSER_SERVICE_URL",
            "ws://browser.loams.svc.cluster.local:9222/devtools/browser",
        ),
        ("LOAMS_BROWSER_SERVICE_TOKEN_REF", "env:browser-token"),
        ("LOAMS_BROWSER_MAX_SESSIONS", "2"),
    ]);
    let mut config = WebBridgeConfig::default();
    config
        .apply_env_from(&|name| environment.get(name).map(|value| (*value).to_string()))
        .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(config.provider, ProviderKind::Service);
    assert_eq!(config.budgets.max_concurrent_sessions, 2);
    assert_eq!(
        config.service.token_secret_ref.as_deref(),
        Some("env:browser-token"),
        "only a reference, never a token"
    );
    config.validate().unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn the_local_provider_finds_its_binary_in_the_environment() {
    let mut config = WebBridgeConfig::default();
    config
        .apply_env_from(&|name| {
            (name == "LOAMS_OBSCURA_BIN").then(|| "/opt/obscura/0.2.3/obscura".to_string())
        })
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(config.provider, ProviderKind::Local, "local is the default");
    assert_eq!(
        config.local.program(),
        std::path::PathBuf::from("/opt/obscura/0.2.3/obscura")
    );
}

#[test]
fn an_unknown_field_is_a_typo_not_a_silent_default() {
    let error = WebBridgeConfig::load("provider = \"local\"\nacount-id = \"x\"")
        .err()
        .unwrap_or_else(|| panic!("an unknown field is refused"));
    assert!(matches!(error, ConfigError::Toml(_)), "{error:?}");
}

#[test]
fn the_removed_cloudflare_sections_are_refused_not_ignored() {
    for raw in [
        "provider = \"browser-run\"",
        "[browser_run]\naccount_id = \"0123456789abcdef0123456789abcdef\"",
        "allowed_domain_sets = [\"common-cdns\"]",
    ] {
        assert!(
            WebBridgeConfig::load(raw).is_err(),
            "{raw} configures the removed provider"
        );
    }
}

#[test]
fn a_service_without_a_token_reference_is_refused_off_loopback() {
    let error = WebBridgeConfig::load("provider = \"service\"")
        .err()
        .unwrap_or_else(|| panic!("the default service url is not loopback"));
    assert!(
        matches!(
            error,
            ConfigError::Missing {
                field: "service.token_secret_ref"
            }
        ),
        "{error:?}"
    );
}

#[test]
fn a_secret_fill_needs_a_host_to_be_allowed_on() {
    let mut config = WebBridgeConfig::default();
    config.credentials.allow_secret_fills = true;
    assert!(
        config.validate().is_err(),
        "an empty host list allows nothing"
    );

    config.credentials.allowed_fill_hosts = vec!["app.example.com".to_string()];
    config.validate().unwrap_or_else(|error| panic!("{error}"));
    assert!(config.credentials.allows_fill_on("app.example.com"));
    assert!(!config.credentials.allows_fill_on("evil.example.com"));
}
