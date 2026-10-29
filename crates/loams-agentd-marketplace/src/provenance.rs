//! Artifact integrity and provenance before an install (design §73 §6,
//! D1459, D1473; MK1 Task 6, Task 0 ruling 5). The bytes must hash to the
//! listing's integrity, and a SLSA v1 provenance bundle must cover those
//! bytes, name the listing's source repository, and verify as signed by
//! that repository's own workflow (the identity is rebuilt from the signed
//! statement, so a statement naming the source but signed elsewhere fails).

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256, Sha512};

use crate::verify::{GITHUB_ACTIONS_ISSUER, Identity, SignatureError, SignatureVerifier};

pub const SLSA_V1: &str = "https://slsa.dev/provenance/v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProvenanceError {
    #[error("integrity mismatch: the bytes hash to {actual}, the listing says {expected}")]
    IntegrityMismatch { expected: String, actual: String },
    #[error("no SLSA v1 provenance covers these bytes")]
    Missing,
    #[error("built from {actual}, but the listing's source is {expected}")]
    RepoMismatch { expected: String, actual: String },
    #[error("the provenance does not verify: {0}")]
    Signature(SignatureError),
    #[error("the provenance is malformed: {0}")]
    Malformed(String),
}

/// What a verified provenance says about the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub repository: String,
    pub workflow: String,
    pub git_ref: String,
    /// The signing identity: `<repository>/<workflow>@<ref>`.
    pub identity: String,
}

#[derive(Deserialize)]
struct Bundle {
    #[serde(rename = "dsseEnvelope")]
    dsse: Option<Envelope>,
}

#[derive(Deserialize)]
struct Envelope {
    payload: String,
}

#[derive(Deserialize)]
struct Statement {
    #[serde(default)]
    subject: Vec<Subject>,
    #[serde(rename = "predicateType", default)]
    predicate_type: String,
    #[serde(default)]
    predicate: serde_json::Value,
}

#[derive(Deserialize)]
struct Subject {
    #[serde(default)]
    digest: std::collections::BTreeMap<String, String>,
}

/// `https://github.com/O/R.git`, `git+https://…/` → `https://github.com/o/r`.
pub fn normalise_repository(url: &str) -> String {
    let mut out = url.trim().trim_start_matches("git+").to_owned();
    if let Some(stripped) = out.strip_suffix('/') {
        out = stripped.to_owned();
    }
    if let Some(stripped) = out.strip_suffix(".git") {
        out = stripped.to_owned();
    }
    out.to_ascii_lowercase()
}

/// `sha512-<base64>` of `bytes`, npm's integrity form.
pub fn npm_integrity(bytes: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
    )
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The digest algorithm a subject must carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Sha512,
    Sha256,
}

fn statement_of(bundle: &[u8]) -> Result<Statement, ProvenanceError> {
    let bundle: Bundle =
        serde_json::from_slice(bundle).map_err(|e| ProvenanceError::Malformed(e.to_string()))?;
    let envelope = bundle
        .dsse
        .ok_or_else(|| ProvenanceError::Malformed("no DSSE envelope".into()))?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(envelope.payload.as_bytes())
        .map_err(|e| ProvenanceError::Malformed(e.to_string()))?;
    serde_json::from_slice(&payload).map_err(|e| ProvenanceError::Malformed(e.to_string()))
}

/// Checks one or more candidate bundles over `bytes`; the first that passes
/// every check wins. Errors name the most specific failure seen.
pub fn verify(
    bytes: &[u8],
    algorithm: Algorithm,
    bundles: &[Vec<u8>],
    source_repository: &str,
    verifier: &dyn SignatureVerifier,
) -> Result<Built, ProvenanceError> {
    let digest = match algorithm {
        Algorithm::Sha512 => hex::encode(Sha512::digest(bytes)),
        Algorithm::Sha256 => sha256_hex(bytes),
    };
    let key = match algorithm {
        Algorithm::Sha512 => "sha512",
        Algorithm::Sha256 => "sha256",
    };
    let expected = normalise_repository(source_repository);
    let mut worst = ProvenanceError::Missing;
    for bundle in bundles {
        let statement = match statement_of(bundle) {
            Ok(s) => s,
            Err(e) => {
                worst = e;
                continue;
            }
        };
        if statement.predicate_type != SLSA_V1
            || !statement.subject.iter().any(|s| {
                s.digest
                    .get(key)
                    .is_some_and(|d| d.eq_ignore_ascii_case(&digest))
            })
        {
            continue;
        }
        let workflow = &statement.predicate["buildDefinition"]["externalParameters"]["workflow"];
        let repository = workflow["repository"].as_str().unwrap_or_default();
        if normalise_repository(repository) != expected {
            worst = ProvenanceError::RepoMismatch {
                expected: source_repository.into(),
                actual: repository.into(),
            };
            continue;
        }
        let (Some(path), Some(git_ref)) = (workflow["path"].as_str(), workflow["ref"].as_str())
        else {
            worst =
                ProvenanceError::Malformed("the provenance names no workflow path or ref".into());
            continue;
        };
        let identity = format!("{}/{path}@{git_ref}", repository.trim_end_matches('/'));
        let signer = Identity {
            identity: identity.clone(),
            issuer: GITHUB_ACTIONS_ISSUER.into(),
        };
        match verifier.verify_blob(bytes, bundle, &signer) {
            Ok(()) => {
                return Ok(Built {
                    repository: repository.into(),
                    workflow: path.into(),
                    git_ref: git_ref.into(),
                    identity,
                });
            }
            Err(error) => worst = ProvenanceError::Signature(error),
        }
    }
    Err(worst)
}

/// npm: the tarball must match the listing's integrity, then its provenance.
pub fn verify_npm(
    tarball: &[u8],
    integrity: &str,
    bundles: &[Vec<u8>],
    source_repository: &str,
    verifier: &dyn SignatureVerifier,
) -> Result<Built, ProvenanceError> {
    let actual = npm_integrity(tarball);
    if actual != integrity {
        return Err(ProvenanceError::IntegrityMismatch {
            expected: integrity.into(),
            actual,
        });
    }
    verify(
        tarball,
        Algorithm::Sha512,
        bundles,
        source_repository,
        verifier,
    )
}

/// An archive: the file must match its SHA-256, then its provenance.
pub fn verify_archive(
    file: &[u8],
    sha256: &str,
    bundles: &[Vec<u8>],
    source_repository: &str,
    verifier: &dyn SignatureVerifier,
) -> Result<Built, ProvenanceError> {
    let actual = sha256_hex(file);
    if !actual.eq_ignore_ascii_case(sha256) {
        return Err(ProvenanceError::IntegrityMismatch {
            expected: sha256.into(),
            actual,
        });
    }
    verify(
        file,
        Algorithm::Sha256,
        bundles,
        source_repository,
        verifier,
    )
}
