//! The marketplace core `MarketplaceService` serves (MK1 Task 4): sources,
//! fetching and verifying each source's signed index and revocation list,
//! the verified cache, search, and the revocation check at start.
//!
//! Trust rule: bytes from a source are used only after both bundles verify
//! against the source's pinned identity, and the on-disk cache is verified
//! again when it is loaded, so a cache file edited on disk is refused too.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::index::{self, Index, IndexEntry, IndexVersion, Kind, Level, Tier};
use crate::revocations::{self, Finding, Installed, Revocation, Revocations};
use crate::sources::{Source, SourceError, Sources};
use crate::verify::{SignatureError, SignatureVerifier};

/// Fetches a URL's bytes. agentd's implementation is an HTTPS client; tests
/// serve fixtures.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
    async fn get(&self, url: &str) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MarketError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("{url}: {error}")]
    Fetch { url: String, error: String },
    #[error("{file} of source {source_name}: {error}")]
    Signature {
        source_name: String,
        file: &'static str,
        error: SignatureError,
    },
    #[error("source {source_name}: {error}")]
    Index { source_name: String, error: String },
    #[error("source {0} has no verified index yet")]
    NotLoaded(String),
    #[error("no listing {0}")]
    NoListing(String),
    #[error("{id} has no version {version}")]
    NoVersion { id: String, version: String },
    #[error("{0}: {1}")]
    Cache(String, String),
    #[error("{id}@{version} was yanked: {reason}")]
    Yanked {
        id: String,
        version: String,
        reason: String,
    },
    #[error("{id}@{version} is revoked: {reason}")]
    Revoked {
        id: String,
        version: String,
        reason: String,
    },
}

/// One source's verified state.
#[derive(Debug, Clone)]
pub struct Verified {
    pub index: Index,
    pub revocations: Revocations,
    pub refreshed_ms: i64,
}

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub source: Source,
    pub refreshed_ms: i64,
    pub error: Option<String>,
    pub listings: usize,
}

/// A search: every filter that is set must match.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// Words, each matched case-insensitively against the id, title, summary
    /// and tool names of the latest version.
    pub text: String,
    pub kinds: Vec<Kind>,
    /// Framework names (`mastra`, `langgraph`, …).
    pub frameworks: Vec<String>,
    pub languages: Vec<String>,
    /// The lowest conformance level a listing must have passed.
    pub min_level: Option<Level>,
    pub tiers: Vec<Tier>,
    /// Only this source.
    pub source: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub id: String,
    pub source: String,
    pub kind: Kind,
    pub title: String,
    pub summary: String,
    pub tier: Tier,
    pub latest: Option<String>,
    pub framework: Option<String>,
    pub language: Option<String>,
    pub level: Option<Level>,
    pub revocation: Option<Revocation>,
}

#[derive(Debug, Clone)]
pub struct ListingView {
    pub summary: Summary,
    pub version: IndexVersion,
    /// Every version, ascending, with its revocation if any.
    pub versions: Vec<(IndexVersion, Option<Revocation>)>,
}

const INDEX: &str = "index.json";
const INDEX_SIG: &str = "index.json.sigstore.json";
const REVOCATIONS: &str = "revocations.json";
const REVOCATIONS_SIG: &str = "revocations.json.sigstore.json";

pub struct Marketplace<F, V> {
    dir: PathBuf,
    sources: Sources,
    verified: HashMap<String, Verified>,
    errors: HashMap<String, String>,
    fetcher: F,
    verifier: V,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// The four files of a source, as fetched or as cached.
struct Files {
    index: Vec<u8>,
    index_sig: Vec<u8>,
    revocations: Vec<u8>,
    revocations_sig: Vec<u8>,
}

impl<F, V> std::fmt::Debug for Marketplace<F, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Marketplace")
            .field("dir", &self.dir)
            .field("sources", &self.sources)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

impl<F: Fetcher, V: SignatureVerifier> Marketplace<F, V> {
    /// Opens the marketplace in `<data>/marketplace`: the sources, and each
    /// source's cached index, verified again before it is used.
    pub fn open(data_dir: &Path, fetcher: F, verifier: V) -> Result<Self, MarketError> {
        let dir = data_dir.join("marketplace");
        let sources = Sources::load(&dir.join("sources.json"))?;
        let mut market = Self {
            dir,
            sources,
            verified: HashMap::new(),
            errors: HashMap::new(),
            fetcher,
            verifier,
        };
        for source in market.sources.list().to_vec() {
            match market.read_cache(&source) {
                Ok(Some(files)) => match market.check(&source, &files) {
                    Ok((index, revocations)) => {
                        let refreshed_ms = std::fs::metadata(market.cache_dir(&source).join(INDEX))
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                            .map(|d| i64::try_from(d.as_millis()).unwrap_or(0))
                            .unwrap_or(0);
                        market.verified.insert(
                            source.name.clone(),
                            Verified {
                                index,
                                revocations,
                                refreshed_ms,
                            },
                        );
                    }
                    Err(error) => {
                        market.errors.insert(source.name.clone(), error.to_string());
                    }
                },
                Ok(None) => {}
                Err(error) => {
                    market.errors.insert(source.name.clone(), error.to_string());
                }
            }
        }
        Ok(market)
    }

