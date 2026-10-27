//! MK1 Task 5: install plans create nothing and show every declared
//! permission; the agent environment is exactly the contract; the token file
//! rotates.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use loams_agentd_marketplace::contract::{
    self, PROXY_DUMMY_KEY, Provisioned, ProxiedMcp, TOKEN_LIFETIME, TOKEN_ROTATION, TokenFile,
    TokenIssuer,
};
use loams_agentd_marketplace::plan::{self, InstalledVersion, Risk};
use loams_agentd_marketplace::sources::{OFFICIAL_IDENTITY, OFFICIAL_NAME};
use loams_agentd_marketplace::verify::{GITHUB_ACTIONS_ISSUER, Identity, SignatureError};
use loams_agentd_marketplace::{Fetcher, MarketError, Marketplace, SignatureVerifier};
use sha2::{Digest, Sha256};

const BASE: &str = "https://ostrium-labs.github.io/loams-marketplace";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/index")
            .join(name),
    )
    .unwrap()
}

fn sign(bytes: &[u8]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "sha256": hex::encode(Sha256::digest(bytes)),
        "identity": OFFICIAL_IDENTITY,
        "issuer": GITHUB_ACTIONS_ISSUER,
    }))
    .unwrap()
}

struct FakeVerifier;

impl SignatureVerifier for FakeVerifier {
    fn verify_blob(
        &self,
        artifact: &[u8],
        bundle: &[u8],
        signer: &Identity,
    ) -> Result<(), SignatureError> {
        let bundle: serde_json::Value =
            serde_json::from_slice(bundle).map_err(|e| SignatureError::Malformed(e.to_string()))?;
        let ok = bundle["sha256"] == hex::encode(Sha256::digest(artifact))
            && bundle["identity"] == signer.identity.as_str();
        ok.then_some(())
            .ok_or_else(|| SignatureError::Invalid("no".into()))
    }
}

struct FakeFetcher(HashMap<String, Vec<u8>>);

#[async_trait::async_trait]
impl Fetcher for FakeFetcher {
    async fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.0.get(url).cloned().ok_or_else(|| "404".into())
    }
}

fn fetcher() -> FakeFetcher {
    let (index, revocations) = (fixture("index.json"), fixture("revocations.json"));
    FakeFetcher(HashMap::from([
        (format!("{BASE}/index.json.sigstore.json"), sign(&index)),
        (
            format!("{BASE}/revocations.json.sigstore.json"),
            sign(&revocations),
        ),
        (format!("{BASE}/index.json"), index),
        (format!("{BASE}/revocations.json"), revocations),
    ]))
}

async fn market(dir: &Path) -> Marketplace<FakeFetcher, FakeVerifier> {
    let mut market = Marketplace::open(dir, fetcher(), FakeVerifier).unwrap();
    market.refresh(OFFICIAL_NAME).await.unwrap();
    market
}

/// Every path under `dir` with its length and modification time.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            out.insert(entry.path(), (meta.len(), meta.modified().unwrap()));
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    out
}

#[tokio::test]
async fn plan_install_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let before = snapshot(dir.path());
    for id in [
        "ostrium/loams-starter",
        "ostrium/github-mcp-server",
        "ostrium/plane-mcp-server",
        "ostrium/claude-code",
    ] {
        market.plan_install(id, None, None, &[]).unwrap();
    }
    assert_eq!(snapshot(dir.path()), before);
}

