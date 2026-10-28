//! The browser tools (D2115): the agents' headless browser, Obscura, through
//! `loams-web-bridge`.
//!
//! The tools are listed when `LOAMS_BROWSER_PROVIDER` is set (`local` or
//! `service`). They are the same tools with the same policy either way: the
//! provider choice only decides where the engine runs (D2111, D2112):
//!
//! * `local` starts `obscura serve` on loopback for this run, the first time a
//!   page is opened, and stops it when the run's MCP server exits;
//! * `service` drives the deployment's `browser` service on the private
//!   network.
//!
//! Pages belong to this MCP server, which is one per agent run, so one run
//! cannot drive another run's pages. The egress policy, the credential
//! boundary, the budgets and recording all live in `loams-web-bridge`.

use std::sync::Arc;

use loams_web_bridge::{
    BrowserProvider, EnvSecretResolver, FillValue, FindQuery, OpenRequest, PageRef, SecretRef,
    SnapshotRequest, Uid, WaitCondition, WaitRequest, WebBridgeConfig,
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::tools::{ToolDef, annotations_for};

/// The variable that turns the browser tools on and picks the provider.
pub const ENV_BROWSER_PROVIDER: &str = "LOAMS_BROWSER_PROVIDER";

const OPEN: &str = "browser_open";
const NAVIGATE: &str = "browser_navigate";
const SNAPSHOT: &str = "browser_snapshot";
const FIND: &str = "browser_find";
const CLICK: &str = "browser_click";
const FILL: &str = "browser_fill";
const WAIT_FOR: &str = "browser_wait_for";
const SCREENSHOT: &str = "browser_screenshot";
const EXTRACT: &str = "browser_extract";
const CLOSE: &str = "browser_close";

const NAMES: [&str; 10] = [
    OPEN, NAVIGATE, SNAPSHOT, FIND, CLICK, FILL, WAIT_FOR, SCREENSHOT, EXTRACT, CLOSE,
];

/// The longest wait a tool call may ask for.
const MAX_WAIT_MS: u64 = 60_000;

/// Whether `name` is one of the browser tools.
pub(crate) fn is_tool(name: &str) -> bool {
    NAMES.contains(&name)
}

/// The risk of a browser tool (design §72 §7.2, D1431): a call that makes the
/// browser fetch is `egress`; reading what is already loaded is `read`.
pub(crate) fn risk(name: &str) -> Option<&'static str> {
    match name {
        OPEN | NAVIGATE | CLICK | FILL => Some("egress"),
        SNAPSHOT | FIND | WAIT_FOR | SCREENSHOT | EXTRACT => Some("read"),
        CLOSE => Some("write"),
        _ => None,
    }
}

/// The browser, configured from the environment.
#[derive(Debug)]
pub struct Browser {
    provider: Result<Arc<dyn BrowserProvider>, String>,
}

impl Browser {
    /// Read `LOAMS_BROWSER_*` and build the provider, or `None` when the
    /// browser is not configured. A configuration that is set but wrong still
    /// lists the tools, and every call answers with what is wrong, so the
    /// agent (and whoever reads its transcript) learns why there is no
    /// browser instead of finding no tools at all.
    pub fn from_env() -> Option<Self> {
        let set = std::env::var(ENV_BROWSER_PROVIDER)
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        if !set {
            return None;
        }
        let provider = (|| {
            let mut config = WebBridgeConfig::default();
            config.apply_env().map_err(|error| error.to_string())?;
            loams_web_bridge::from_config(&config, Arc::new(EnvSecretResolver::new()))
                .map_err(|error| error.to_string())
        })();
        if let Err(error) = &provider {
            tracing::warn!(%error, "the browser tools are configured but unusable");
        }
        Some(Self { provider })
    }

    /// A browser over an existing provider (tests, and hosts that build their
    /// own).
    pub fn over(provider: Arc<dyn BrowserProvider>) -> Self {
        Self {
            provider: Ok(provider),
        }
    }