    fn cache_dir(&self, source: &Source) -> PathBuf {
        self.dir.join("cache").join(&source.name)
    }

    fn read_cache(&self, source: &Source) -> Result<Option<Files>, MarketError> {
        let dir = self.cache_dir(source);
        if !dir.join(INDEX).exists() {
            return Ok(None);
        }
        let read = |name: &str| {
            std::fs::read(dir.join(name)).map_err(|e| {
                MarketError::Cache(dir.join(name).display().to_string(), e.to_string())
            })
        };
        // A missing signature is read as empty, so the check refuses it.
        let read_sig = |name: &str| std::fs::read(dir.join(name)).unwrap_or_default();
        Ok(Some(Files {
            index: read(INDEX)?,
            index_sig: read_sig(INDEX_SIG),
            revocations: read(REVOCATIONS)?,
            revocations_sig: read_sig(REVOCATIONS_SIG),
        }))
    }

    fn write_cache(&self, source: &Source, files: &Files) -> Result<(), MarketError> {
        let dir = self.cache_dir(source);
        let err = |e: std::io::Error| MarketError::Cache(dir.display().to_string(), e.to_string());
        std::fs::create_dir_all(&dir).map_err(err)?;
        for (name, bytes) in [
            (INDEX_SIG, &files.index_sig),
            (REVOCATIONS_SIG, &files.revocations_sig),
            (REVOCATIONS, &files.revocations),
            (INDEX, &files.index),
        ] {
            let tmp = dir.join(format!("{name}.tmp"));
            std::fs::write(&tmp, bytes).map_err(err)?;
            std::fs::rename(&tmp, dir.join(name)).map_err(err)?;
        }
        Ok(())
    }

