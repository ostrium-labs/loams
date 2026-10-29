//! Where an artifact's bytes come from (MS1 Task 5, D1812).
//!
//! An artifact has its own location: the npm registry's tarball, or an
//! archive file's `url`. A listing may add public mirrors beside it, today
//! only `{ "kind": "walrus", "blob_id", "sha256" }`. With Walrus enabled the
//! mirror is read first through `loams-walrus`, which checks every byte
//! against the listing's SHA-256 (the listing is covered by the source's
//! signed index; the index carries the reference, never the bytes, D1460).
//! On any Walrus failure the HTTPS location is used. With Walrus disabled
//! only the HTTPS location is, and Walrus is never called.
//!
//! Fetching is all this module does: the caller then checks integrity and
//! provenance exactly as for any artifact (D1459), whichever way the bytes
//! came.

use loams_walrus::{BlobId, ContentDigest, Walrus, WalrusError};
use serde_json::Value;

use crate::market::Fetcher;

/// The public npm registry, the default `npm_registry`.
pub const NPM_REGISTRY: &str = "https://registry.npmjs.org";

/// A Walrus mirror of an artifact's bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalrusLocation {
    pub blob_id: BlobId,
    pub sha256: ContentDigest,
}

/// Every place one artifact file can be read from, in the order tried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactSources {
    pub walrus: Vec<WalrusLocation>,
    /// The artifact's own location; `None` when this node has none (for an
    /// npm artifact, no registry configured).
    pub https: Option<String>,
}

/// Where the bytes came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Walrus { blob_id: String },
    Https { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("the artifact has no location this node can read")]
    NoLocation,
    #[error("the artifact's walrus location is malformed: {0}")]
    Invalid(String),
    #[error(
        "the artifact is only on Walrus ({blob_id}), and Walrus is disabled ([walrus] enabled = false)"
    )]
    WalrusDisabled { blob_id: String },
    #[error(
        "Walrus blob {blob_id} failed its SHA-256 check and there is no HTTPS location: {error}"
    )]
    WalrusIntegrity { blob_id: String, error: String },
    #[error("Walrus blob {blob_id} could not be read and there is no HTTPS location: {error}")]
    Walrus { blob_id: String, error: String },
    #[error("{url}: {error}{}", walrus.as_ref().map(|w| format!(" (after Walrus failed: {w})")).unwrap_or_default())]
    Https {
        url: String,
        error: String,
        /// Why Walrus was not used, when a walrus location was tried first.
        walrus: Option<String>,
    },
}

fn str_at<'a>(value: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(value, |v, key| v.get(*key))?.as_str()
}

fn walrus_locations(locations: &Value) -> Result<Vec<WalrusLocation>, FetchError> {
    let mut out = Vec::new();
    for location in locations.as_array().into_iter().flatten() {
        if location["kind"].as_str() != Some("walrus") {
            continue;
        }
        let blob_id = location["blob_id"].as_str().unwrap_or_default();
        let sha256 = location["sha256"].as_str().unwrap_or_default();
        out.push(WalrusLocation {
            blob_id: BlobId::parse(blob_id).map_err(|e| FetchError::Invalid(e.to_string()))?,
            sha256: ContentDigest::parse_hex(sha256)
                .map_err(|e| FetchError::Invalid(e.to_string()))?,
        });
    }
    Ok(out)
}

/// The registry's tarball URL: `<registry>/<name>/-/<unscoped name>-<version>.tgz`.
pub fn npm_tarball_url(registry: &str, name: &str, version: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    format!(
        "{}/{name}/-/{base}-{version}.tgz",
        registry.trim_end_matches('/')
    )
}

impl ArtifactSources {
    /// An npm, pypi or oci artifact's file: its `locations`, then (npm
    /// only) the registry's tarball when `npm_registry` is set.
    pub fn package(listing: &Value, npm_registry: Option<&str>) -> Result<Self, FetchError> {
        let artifact = &listing["artifact"];
        let https = match (str_at(artifact, &["type"]), npm_registry) {
            (Some("npm"), Some(registry)) => Some(npm_tarball_url(
                registry,
                str_at(artifact, &["name"]).unwrap_or_default(),
                str_at(artifact, &["version"]).unwrap_or_default(),
            )),
            _ => None,
        };
        Ok(Self {
            walrus: walrus_locations(&artifact["locations"])?,
            https,
        })
    }

    /// One platform's file of an archive artifact: its `locations`, then its `url`.
    pub fn archive_file(listing: &Value, platform: &str) -> Result<Self, FetchError> {
        let file = listing["artifact"]["files"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|f| f["platform"].as_str() == Some(platform))
            .ok_or(FetchError::NoLocation)?;
        Ok(Self {
            walrus: walrus_locations(&file["locations"])?,
            https: file["url"].as_str().map(str::to_owned),
        })
    }
}

/// What a fetch needs: the Walrus client (disabled unless `[walrus]
/// enabled = true`), an HTTPS fetcher and the npm registry, if any.
#[derive(Clone, Copy)]
pub struct ArtifactFetch<'a> {
    pub walrus: &'a Walrus,
    pub https: &'a dyn Fetcher,
    pub npm_registry: Option<&'a str>,
}

impl std::fmt::Debug for ArtifactFetch<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtifactFetch")
            .field("walrus", &self.walrus.is_enabled())
            .field("npm_registry", &self.npm_registry)
            .finish_non_exhaustive()
    }
}

impl ArtifactFetch<'_> {
    /// Reads the artifact's bytes: each walrus location (verified by its
    /// SHA-256) while Walrus is enabled, then the HTTPS location.
    pub async fn fetch(&self, sources: &ArtifactSources) -> Result<(Vec<u8>, Origin), FetchError> {
        // The last Walrus failure, and whether it was a digest mismatch.
        let mut walrus_failure: Option<(String, String, bool)> = None;
        if self.walrus.is_enabled() {
            for location in &sources.walrus {
                match self.walrus.read(&location.blob_id, &location.sha256).await {
                    Ok(bytes) => {
                        return Ok((
                            bytes.to_vec(),
                            Origin::Walrus {
                                blob_id: location.blob_id.to_string(),
                            },
                        ));
                    }
                    Err(error) => {
                        let integrity = matches!(error, WalrusError::Integrity { .. });
                        walrus_failure =
                            Some((location.blob_id.to_string(), error.to_string(), integrity));
                    }
                }
            }
        }
        let Some(url) = &sources.https else {
            return Err(match (walrus_failure, sources.walrus.first()) {
                (Some((blob_id, error, true)), _) => FetchError::WalrusIntegrity { blob_id, error },
                (Some((blob_id, error, false)), _) => FetchError::Walrus { blob_id, error },
                (None, Some(location)) => FetchError::WalrusDisabled {
                    blob_id: location.blob_id.to_string(),
                },
                (None, None) => FetchError::NoLocation,
            });
        };
        match self.https.get(url).await {
            Ok(bytes) => Ok((bytes, Origin::Https { url: url.clone() })),
            Err(error) => Err(FetchError::Https {
                url: url.clone(),
                error,
                walrus: walrus_failure.map(|(blob_id, error, _)| format!("{blob_id}: {error}")),
            }),
        }
    }
}