    fn provider(&self) -> Result<&Arc<dyn BrowserProvider>, String> {
        self.provider
            .as_ref()
            .map_err(|error| format!("the browser is not available: {error}"))
    }

    fn page(&self, id: &str) -> Result<PageRef, String> {
        let provider = self.provider()?;
        if id.trim().is_empty() {
            return Err("page is required; use the id browser_open returned".to_string());
        }
        Ok(PageRef::new(provider.kind(), id.trim()))
    }

    /// Run one browser tool.
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let provider = self.provider()?;
        match name {
            OPEN => {
                let args: OpenArgs = parse(args)?;
                let request = match args.url.as_deref() {
                    Some(raw) => OpenRequest::at(parse_url(raw)?),
                    None => OpenRequest::blank(),
                };
                let page = provider.open(request).await.map_err(|e| e.to_string())?;
                let snapshot = provider
                    .snapshot(&page, &SnapshotRequest::new())
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "page": page.id(),
                    "provider": provider.kind().as_str(),
                    "url": snapshot.url,
                    "title": snapshot.title,
                    "snapshot": snapshot.render(SnapshotRequest::new().budget()),
                }))
            }
            NAVIGATE => {
                let args: NavigateArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let navigation = provider
                    .navigate(&page, &parse_url(&args.url)?)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "url": navigation.url,
                    "title": navigation.title,
                    "change": navigation.change.render(),
                }))
            }
            SNAPSHOT => {
                let args: SnapshotArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let mut request = SnapshotRequest::new();
                if args.verbose {
                    request = request.verbose();
                }
                if let Some(budget) = args.budget_chars {
                    request = request.with_budget_chars(budget.clamp(500, 200_000));
                }
                let snapshot = provider
                    .snapshot(&page, &request)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "url": snapshot.url,
                    "title": snapshot.title,
                    "snapshot": snapshot.render(request.budget()),
                }))
            }
            FIND => {
                let args: FindArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let found = provider
                    .find(&page, &FindQuery::text(args.text))
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "matches": found.iter().map(|node| node.render()).collect::<Vec<_>>(),
                }))
            }
            CLICK => {
                let args: UidArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let uid = Uid::parse(&args.uid).map_err(|e| e.to_string())?;
                let outcome = provider
                    .click(&page, &uid)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "result": outcome.render() }))
            }
            FILL => {
                let args: FillArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let uid = Uid::parse(&args.uid).map_err(|e| e.to_string())?;
                let value = match (args.value, args.secret_ref) {
                    (Some(value), None) => FillValue::Literal(value),
                    (None, Some(reference)) => {
                        FillValue::Secret(SecretRef::parse(&reference).map_err(|e| e.to_string())?)
                    }
                    _ => return Err("give exactly one of value or secret_ref".to_string()),
                };
                let outcome = provider
                    .fill(&page, &uid, &value)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({ "result": outcome.render() }))
            }
            WAIT_FOR => {
                let args: WaitArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let condition = match (args.text, args.url, args.selector) {
                    (Some(text), None, None) => WaitCondition::Text(text),
                    (None, Some(url), None) => WaitCondition::Url(url),
                    (None, None, Some(selector)) => WaitCondition::Selector(selector),
                    _ => return Err("give exactly one of text, url or selector".to_string()),
                };
                let request = WaitRequest::new(condition)
                    .with_timeout_ms(args.timeout_ms.unwrap_or(10_000).min(MAX_WAIT_MS));
                let outcome = provider
                    .wait_for(&page, &request)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "matched": outcome.matched,
                    "waited_ms": outcome.waited_ms,
                    "note": outcome.note,
                }))
            }
            SCREENSHOT => {
                let args: PageArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let shot = provider
                    .screenshot(&page)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(json!({
                    "path": shot.path.display().to_string(),
                    "bytes": shot.bytes,
                    "sha256": shot.sha256,
                }))
            }
            EXTRACT => {
                let args: PageArgs = parse(args)?;
                let page = self.page(&args.page)?;
                let extracted = provider.extract(&page).await.map_err(|e| e.to_string())?;
                Ok(json!({
                    "url": extracted.url,
                    "title": extracted.title,
                    "text": extracted.text,
                    "truncated": extracted.truncated,
                }))
            }
            CLOSE => {
                let args: PageArgs = parse(args)?;
                let page = self.page(&args.page)?;
                provider.close(&page).await.map_err(|e| e.to_string())?;
                Ok(json!({ "closed": page.id() }))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn parse_url(raw: &str) -> Result<url::Url, String> {
    url::Url::parse(raw.trim()).map_err(|e| format!("{raw:?} is not a URL: {e}"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PageArgs {
    page: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NavigateArgs {
    page: String,
    url: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotArgs {
    page: String,
    #[serde(default)]
    verbose: bool,
    #[serde(default)]
    budget_chars: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FindArgs {
    page: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UidArgs {
    page: String,
    uid: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillArgs {
    page: String,
    uid: String,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    secret_ref: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    page: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// The browser tools' definitions.
pub(crate) fn defs() -> Vec<ToolDef> {
    let page = json!({ "type": "string", "description": "The page id browser_open returned." });
    let uid = json!({ "type": "string", "description": "An element uid from the latest browser_snapshot, e.g. s3_12." });
    let object = |properties: Value, required: &[&str]| json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false });
    let mut defs = vec![
        ToolDef {
            name: OPEN,
            description: "Open a page in the headless browser (Obscura) and return its id and a text snapshot with element uids. Public http(s) URLs only: private, loopback and cloud-metadata addresses are refused, here and for every request the page makes. Close pages you are done with; a run has a few at most.",
            input_schema: object(
                json!({ "url": { "type": "string", "description": "Where to go first; omit for a blank page." } }),
                &[],
            ),
            annotations: Value::Null,
        },
        ToolDef {
            name: NAVIGATE,
            description: "Navigate an open page to a URL, under the same network policy as browser_open. A redirect to a refused host is refused and the page is left blank.",
            input_schema: object(
                json!({ "page": page, "url": { "type": "string" } }),
                &["page", "url"],
            ),
            annotations: Value::Null,
        },
        ToolDef {
            name: SNAPSHOT,
            description: "Read the page as text with element uids (the accessibility tree). Uids name their snapshot, so act on uids from the latest snapshot only. Credential fields never show their value.",
            input_schema: object(
                json!({
                    "page": page,
                    "verbose": { "type": "boolean", "default": false, "description": "Keep nodes the default pruning drops." },
                    "budget_chars": { "type": "integer", "minimum": 500, "maximum": 200000 }
                }),
                &["page"],
            ),
            annotations: Value::Null,
        },
        ToolDef {
            name: FIND,
            description: "Search the latest snapshot for elements whose text contains a string, instead of reading the whole page.",
            input_schema: object(
                json!({ "page": page, "text": { "type": "string" } }),
                &["page", "text"],
            ),
            annotations: Value::Null,
        },
        ToolDef {
            name: CLICK,
            description: "Click an element by uid. Answers in one line with what changed.",
            input_schema: object(json!({ "page": page, "uid": uid }), &["page", "uid"]),
            annotations: Value::Null,
        },
        ToolDef {
            name: FILL,
            description: "Type into a field by uid: a literal value, or a secret_ref the browser resolves itself so the secret never passes through the conversation. Secret fills are off unless the operator allowed them for that host.",
            input_schema: object(
                json!({
                    "page": page,
                    "uid": uid,
                    "value": { "type": "string" },
                    "secret_ref": { "type": "string", "description": "A stored secret's reference, e.g. loams:acme/app#password." }
                }),
                &["page", "uid"],
            ),
            annotations: Value::Null,
        },
        ToolDef {
            name: WAIT_FOR,
            description: "Wait until text appears, the URL contains a string, or a CSS selector matches (exactly one). Answers whether it happened; it never claims a match that did not.",
            input_schema: object(
                json!({
                    "page": page,
                    "text": { "type": "string" },
                    "url": { "type": "string" },
                    "selector": { "type": "string" },
                    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": MAX_WAIT_MS, "default": 10000 }
                }),
                &["page"],
            ),
            annotations: Value::Null,
        },
        ToolDef {
            name: SCREENSHOT,
            description: "Save a PNG screenshot of the page and return its path, size and SHA-256 (not the image).",
            input_schema: object(json!({ "page": page }), &["page"]),
            annotations: Value::Null,
        },
        ToolDef {
            name: EXTRACT,
            description: "Return the page's visible text, title and URL, cut to a budget.",
            input_schema: object(json!({ "page": page }), &["page"]),
            annotations: Value::Null,
        },
        ToolDef {
            name: CLOSE,
            description: "Close a page and discard its cookies and storage.",
            input_schema: object(json!({ "page": page }), &["page"]),
            annotations: Value::Null,
        },
    ];
    for def in &mut defs {
        def.annotations = annotations_for(def.name);
    }
    defs
}

#[cfg(test)]
mod tests {
    use super::*;
    use loams_web_bridge::testing::{FakeCdp, obscura_answers, obscura_over};
    use loams_web_bridge::{MapSecretResolver, ProviderKind};

    fn browser(kind: ProviderKind) -> Browser {
        let config = WebBridgeConfig {
            provider: kind,
            ..WebBridgeConfig::default()
        };
        Browser::over(Arc::new(obscura_over(
            kind,
            &config,
            Arc::new(FakeCdp::new(obscura_answers("https://example.com/login"))),
            Arc::new(MapSecretResolver::new()),
        )))
    }

    #[test]
    fn every_tool_has_a_definition_and_a_risk() {
        let defs = defs();
        assert_eq!(defs.len(), NAMES.len());
        for def in &defs {
            assert!(is_tool(def.name));
            assert!(risk(def.name).is_some(), "{}", def.name);
            assert_eq!(def.annotations["risk"], risk(def.name).unwrap_or_default());
        }
        assert_eq!(risk(OPEN), Some("egress"));
        assert_eq!(risk(SNAPSHOT), Some("read"));
    }

    #[tokio::test]
    async fn the_same_calls_work_on_both_providers() {
        for kind in [ProviderKind::Local, ProviderKind::Service] {
            let browser = browser(kind);
            let opened = browser
                .call(OPEN, json!({ "url": "https://example.com/login" }))
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(opened["provider"], kind.as_str());
            let page = opened["page"].as_str().unwrap_or_default().to_string();
            assert!(
                opened["snapshot"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("uid=")
            );

            let refused = browser
                .call(
                    NAVIGATE,
                    json!({ "page": page, "url": "http://169.254.169.254/latest/meta-data/" }),
                )
                .await
                .err()
                .unwrap_or_else(|| panic!("the metadata endpoint is refused"));
            assert!(refused.contains("egress"), "{refused}");

            let extracted = browser
                .call(EXTRACT, json!({ "page": page }))
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(extracted["title"], "Sign in");

            browser
                .call(CLOSE, json!({ "page": page }))
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            assert!(
                browser
                    .call(EXTRACT, json!({ "page": page }))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn arguments_are_checked_before_the_browser_is_touched() {
        let browser = browser(ProviderKind::Service);
        assert!(
            browser
                .call(OPEN, json!({ "url": "not a url" }))
                .await
                .is_err()
        );
        assert!(
            browser
                .call(FILL, json!({ "page": "p", "uid": "s1_1" }))
                .await
                .is_err_and(|error| error.contains("exactly one"))
        );
        assert!(
            browser
                .call(WAIT_FOR, json!({ "page": "p", "text": "a", "url": "b" }))
                .await
                .is_err_and(|error| error.contains("exactly one"))
        );
        assert!(
            browser
                .call(OPEN, json!({ "url": "https://example.com", "extra": 1 }))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_broken_configuration_answers_every_call_with_the_reason() {
        let browser = Browser {
            provider: Err("service.token_secret_ref is required".to_string()),
        };
        let error = browser
            .call(OPEN, json!({}))
            .await
            .err()
            .unwrap_or_else(|| panic!("no provider"));
        assert!(error.contains("token_secret_ref"), "{error}");
    }
}