    /// Verifies both signatures, then parses. Nothing is parsed before its
    /// signature verifies.
    fn check(&self, source: &Source, files: &Files) -> Result<(Index, Revocations), MarketError> {
        let signer = source.signer();
        for (file, bytes, sig) in [
            (INDEX, &files.index, &files.index_sig),
            (REVOCATIONS, &files.revocations, &files.revocations_sig),
        ] {
            if sig.is_empty() {
                return Err(MarketError::Signature {
                    source_name: source.name.clone(),
                    file,
                    error: SignatureError::Missing(format!("{file}.sigstore.json")),
                });
            }
            self.verifier
                .verify_blob(bytes, sig, &signer)
                .map_err(|error| MarketError::Signature {
                    source_name: source.name.clone(),
                    file,
                    error,
                })?;
        }
        let index = index::parse(&files.index).map_err(|e| MarketError::Index {
            source_name: source.name.clone(),
            error: e.to_string(),
        })?;
        let revocations =
            Revocations::parse(&files.revocations).map_err(|e| MarketError::Index {
                source_name: source.name.clone(),
                error: e.to_string(),
            })?;
        Ok((index, revocations))
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>, MarketError> {
        self.fetcher
            .get(url)
            .await
            .map_err(|error| MarketError::Fetch {
                url: url.into(),
                error,
            })
    }

    /// A missing signature (HTTP 404) reads as empty, so `check` names it.
    async fn fetch_signature(&self, url: &str) -> Vec<u8> {
        self.fetcher.get(url).await.unwrap_or_default()
    }

    async fn fetch_files(&self, source: &Source) -> Result<Files, MarketError> {
        Ok(Files {
            index: self.fetch(&source.url).await?,
            index_sig: self.fetch_signature(&source.signature_url()).await,
            revocations: self.fetch(&source.revocations_url()).await?,
            revocations_sig: self
                .fetch_signature(&format!("{}.sigstore.json", source.revocations_url()))
                .await,
        })
    }

    /// Fetches and verifies a source. On failure the last verified index stays
    /// in use and the error is reported on the source.
    pub async fn refresh(&mut self, name: &str) -> Result<(), MarketError> {
        let source = self
            .sources
            .get(name)
            .cloned()
            .ok_or_else(|| SourceError::Unknown(name.into()))?;
        let result = async {
            let files = self.fetch_files(&source).await?;
            let (index, revocations) = self.check(&source, &files)?;
            self.write_cache(&source, &files)?;
            Ok::<_, MarketError>(Verified {
                index,
                revocations,
                refreshed_ms: now_ms(),
            })
        }
        .await;
        match result {
            Ok(verified) => {
                self.verified.insert(source.name.clone(), verified);
                self.errors.remove(&source.name);
                Ok(())
            }
            Err(error) => {
                self.errors.insert(source.name.clone(), error.to_string());
                Err(error)
            }
        }
    }

    /// Refreshes every source; returns the ones that failed.
    pub async fn refresh_all(&mut self) -> Vec<(String, MarketError)> {
        let mut failed = Vec::new();
        for name in self
            .sources
            .list()
            .iter()
            .map(|s| s.name.clone())
            .collect::<Vec<_>>()
        {
            if let Err(error) = self.refresh(&name).await {
                failed.push((name, error));
            }
        }
        failed
    }

    /// Adds a source only once its index and revocations verify against the
    /// identity it is pinned to.
    pub async fn add_source(&mut self, source: Source) -> Result<SourceInfo, MarketError> {
        crate::sources::validate(&source)?;
        if self.sources.get(&source.name).is_some() {
            return Err(SourceError::Exists(source.name).into());
        }
        let files = self.fetch_files(&source).await?;
        let (index, revocations) = self.check(&source, &files)?;
        self.sources.add(source.clone())?;
        self.write_cache(&source, &files)?;
        self.verified.insert(
            source.name.clone(),
            Verified {
                index,
                revocations,
                refreshed_ms: now_ms(),
            },
        );
        self.info(&source.name)
            .ok_or_else(|| SourceError::Unknown(source.name).into())
    }

    /// Removes a source and its cache. Installed listings are not touched.
    pub fn remove_source(&mut self, name: &str) -> Result<(), MarketError> {
        let removed = self.sources.remove(name)?;
        self.verified.remove(name);
        self.errors.remove(name);
        let dir = self.cache_dir(&removed);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| MarketError::Cache(dir.display().to_string(), e.to_string()))?;
        }
        Ok(())
    }

    fn info(&self, name: &str) -> Option<SourceInfo> {
        let source = self.sources.get(name)?.clone();
        let verified = self.verified.get(name);
        Some(SourceInfo {
            refreshed_ms: verified.map_or(0, |v| v.refreshed_ms),
            error: self.errors.get(name).cloned(),
            listings: verified.map_or(0, |v| v.index.listings.len()),
            source,
        })
    }

    pub fn list_sources(&self) -> Vec<SourceInfo> {
        self.sources
            .list()
            .iter()
            .filter_map(|s| self.info(&s.name))
            .collect()
    }

    pub fn verified(&self, source: &str) -> Option<&Verified> {
        self.verified.get(source)
    }

    fn summary(source: &str, entry: &IndexEntry, revocations: &Revocations) -> Summary {
        let latest = entry.latest_version();
        Summary {
            id: entry.id.clone(),
            source: source.into(),
            kind: entry.kind.clone(),
            title: entry.title.clone(),
            summary: entry.summary.clone(),
            tier: entry.tier,
            latest: entry.latest.clone(),
            framework: latest.and_then(|v| v.meta.framework.as_ref().map(|f| f.name.clone())),
            language: latest.and_then(|v| v.meta.language.clone()),
            level: latest.and_then(|v| v.meta.conformance.as_ref().map(|c| c.level)),
            revocation: entry
                .latest
                .as_deref()
                .and_then(|v| revocations.for_version(&entry.id, v))
                .cloned(),
        }
    }