#[tokio::test]
async fn plan_lists_every_declared_permission() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let (plan, digest) = market
        .plan_install("ostrium/loams-starter", None, None, &[])
        .unwrap();
    let listing = market
        .get_listing("ostrium/loams-starter", None, None)
        .unwrap()
        .version
        .listing;

    // Every tool, with its risk and whether it always asks; most dangerous first.
    let declared: BTreeSet<String> = listing["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    let shown: BTreeSet<String> = plan.tools.iter().map(|t| t.name.clone()).collect();
    assert_eq!(shown, declared);
    assert_eq!(plan.tools[0].name, "create_issue");
    assert_eq!(plan.tools[0].risk, Risk::Egress);
    assert!(plan.tools[0].always_asks);
    assert!(plan.tools.windows(2).all(|w| w[0].risk >= w[1].risk));
    // Every data scope, host, secret, dependency, role and capability.
    let scopes: Vec<(String, String)> = plan
        .data
        .iter()
        .map(|d| (d.name.clone(), d.access.clone()))
        .collect();
    assert_eq!(
        scopes,
        [
            ("notes_vectors".into(), "read".into()),
            ("notes".into(), "write".into())
        ]
    );
    assert_eq!(plan.egress, ["api.github.com"]);
    assert_eq!(plan.secrets.len(), 1);
    assert_eq!(plan.secrets[0].name, "github_token");
    assert!(plan.secrets[0].required);
    assert_eq!(plan.dependencies.len(), 1);
    assert_eq!(plan.dependencies[0].id, "ostrium/github-mcp-server");
    assert!(!plan.dependencies[0].installed);
    assert_eq!(plan.roles, ["reader"]);
    assert_eq!(plan.capabilities, ["durable", "postgres", "qdrant"]);
    // Models, budgets, licence, tier, provenance and level.
    assert_eq!(plan.models["api"][0], "anthropic-messages");
    assert_eq!(plan.cost["budget_usd_per_day"], 5);
    assert_eq!(plan.licence, "Apache-2.0");
    assert_eq!(plan.tier, "first-party");
    assert_eq!(plan.provenance["type"], "npm-slsa");
    assert_eq!(
        plan.source_repository,
        "https://github.com/ostrium-labs/loams-mastra"
    );
    assert_eq!(plan.framework.as_deref(), Some("mastra"));
    assert_eq!(plan.level.as_deref(), Some("compatible"));
    assert!(plan.cannot.iter().any(|c| c.contains("api.github.com")));
    // The digest is stable and changes with the plan.
    assert_eq!(digest, plan::digest(&plan));
    assert_eq!(digest.len(), 64);
}

#[tokio::test]
async fn an_installed_dependency_is_reused() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let installed = [InstalledVersion {
        id: "ostrium/github-mcp-server".into(),
        version: "2.0.3".into(),
    }];
    let (plan, _) = market
        .plan_install("ostrium/loams-starter", None, None, &installed)
        .unwrap();
    assert!(plan.dependencies[0].installed);
}

#[tokio::test]
async fn a_remote_mcp_server_shows_its_host() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let (plan, _) = market
        .plan_install("ostrium/plane-mcp-server", None, None, &[])
        .unwrap();
    assert_eq!(plan.egress, ["mcp.plane.so"]);
    assert_eq!(plan.kind, "mcp-server");
}

#[tokio::test]
async fn a_revoked_version_cannot_be_planned() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    assert!(matches!(
        market.plan_install("ostrium/loams-starter", Some("0.1.0"), None, &[]),
        Err(MarketError::Revoked { .. })
    ));
}

#[tokio::test]
async fn widening_updates_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let (old, _) = market
        .plan_install("ostrium/loams-starter", None, None, &[])
        .unwrap();
    assert!(plan::widens(&old, &old).is_empty());
    let mut new = old.clone();
    new.egress.push("evil.example".into());
    new.tools[1].risk = Risk::Destructive;
    new.data[0].access = "write".into();
    new.tools[0].always_asks = false;
    let reasons = plan::widens(&old, &new);
    assert_eq!(reasons.len(), 4, "{reasons:?}");
    // Narrowing is not widening.
    let mut narrower = old.clone();
    narrower.tools.pop();
    narrower.egress.clear();
    assert!(plan::widens(&old, &narrower).is_empty());
    assert_ne!(plan::digest(&old), plan::digest(&new));
}

