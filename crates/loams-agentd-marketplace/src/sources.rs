//! Marketplace sources (design §73 §5, D1462): ordered index URLs, each
//! pinned to the identity that must sign it. The official source, the Seed
//! Exchange, is built in and pinned to `loams-marketplace`'s publish workflow.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::verify::{GITHUB_ACTIONS_ISSUER, Identity};

pub const OFFICIAL_NAME: &str = "official";
/// Where the official index is served today: GitHub Pages of
/// `ostrium-labs/loams-marketplace` (MK1 Task 4 ruling 1). D1461's
/// `https://loams.dev/marketplace/index.json` replaces it when loams-site
/// serves the index.
pub const OFFICIAL_URL: &str = "https://ostrium-labs.github.io/loams-marketplace/index.json";
pub const OFFICIAL_IDENTITY: &str = "https://github.com/ostrium-labs/loams-marketplace/.github/workflows/publish.yml@refs/heads/main";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    /// The index URL; its signature is `<url>.sigstore.json`, and the
    /// revocation list `revocations.json` beside it.
    pub url: String,
    pub identity: String,
    pub issuer: String,
    #[serde(default)]
    pub official: bool,
}

impl Source {
    pub fn official() -> Self {
        Self {
            name: OFFICIAL_NAME.into(),
            url: OFFICIAL_URL.into(),
            identity: OFFICIAL_IDENTITY.into(),
            issuer: GITHUB_ACTIONS_ISSUER.into(),
            official: true,
        }
    }

    pub fn signer(&self) -> Identity {
        Identity {
            identity: self.identity.clone(),
            issuer: self.issuer.clone(),
        }
    }

    pub fn signature_url(&self) -> String {
        format!("{}.sigstore.json", self.url)
    }

    /// `revocations.json` in the index's directory.
    pub fn revocations_url(&self) -> String {
        match self.url.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/revocations.json"),
            None => "revocations.json".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("source names are 1 to 40 lower-case letters, digits and dashes: {0}")]
    BadName(String),
    #[error("a source URL is https and ends in .json: {0}")]
    BadUrl(String),
    #[error("a source needs the identity that signs it")]
    NoIdentity,
    #[error("a source named {0} exists")]
    Exists(String),
    #[error("no source named {0}")]
    Unknown(String),
    #[error("the official source cannot be removed")]
    Official,
    #[error("sources file {path}: {error}")]
    Io { path: String, error: String },
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Checks a new source's fields.
pub fn validate(source: &Source) -> Result<(), SourceError> {
    if !valid_name(&source.name) {
        return Err(SourceError::BadName(source.name.clone()));
    }
    if !source.url.starts_with("https://") || !source.url.ends_with(".json") {
        return Err(SourceError::BadUrl(source.url.clone()));
    }
    if source.identity.trim().is_empty() || source.issuer.trim().is_empty() {
        return Err(SourceError::NoIdentity);
    }
    Ok(())
}

/// The ordered source list, kept at `<data>/marketplace/sources.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sources {
    path: PathBuf,
    list: Vec<Source>,
}

#[derive(Serialize, Deserialize)]
struct File {
    sources: Vec<Source>,
}

impl Sources {
    /// Loads the list; a missing file is the official source alone. The
    /// official source is always first and always pinned to its built-in
    /// identity, whatever the file says.
    pub fn load(path: &Path) -> Result<Self, SourceError> {
        let io = |e: std::io::Error| SourceError::Io {
            path: path.display().to_string(),
            error: e.to_string(),
        };
        let mut list = match std::fs::read(path) {
            Ok(bytes) => {
                serde_json::from_slice::<File>(&bytes)
                    .map_err(|e| SourceError::Io {
                        path: path.display().to_string(),
                        error: e.to_string(),
                    })?
                    .sources
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(io(e)),
        };
        list.retain(|s| !s.official && s.name != OFFICIAL_NAME);
        list.insert(0, Source::official());
        Ok(Self {
            path: path.to_owned(),
            list,
        })
    }

    pub fn list(&self) -> &[Source] {
        &self.list
    }

    pub fn get(&self, name: &str) -> Option<&Source> {
        self.list.iter().find(|s| s.name == name)
    }

    /// Adds a source after the others. The caller verifies its index first.
    pub fn add(&mut self, source: Source) -> Result<(), SourceError> {
        validate(&source)?;
        if self.get(&source.name).is_some() {
            return Err(SourceError::Exists(source.name));
        }
        self.list.push(Source {
            official: false,
            ..source
        });
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<Source, SourceError> {
        if name == OFFICIAL_NAME {
            return Err(SourceError::Official);
        }
        let at = self
            .list
            .iter()
            .position(|s| s.name == name)
            .ok_or_else(|| SourceError::Unknown(name.into()))?;
        let removed = self.list.remove(at);
        self.save()?;
        Ok(removed)
    }

    fn save(&self) -> Result<(), SourceError> {
        let io = |e: std::io::Error| SourceError::Io {
            path: self.path.display().to_string(),
            error: e.to_string(),
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let file = File {
            sources: self.list.iter().filter(|s| !s.official).cloned().collect(),
        };
        let text = serde_json::to_vec_pretty(&file).map_err(|e| SourceError::Io {
            path: self.path.display().to_string(),
            error: e.to_string(),
        })?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }
}