    fn matches(query: &Query, entry: &IndexEntry, summary: &Summary) -> bool {
        if !query.kinds.is_empty() && !query.kinds.contains(&summary.kind) {
            return false;
        }
        if !query.tiers.is_empty() && !query.tiers.contains(&summary.tier) {
            return false;
        }
        if !query.frameworks.is_empty()
            && !summary
                .framework
                .as_ref()
                .is_some_and(|f| query.frameworks.contains(f))
        {
            return false;
        }
        if !query.languages.is_empty()
            && !summary
                .language
                .as_ref()
                .is_some_and(|l| query.languages.contains(l))
        {
            return false;
        }
        if let Some(min) = query.min_level
            && !summary.level.is_some_and(|level| level >= min)
        {
            return false;
        }
        let words: Vec<String> = query
            .text
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if words.is_empty() {
            return true;
        }
        let mut haystack = format!(
            "{} {} {}",
            entry.id.to_lowercase(),
            entry.title.to_lowercase(),
            entry.summary.to_lowercase()
        );
        if let Some(latest) = entry.latest_version() {
            for tool in &latest.meta.tools {
                haystack.push(' ');
                haystack.push_str(&tool.name.to_lowercase());
            }
        }
        words.iter().all(|w| haystack.contains(w.as_str()))
    }

    /// Searches the verified indexes, in source order, then by id. Listings
    /// whose every version is yanked are left out.
    pub fn search(&self, query: &Query) -> Vec<Summary> {
        let mut out = Vec::new();
        for source in self.sources.list() {
            if query.source.as_ref().is_some_and(|s| s != &source.name) {
                continue;
            }
            let Some(verified) = self.verified.get(&source.name) else {
                continue;
            };
            for entry in &verified.index.listings {
                if entry.latest.is_none() {
                    continue;
                }
                let summary = Self::summary(&source.name, entry, &verified.revocations);
                if Self::matches(query, entry, &summary) {
                    out.push(summary);
                }
            }
        }
        out
    }

    /// One listing at `version` (default: latest) from `source` (default: the
    /// first source that has it).
    pub fn get_listing(
        &self,
        id: &str,
        version: Option<&str>,
        source: Option<&str>,
    ) -> Result<ListingView, MarketError> {
        for candidate in self.sources.list() {
            if source.is_some_and(|s| s != candidate.name) {
                continue;
            }
            let Some(verified) = self.verified.get(&candidate.name) else {
                if source.is_some() {
                    return Err(MarketError::NotLoaded(candidate.name.clone()));
                }
                continue;
            };
            let Some(entry) = verified.index.listings.iter().find(|e| e.id == id) else {
                continue;
            };
            let wanted = match version {
                Some(v) => entry.version(v).ok_or_else(|| MarketError::NoVersion {
                    id: id.into(),
                    version: v.into(),
                })?,
                None => entry
                    .latest_version()
                    .ok_or_else(|| MarketError::NoListing(id.into()))?,
            };
            let versions = entry
                .versions
                .iter()
                .map(|v| {
                    (
                        v.clone(),
                        verified.revocations.for_version(id, &v.version).cloned(),
                    )
                })
                .collect();
            return Ok(ListingView {
                summary: Self::summary(&candidate.name, entry, &verified.revocations),
                version: wanted.clone(),
                versions,
            });
        }
        Err(MarketError::NoListing(id.into()))
    }

    /// The consent plan for an install (D1466) and its digest. Creates
    /// nothing: no Place, role, key, file or process.
    pub fn plan_install(
        &self,
        id: &str,
        version: Option<&str>,
        source: Option<&str>,
        installed: &[crate::plan::InstalledVersion],
    ) -> Result<(crate::plan::InstallPlan, String), MarketError> {
        let view = self.get_listing(id, version, source)?;
        if let Some(reason) = &view.version.yanked {
            return Err(MarketError::Yanked {
                id: id.into(),
                version: view.version.version.clone(),
                reason: reason.clone(),
            });
        }
        if let Some((_, Some(revocation))) = view
            .versions
            .iter()
            .find(|(v, _)| v.version == view.version.version)
            && revocation.severity == crate::revocations::Severity::Disable
        {
            return Err(MarketError::Revoked {
                id: id.into(),
                version: view.version.version.clone(),
                reason: revocation.reason.clone(),
            });
        }
        let plan = crate::plan::plan(&view, installed);
        let digest = crate::plan::digest(&plan);
        Ok((plan, digest))
    }

    /// The revocation check (start, and every 6 hours after a refresh): every
    /// installed listing covered by its source's revocation list.
    pub fn check_revocations(&self, installed: &[Installed]) -> Vec<Finding> {
        revocations::check(installed, |source| {
            self.verified.get(source).map(|v| &v.revocations)
        })
    }
}