#[tokio::test]
async fn plan_shows_engine() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let view = market
        .get_listing("ostrium/loams-starter", None, None)
        .unwrap();
    // No `engine` in the listing: the node runtime runs on node.
    assert_eq!(plan::plan(&view, &[]).engine.as_deref(), Some("node"));
    let mut bun = view.clone();
    bun.version.listing["runtime"]["engine"] = "bun".into();
    let on_bun = plan::plan(&bun, &[]);
    assert_eq!(on_bun.engine.as_deref(), Some("bun"));
    assert_ne!(plan::digest(&on_bun), plan::digest(&plan::plan(&view, &[])));
    // Other runtimes have no engine.
    let mcp = market
        .get_listing("ostrium/github-mcp-server", None, None)
        .unwrap();
    assert_eq!(plan::plan(&mcp, &[]).engine, None);
}

#[tokio::test]
async fn engine_change_widens() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let (old, _) = market
        .plan_install("ostrium/loams-starter", None, None, &[])
        .unwrap();
    let mut new = old.clone();
    new.engine = Some("bun".into());
    assert_eq!(plan::widens(&old, &new), ["runs on bun instead of node"]);
    assert!(plan::widens(&old, &old).is_empty());
}

fn provisioned(dir: &Path) -> Provisioned {
    Provisioned {
        endpoint: "http://127.0.0.1:7700".into(),
        namespace: "place-notes".into(),
        data_dir: dir.join("agents/ostrium/loams-starter/data"),
        token_file: dir.join("agents/ostrium/loams-starter/token"),
        runtime_bin: Some(dir.join("runtimes/node-22/bin")),
        pg_url: Some("postgres://loams_agent_x@127.0.0.1:5432/notes".into()),
        qdrant: Some(("http://127.0.0.1:6333".into(), "env-key".into())),
        elasticsearch_url: Some("http://127.0.0.1:9200".into()),
        durable_url: Some("http://127.0.0.1:8001".into()),
        mcp_config: Some(dir.join("agents/ostrium/loams-starter/mcp.json")),
        model_proxy: Some("http://127.0.0.1:7801".into()),
        egress_proxy: Some("http://127.0.0.1:7802".into()),
        config: BTreeMap::from([
            ("tone".into(), "plain".into()),
            ("github_token".into(), "ghp_secret".into()),
        ]),
        provider_keys: BTreeMap::from([("ANTHROPIC_API_KEY".into(), "sk-real".into())]),
    }
}

#[tokio::test]
async fn agent_env_is_exactly_the_contract() {
    let dir = tempfile::tempdir().unwrap();
    let market = market(dir.path()).await;
    let listing = market
        .get_listing("ostrium/loams-starter", None, None)
        .unwrap()
        .version
        .listing;
    let p = provisioned(dir.path());
    let env = contract::environment(&listing, &p);
    let keys: BTreeSet<&str> = env.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "LOAMS_ENDPOINT",
        "LOAMS_NAMESPACE",
        "LOAMS_TOKEN_FILE",
        "LOAMS_DATA_DIR",
        "HOME",
        "TMPDIR",
        "PATH",
        // Declared: postgres, qdrant, durable. Not declared: elasticsearch.
        "LOAMS_PG_URL",
        "LOAMS_QDRANT_URL",
        "LOAMS_QDRANT_API_KEY",
        "LOAMS_DURABLE_URL",
        "LOAMS_MCP_CONFIG",
        "OPENAI_BASE_URL",
        "ANTHROPIC_BASE_URL",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        // The config input that names a variable.
        "GITHUB_TOKEN",
    ]
    .into();
    assert_eq!(keys, expected);
    // Through the proxy the agent gets a dummy key, never the user's.
    assert_eq!(env["ANTHROPIC_API_KEY"], PROXY_DUMMY_KEY);
    assert_eq!(
        env["PATH"],
        dir.path()
            .join("runtimes/node-22/bin")
            .display()
            .to_string()
    );
    assert!(env["HOME"].starts_with(&env["LOAMS_DATA_DIR"]));
    assert!(env["TMPDIR"].starts_with(&env["LOAMS_DATA_DIR"]));
    // Nothing from this process's own environment is copied.
    for (name, value) in std::env::vars() {
        if !expected.contains(name.as_str()) {
            assert!(!env.contains_key(&name), "{name} leaked");
        } else if name != "PATH" && name != "HOME" && name != "TMPDIR" {
            assert_ne!(env.get(&name), Some(&value), "{name} copied from the host");
        }
    }

    // A listing that declares nothing gets only the base contract.
    let bare = serde_json::json!({ "id": "ostrium/x" });
    let p = Provisioned {
        model_proxy: None,
        egress_proxy: None,
        mcp_config: None,
        ..provisioned(dir.path())
    };
    let keys: BTreeSet<String> = contract::environment(&bare, &p).into_keys().collect();
    let base: BTreeSet<String> = [
        "LOAMS_ENDPOINT",
        "LOAMS_NAMESPACE",
        "LOAMS_TOKEN_FILE",
        "LOAMS_DATA_DIR",
        "HOME",
        "TMPDIR",
        "PATH",
        // MK0 first-party agents without the proxy get the vault's key.
        "ANTHROPIC_API_KEY",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(keys, base);
}

