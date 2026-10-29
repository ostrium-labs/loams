//! Install is consent (design §73 §7.2, D1466; MK1 Task 5). `plan` turns a
//! listing into the consent screen's content: the Place the agent will get,
//! its tools by risk, the hosts it may reach, the MCP servers it needs, the
//! secrets to supply, models, budgets, licence, tier and provenance. It is a
//! pure function: it creates no Place, role, key, file or process. The plan's
//! digest binds `Install` to exactly what the person saw.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::index::canonical;
use crate::market::ListingView;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Risk {
    Read,
    Write,
    Egress,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanTool {
    pub name: String,
    pub description: String,
    pub risk: Risk,
    pub always_asks: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataScope {
    pub kind: String,
    pub name: String,
    pub access: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanSecret {
    pub name: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDependency {
    pub id: String,
    /// The version range the listing asks for.
    pub version: String,
    /// Already installed at a version in range, so it is reused.
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstallPlan {
    pub id: String,
    pub version: String,
    pub source: String,
    pub kind: String,
    pub title: String,
    pub tier: String,
    pub data: Vec<DataScope>,
    pub roles: Vec<String>,
    /// Ordered by risk, most dangerous first, then by name.
    pub tools: Vec<PlanTool>,
    pub egress: Vec<String>,
    pub dependencies: Vec<PlanDependency>,
    pub secrets: Vec<PlanSecret>,
    /// The Loams surfaces it will be given (`postgres`, `qdrant`, …).
    pub capabilities: Vec<String>,
    pub models: Value,
    pub cost: Value,
    pub licence: String,
    pub provenance: Value,
    pub source_repository: String,
    pub framework: Option<String>,
    /// The `node` runtime's engine, `node` or `bun` (D1808); `None` for other
    /// runtimes. Bound by the digest, so the person consents to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    pub level: Option<String>,
    /// What the agent is not allowed, in plain words.
    pub cannot: Vec<String>,
}

/// An installed listing, for dependency reuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledVersion {
    pub id: String,
    pub version: String,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn str_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn risk_of(value: &Value) -> Risk {
    match value.as_str() {
        Some("read") => Risk::Read,
        Some("write") => Risk::Write,
        Some("egress") => Risk::Egress,
        // The schema allows only the four; anything else is treated as the worst.
        _ => Risk::Destructive,
    }
}

/// The host of an https URL.
fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn version_in(range: &str, version: &str) -> bool {
    match (
        semver::VersionReq::parse(range),
        semver::Version::parse(version),
    ) {
        (Ok(req), Ok(v)) => req.matches(&v),
        _ => false,
    }
}

/// Builds the consent plan for `view`'s version. Pure: no side effects.
pub fn plan(view: &ListingView, installed: &[InstalledVersion]) -> InstallPlan {
    let listing = &view.version.listing;
    let place = listing.get("place").cloned().unwrap_or(Value::Null);
    let always_ask: BTreeSet<String> = strings(&place["always_ask"]).into_iter().collect();

    let mut tools: Vec<PlanTool> = listing["tools"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|t| PlanTool {
                    name: str_of(t, "name"),
                    description: str_of(t, "description"),
                    risk: risk_of(&t["risk"]),
                    always_asks: always_ask.contains(&str_of(t, "name")),
                    mcp_server: t
                        .get("mcp_server")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                })
                .collect()
        })
        .unwrap_or_default();
    tools.sort_by(|a, b| b.risk.cmp(&a.risk).then_with(|| a.name.cmp(&b.name)));

    let mut data: Vec<DataScope> = place["data"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|d| DataScope {
                    kind: str_of(d, "kind"),
                    name: str_of(d, "name"),
                    access: str_of(d, "access"),
                })
                .collect()
        })
        .unwrap_or_default();
    data.sort();

    let mut egress: BTreeSet<String> = strings(&place["egress"]).into_iter().collect();
    // A remote MCP server is reached at its URL: that host is egress too.
    if let Some(host) = listing["mcp_server"]["remote_url"]
        .as_str()
        .and_then(host_of)
    {
        egress.insert(host);
    }

    let dependencies = listing["mcp_servers"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|d| {
                    let id = str_of(d, "id");
                    let version = str_of(d, "version");
                    let installed = installed
                        .iter()
                        .any(|i| i.id == id && version_in(&version, &i.version));
                    PlanDependency {
                        id,
                        version,
                        installed,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let secrets = listing["config"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|c| c["secret"].as_bool() == Some(true))
                .map(|c| PlanSecret {
                    name: str_of(c, "name"),
                    description: str_of(c, "description"),
                    required: c["required"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();

    let capabilities: Vec<String> = listing["loams"]
        .as_object()
        .map(|caps| {
            caps.iter()
                .filter(|(_, on)| on.as_bool() == Some(true))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();

    let tier = match view.summary.tier {
        crate::index::Tier::FirstParty => "first-party",
        crate::index::Tier::Verified => "verified",
        crate::index::Tier::Community => "community",
    };

    let mut plan = InstallPlan {
        id: view.summary.id.clone(),
        version: view.version.version.clone(),
        source: view.summary.source.clone(),
        kind: view.summary.kind.as_str().into(),
        title: view.summary.title.clone(),
        tier: tier.into(),
        data,
        roles: strings(&place["roles"]),
        tools,
        egress: egress.into_iter().collect(),
        dependencies,
        secrets,
        capabilities,
        models: listing.get("models").cloned().unwrap_or(Value::Null),
        cost: listing.get("cost").cloned().unwrap_or(Value::Null),
        licence: str_of(listing, "licence"),
        provenance: listing.get("provenance").cloned().unwrap_or(Value::Null),
        source_repository: str_of(&listing["source"], "repository"),
        framework: listing["framework"]["name"].as_str().map(str::to_owned),
        engine: crate::engine::Engine::of(listing).map(|e| e.as_str().to_owned()),
        level: listing["conformance"]["level"].as_str().map(str::to_owned),
        cannot: Vec::new(),
    };
    plan.cannot = cannot(&plan);
    plan
}

/// The "can't" half of the consent screen (§60 Q937), from the plan itself.
fn cannot(plan: &InstallPlan) -> Vec<String> {
    let mut out = vec![
        "Read your own credentials, dotfiles or keychain.".to_owned(),
        format!("Call any tool other than the {} listed.", plan.tools.len()),
    ];
    if plan.egress.is_empty() {
        out.push("Reach the network, except the model proxy.".into());
    } else {
        out.push(format!(
            "Reach any host other than {} and the model proxy.",
            plan.egress.join(", ")
        ));
    }
    if plan.data.is_empty() {
        out.push("Read or write any of your data in Loams.".into());
    } else {
        let writes: Vec<&DataScope> = plan.data.iter().filter(|d| d.access == "write").collect();
        if writes.is_empty() {
            out.push("Write to any of your data in Loams.".into());
        }
        out.push("Touch data outside the scopes listed.".into());
    }
    out.push("Write files outside its own data directory.".into());
    out
}

/// The SHA-256 of the plan's canonical JSON: `Install` must present it.
pub fn digest(plan: &InstallPlan) -> String {
    let value = serde_json::to_value(plan).unwrap_or(Value::Null);
    hex::encode(Sha256::digest(canonical(&value).as_bytes()))
}

fn access_rank(access: &str) -> u8 {
    if access == "write" { 2 } else { 1 }
}

/// Whether `new` asks for anything `old` did not: a tool or a higher risk, a
/// host, a data scope or write access, a secret, a dependency, a role or a
/// Loams capability, or another engine. Such an update needs consent again
/// (D1466).
pub fn widens(old: &InstallPlan, new: &InstallPlan) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(was), Some(now)) = (&old.engine, &new.engine)
        && was != now
    {
        out.push(format!("runs on {now} instead of {was}"));
    }
    for tool in &new.tools {
        match old.tools.iter().find(|t| t.name == tool.name) {
            None => out.push(format!("new tool {} ({:?})", tool.name, tool.risk)),
            Some(was) if tool.risk > was.risk => {
                out.push(format!(
                    "tool {} goes from {:?} to {:?}",
                    tool.name, was.risk, tool.risk
                ));
            }
            Some(was) if was.always_asks && !tool.always_asks => {
                out.push(format!("tool {} no longer asks first", tool.name));
            }
            _ => {}
        }
    }
    for host in &new.egress {
        if !old.egress.contains(host) {
            out.push(format!("new host {host}"));
        }
    }
    for scope in &new.data {
        match old
            .data
            .iter()
            .find(|d| d.kind == scope.kind && d.name == scope.name)
        {
            None => out.push(format!(
                "new {} {} ({})",
                scope.kind, scope.name, scope.access
            )),
            Some(was) if access_rank(&scope.access) > access_rank(&was.access) => {
                out.push(format!("{} {} becomes writable", scope.kind, scope.name));
            }
            _ => {}
        }
    }
    for secret in &new.secrets {
        if !old.secrets.iter().any(|s| s.name == secret.name) {
            out.push(format!("new secret {}", secret.name));
        }
    }
    for dep in &new.dependencies {
        if !old.dependencies.iter().any(|d| d.id == dep.id) {
            out.push(format!("new MCP server {}", dep.id));
        }
    }
    for role in &new.roles {
        if !old.roles.contains(role) {
            out.push(format!("new role {role}"));
        }
    }
    for capability in &new.capabilities {
        if !old.capabilities.contains(capability) {
            out.push(format!("new Loams capability {capability}"));
        }
    }
    out
}
