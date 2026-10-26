//! The signed manifest: one per (repository, commit), kept in the bucket.

use std::collections::BTreeSet;

use loams_agentd_marketplace::verify::{Identity, SignatureVerifier};
use loams_walrus::{ContentDigest, QuiltId, QuiltPatchId};
use serde::{Deserialize, Serialize};

use crate::error::KnowledgeError;

/// The manifest's `schema` value.
pub const MANIFEST_SCHEMA: &str = "loams.knowledge.manifest.v1";

/// What a knowledge quilt holds, at one commit of one public repository.
///
/// The signature covers the manifest's stored bytes exactly; nothing is
/// re-canonicalised before verification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeManifest {
    /// Always [`MANIFEST_SCHEMA`].
    pub schema: String,
    /// The repository, `owner/name` or any `/`-separated name.
    pub repo: String,
    /// The commit, 40 or 64 lowercase hex digits.
    pub commit: String,
    /// The Walrus quilt holding every entry.
    pub quilt_id: QuiltId,
    /// When the manifest was made (RFC 3339).
    pub created: String,
    /// The Walrus epoch the quilt is stored until (exclusive).
    pub end_epoch: u64,
    /// One per file, sorted by path, unique.
    pub entries: Vec<ManifestEntry>,
}

/// One file of the knowledge set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestEntry {
    /// The file's path in the repository; also its quilt identifier.
    pub path: String,
    /// Its patch in the quilt.
    pub quilt_patch_id: QuiltPatchId,
    /// The SHA-256 of its bytes: every read is checked against it.
    pub sha256: ContentDigest,
    /// Its size.
    pub bytes: u64,
}

impl KnowledgeManifest {
    /// Checks the manifest's own rules: the schema, the repository and
    /// commit forms, and sorted, unique, relative entry paths.
    pub fn validate(&self) -> Result<(), KnowledgeError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(bad(format!("schema {:?}", self.schema)));
        }
        check_repo(&self.repo)?;
        check_commit(&self.commit)?;
        let mut seen = BTreeSet::new();
        let mut prev: Option<&str> = None;
        for e in &self.entries {
            check_path(&e.path)?;
            if !seen.insert(e.path.as_str()) {
                return Err(bad(format!("duplicate entry {}", e.path)));
            }
            if prev.is_some_and(|p| p > e.path.as_str()) {
                return Err(bad("entries are not sorted by path".into()));
            }
            prev = Some(&e.path);
        }
        Ok(())
    }

    /// The manifest as stored and signed: pretty JSON with a final newline.
    pub fn to_bytes(&self) -> Result<Vec<u8>, KnowledgeError> {
        self.validate()?;
        let mut out = serde_json::to_vec_pretty(self)
            .map_err(|e| KnowledgeError::Invalid(format!("manifest JSON: {e}")))?;
        out.push(b'\n');
        Ok(out)
    }

    /// Parses and validates stored bytes. Unverified: use
    /// [`VerifiedManifest::verify`] for anything read.
    pub fn parse(bytes: &[u8]) -> Result<Self, KnowledgeError> {
        let m: Self = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    /// The entry for `path`.
    pub fn entry(&self, path: &str) -> Option<&ManifestEntry> {
        self.entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()
            .map(|i| &self.entries[i])
    }
}

/// A manifest whose Sigstore bundle verified for the repository's pinned
/// signer, and whose `repo` and `commit` are the ones asked for. The only
/// way to get one is [`VerifiedManifest::verify`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedManifest(KnowledgeManifest);

impl VerifiedManifest {
    /// Verifies `bundle` over `bytes` for `signer` first; only then parses
    /// the bytes, and checks they describe `repo` (and `commit`, when one
    /// was asked for).
    pub fn verify(
        bytes: &[u8],
        bundle: Option<&[u8]>,
        verifier: &dyn SignatureVerifier,
        signer: &Identity,
        repo: &str,
        commit: &str,
    ) -> Result<Self, KnowledgeError> {
        let sig_err = |error| KnowledgeError::Signature {
            repo: repo.to_string(),
            commit: commit.to_string(),
            error,
        };
        let bundle = bundle.ok_or_else(|| {
            sig_err(loams_agentd_marketplace::verify::SignatureError::Missing(
                "manifest.json.sigstore.json".into(),
            ))
        })?;
        verifier
            .verify_blob(bytes, bundle, signer)
            .map_err(sig_err)?;
        let m = KnowledgeManifest::parse(bytes)?;
        if m.repo != repo || m.commit != commit {
            return Err(bad(format!(
                "signed for {}@{}, found at {repo}@{commit}",
                m.repo, m.commit
            )));
        }
        Ok(Self(m))
    }

    /// The manifest.
    pub fn manifest(&self) -> &KnowledgeManifest {
        &self.0
    }
}

impl std::ops::Deref for VerifiedManifest {
    type Target = KnowledgeManifest;

    fn deref(&self) -> &KnowledgeManifest {
        &self.0
    }
}

/// Where the knowledge cache lives in the bucket:
/// `<prefix>/<repo>/<commit>/manifest.json`, its bundle beside it, and
/// `<prefix>/<repo>/latest`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    prefix: String,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            prefix: "knowledge".into(),
        }
    }
}

impl Layout {
    /// A layout under `prefix` (no leading or trailing `/`).
    pub fn new(prefix: &str) -> Result<Self, KnowledgeError> {
        let prefix = prefix.trim_matches('/');
        if prefix.is_empty() {
            return Err(KnowledgeError::Invalid("empty knowledge prefix".into()));
        }
        check_path(prefix)?;
        Ok(Self {
            prefix: prefix.to_string(),
        })
    }

    /// The manifest's key.
    pub fn manifest(&self, repo: &str, commit: &str) -> String {
        format!("{}/{repo}/{commit}/manifest.json", self.prefix)
    }

    /// The manifest's Sigstore bundle's key.
    pub fn bundle(&self, repo: &str, commit: &str) -> String {
        format!("{}.sigstore.json", self.manifest(repo, commit))
    }

    /// The key of the newest published commit.
    pub fn latest(&self, repo: &str) -> String {
        format!("{}/{repo}/latest", self.prefix)
    }
}

fn bad(msg: String) -> KnowledgeError {
    KnowledgeError::InvalidManifest(msg)
}

/// A repository name: `/`-separated segments of `[A-Za-z0-9._-]`, no `.`
/// or `..` segment.
pub(crate) fn check_repo(repo: &str) -> Result<(), KnowledgeError> {
    let ok = !repo.is_empty()
        && repo.len() <= 200
        && repo.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        });
    if ok {
        Ok(())
    } else {
        Err(KnowledgeError::Invalid(format!(
            "bad repository name {repo:?}"
        )))
    }
}

/// A commit: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
pub(crate) fn check_commit(commit: &str) -> Result<(), KnowledgeError> {
    let ok = matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if ok {
        Ok(())
    } else {
        Err(KnowledgeError::Invalid(format!(
            "bad commit {commit:?}: expected 40 or 64 lowercase hex digits"
        )))
    }
}

/// A relative `/`-separated path with no empty, `.` or `..` segment and no
/// character a quilt identifier refuses.
pub(crate) fn check_path(path: &str) -> Result<(), KnowledgeError> {
    let ok = !path.is_empty()
        && path.len() <= 1024
        && path
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..")
        && !path
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control());
    if ok {
        Ok(())
    } else {
        Err(KnowledgeError::Invalid(format!("bad path {path:?}")))
    }
}
