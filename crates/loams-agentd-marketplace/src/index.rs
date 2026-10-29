//! A source's index (`schemas/marketplace/index.v1.json`, MK1 Task 2 ruling
//! 4): every listing version in full, so one signature covers them all. The
//! types read what agentd searches and plans with; each listing is kept as
//! JSON too, and checked against schema v1 when the index is loaded.

use serde::Deserialize;
use serde_json::Value;

use crate::listing;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Agent,
    Harness,
    McpServer,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Agent => "agent",
            Kind::Harness => "harness",
            Kind::McpServer => "mcp-server",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    FirstParty,
    Verified,
    Community,
}

/// LAPC levels, ordered: `Core < Compatible < Native` (design §73 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Level {
    Core,
    Compatible,
    Native,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Framework {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Conformance {
    pub level: Level,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tool {
    pub name: String,
}

/// The fields of a listing that search reads.
#[derive(Debug, Clone, Deserialize)]
pub struct ListingMeta {
    pub id: String,
    pub kind: Kind,
    pub version: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub framework: Option<Framework>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub conformance: Option<Conformance>,
    #[serde(default)]
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone)]
pub struct IndexVersion {
    pub version: String,
    pub yanked: Option<String>,
    pub listing_sha256: String,
    pub meta: ListingMeta,
    /// The listing as the index holds it.
    pub listing: Value,
}

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    pub tier: Tier,
    pub latest: Option<String>,
    /// Ascending by semantic-version precedence, as the index builds them.
    pub versions: Vec<IndexVersion>,
}

impl IndexEntry {
    pub fn version(&self, version: &str) -> Option<&IndexVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    /// The version a new install gets.
    pub fn latest_version(&self) -> Option<&IndexVersion> {
        self.latest.as_deref().and_then(|v| self.version(v))
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    pub source_commit: Option<String>,
    pub listings: Vec<IndexEntry>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IndexError {
    #[error("the index is not JSON: {0}")]
    NotJson(String),
    #[error("the index is not index v1: {0}")]
    Shape(String),
    #[error("{id}@{version} does not match listing schema v1: {issues}")]
    Listing {
        id: String,
        version: String,
        issues: String,
    },
    #[error("{id}@{version}: listing_sha256 does not match the listing")]
    Digest { id: String, version: String },
}

#[derive(Deserialize)]
struct RawIndex {
    index_version: u32,
    #[serde(default)]
    source_commit: Option<String>,
    listings: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    id: String,
    kind: Kind,
    title: String,
    summary: String,
    tier: Tier,
    #[serde(default)]
    latest: Option<String>,
    versions: Vec<RawVersion>,
}

#[derive(Deserialize)]
struct RawVersion {
    version: String,
    #[serde(default)]
    yanked: Option<String>,
    listing_sha256: String,
    listing: Value,
}

/// `JSON.stringify(sortKeys(value), null, 2) + "\n"`, the canonical form
/// `loams-listing` hashes (`canonical` in index-build.ts).
pub fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for key in keys {
                    out.insert(key.clone(), sorted(&map[key]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    // `sorted` inserts keys in order, so the output is sorted whether or not
    // serde_json's `preserve_order` feature is on in the build.
    let mut text = serde_json::to_string_pretty(&sorted(value)).unwrap_or_default();
    text.push('\n');
    text
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Parses verified index bytes. Call only after the signature check.
pub fn parse(bytes: &[u8]) -> Result<Index, IndexError> {
    let raw: Value =
        serde_json::from_slice(bytes).map_err(|e| IndexError::NotJson(e.to_string()))?;
    let raw: RawIndex =
        serde_json::from_value(raw).map_err(|e| IndexError::Shape(e.to_string()))?;
    if raw.index_version != 1 {
        return Err(IndexError::Shape(format!(
            "index_version {} is not 1",
            raw.index_version
        )));
    }
    let mut listings = Vec::with_capacity(raw.listings.len());
    for entry in raw.listings {
        let mut versions = Vec::with_capacity(entry.versions.len());
        for version in entry.versions {
            if let Err(issues) = listing::validate(&version.listing) {
                return Err(IndexError::Listing {
                    id: entry.id.clone(),
                    version: version.version.clone(),
                    issues: issues
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; "),
                });
            }
            if sha256_hex(&canonical(&version.listing)) != version.listing_sha256 {
                return Err(IndexError::Digest {
                    id: entry.id.clone(),
                    version: version.version.clone(),
                });
            }
            let meta: ListingMeta = serde_json::from_value(version.listing.clone())
                .map_err(|e| IndexError::Shape(format!("{}: {e}", entry.id)))?;
            if meta.id != entry.id || meta.version != version.version {
                return Err(IndexError::Shape(format!(
                    "{}@{} holds the listing {}@{}",
                    entry.id, version.version, meta.id, meta.version
                )));
            }
            versions.push(IndexVersion {
                version: version.version,
                yanked: version.yanked,
                listing_sha256: version.listing_sha256,
                meta,
                listing: version.listing,
            });
        }
        listings.push(IndexEntry {
            id: entry.id,
            kind: entry.kind,
            title: entry.title,
            summary: entry.summary,
            tier: entry.tier,
            latest: entry.latest,
            versions,
        });
    }
    Ok(Index {
        source_commit: raw.source_commit,
        listings,
    })
}