#[test]
fn a_config_input_cannot_shadow_the_contract() {
    let listing = serde_json::json!({
        "config": [
            { "name": "a", "type": "string", "env": "LOAMS_ENDPOINT" },
            { "name": "b", "type": "string", "env": "HOME" },
            { "name": "c", "type": "string", "env": "LOAMS_NEW_THING" }
        ]
    });
    let dir = tempfile::tempdir().unwrap();
    let mut p = provisioned(dir.path());
    p.config = BTreeMap::from([
        ("a".into(), "http://evil".into()),
        ("b".into(), "/root".into()),
        ("c".into(), "x".into()),
    ]);
    let env = contract::environment(&listing, &p);
    assert_eq!(env["LOAMS_ENDPOINT"], "http://127.0.0.1:7700");
    assert_ne!(env["HOME"], "/root");
    assert!(!env.contains_key("LOAMS_NEW_THING"));
}

#[test]
fn the_mcp_config_holds_no_credentials() {
    let text = contract::mcp_config(&[ProxiedMcp {
        name: "github".into(),
        url: "http://127.0.0.1:7803/mcp/ostrium/github-mcp-server".into(),
    }]);
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        value["mcpServers"]["github"]["url"],
        "http://127.0.0.1:7803/mcp/ostrium/github-mcp-server"
    );
    assert_eq!(value["mcpServers"]["github"].as_object().unwrap().len(), 2);
}

#[test]
fn pg_roles_are_stable_and_short() {
    let role = contract::pg_role("ostrium/loams-starter");
    assert_eq!(role, contract::pg_role("ostrium/loams-starter"));
    assert_ne!(role, contract::pg_role("ostrium/other"));
    assert!(role.len() <= 63 && role.starts_with("loams_agent_"));
}

struct CountingIssuer(Mutex<u32>);

impl TokenIssuer for CountingIssuer {
    fn issue(&self, namespace: &str, lifetime: Duration) -> Result<String, String> {
        assert_eq!(lifetime, TOKEN_LIFETIME);
        let mut n = self.0.lock().unwrap();
        *n += 1;
        Ok(format!("token-{namespace}-{n}"))
    }
}

#[test]
fn token_file_rotates() {
    assert_eq!(TOKEN_LIFETIME, Duration::from_secs(900));
    assert_eq!(TOKEN_ROTATION, Duration::from_secs(600));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent/token");
    let mut file = TokenFile::new(
        path.clone(),
        "place-notes".into(),
        CountingIssuer(Mutex::new(0)),
    );
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    assert!(file.tick(t0).unwrap(), "the first tick writes a token");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "token-place-notes-1"
    );
    assert!(!file.tick(t0 + Duration::from_secs(599)).unwrap());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "token-place-notes-1"
    );
    assert!(file.tick(t0 + TOKEN_ROTATION).unwrap());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "token-place-notes-2"
    );
    // A token is replaced well before it expires.
    assert!(TOKEN_ROTATION < TOKEN_LIFETIME);
    assert_eq!(file.next_rotation(), Some(t0 + TOKEN_ROTATION * 2));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
