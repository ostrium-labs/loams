//! Verified reads: what agents get through agentd's MCP shim. There is no
//! write path here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use loams_agentd_marketplace::verify::{Identity, SignatureVerifier};
use loams_store::{Store, StoreError};
use loams_walrus::{ContentDigest, Walrus, WalrusError};
use serde::Serialize;

use crate::error::KnowledgeError;
use crate::manifest::{
    Layout, ManifestEntry, VerifiedManifest, check_commit, check_path, check_repo,
};
use crate::search::{self, SearchResults, TextIndex};

/// Who may sign each repository's manifests. A repository with no pinned
/// signer is refused.
#[derive(Clone, Debug, Default)]
pub struct TrustPolicy {
    signers: HashMap<String, Identity>,
}

impl TrustPolicy {
    /// An empty policy: every repository is refused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Pins `signer` for `repo`.
    pub fn pin(mut self, repo: &str, signer: Identity) -> Self {
        self.signers.insert(repo.to_string(), signer);
        self
    }

    /// The pinned signer of `repo`.
    pub fn signer(&self, repo: &str) -> Option<&Identity> {
        self.signers.get(repo)
    }
}

/// A verified file of the knowledge cache.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct KnowledgeFile {
    /// The repository.
    pub repo: String,
    /// The commit.
    pub commit: String,
    /// The path.
    pub path: String,
    /// The SHA-256 its bytes were checked against.
    pub sha256: ContentDigest,
    /// Its bytes.
    #[serde(skip)]
    pub data: Bytes,
}

impl KnowledgeFile {
    /// The file as text (every published entry is UTF-8).
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

/// Verified manifests by (repository, commit).
type LoadedMap = HashMap<(String, String), Arc<Loaded>>;

struct Loaded {
    manifest: VerifiedManifest,
    index: tokio::sync::OnceCell<Arc<TextIndex>>,
}

/// Reads the knowledge cache: the manifest from the bucket, verified
/// against the repository's pinned signer, then each entry from Walrus,
/// verified against the manifest's SHA-256.
#[derive(Clone)]
pub struct KnowledgeReader {
    store: Store,
    walrus: Walrus,
    verifier: Arc<dyn SignatureVerifier>,
    trust: TrustPolicy,
    layout: Layout,
    loaded: Arc<Mutex<LoadedMap>>,
}

impl std::fmt::Debug for KnowledgeReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KnowledgeReader")
            .field("trust", &self.trust)
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl KnowledgeReader {
    /// A reader over `store` and `walrus`.
    pub fn new(
        store: Store,
        walrus: Walrus,
        verifier: Arc<dyn SignatureVerifier>,
        trust: TrustPolicy,
        layout: Layout,
    ) -> Self {
        Self {
            store,
            walrus,
            verifier,
            trust,
            layout,
            loaded: Arc::default(),
        }
    }

    /// The verified manifest of `repo` at `commit`, or at the newest
    /// published commit.
    pub async fn manifest(
        &self,
        repo: &str,
        commit: Option<&str>,
    ) -> Result<VerifiedManifest, KnowledgeError> {
        Ok(self.load(repo, commit).await?.manifest.clone())
    }

    /// The file `path` of `repo` at `commit` (or the newest), verified.
    pub async fn get(
        &self,
        repo: &str,
        commit: Option<&str>,
        path: &str,
    ) -> Result<KnowledgeFile, KnowledgeError> {
        check_path(path)?;
        let loaded = self.load(repo, commit).await?;
        let m = &loaded.manifest;
        let entry = m.entry(path).ok_or_else(|| KnowledgeError::NotInManifest {
            repo: m.repo.clone(),
            commit: m.commit.clone(),
            path: path.to_string(),
        })?;
        let data = self.read_entry(entry).await?;
        Ok(KnowledgeFile {
            repo: m.repo.clone(),
            commit: m.commit.clone(),
            path: entry.path.clone(),
            sha256: entry.sha256,
            data,
        })
    }

    /// Searches the manifest's paths and its entries' text.
    pub async fn search(
        &self,
        repo: &str,
        commit: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<SearchResults, KnowledgeError> {
        let loaded = self.load(repo, commit).await?;
        let index = loaded
            .index
            .get_or_init(|| async { Arc::new(self.build_index(&loaded.manifest).await) })
            .await
            .clone();
        Ok(search::search(&loaded.manifest, &index, query, limit))
    }

    /// One entry's bytes, read by patch id with the manifest's SHA-256
    /// (`loams-walrus` checks it, and serves its verified cache), and
    /// checked again here before use.
    pub(crate) async fn read_entry(&self, entry: &ManifestEntry) -> Result<Bytes, KnowledgeError> {
        let digest_err =
            |expected: &ContentDigest, actual: &ContentDigest| KnowledgeError::Digest {
                path: entry.path.clone(),
                expected: expected.to_hex(),
                actual: actual.to_hex(),
            };
        let data = self
            .walrus
            .read_patch(&entry.quilt_patch_id, &entry.sha256)
            .await
            .map_err(|e| match e {
                WalrusError::Integrity {
                    expected, actual, ..
                } => digest_err(&expected, &actual),
                other => other.into(),
            })?;
        let actual = ContentDigest::of(&data);
        if actual != entry.sha256 || data.len() as u64 != entry.bytes {
            return Err(digest_err(&entry.sha256, &actual));
        }
        Ok(data)
    }

    async fn build_index(&self, m: &VerifiedManifest) -> TextIndex {
        use futures::StreamExt as _;
        // Owned entries and a cloned reader: a stream of futures borrowing
        // `self` is not provably `Send` to rustc inside `tokio::spawn`.
        let reads = futures::stream::iter(m.entries.clone())
            .map(|e| {
                let me = self.clone();
                async move {
                    let read = me.read_entry(&e).await;
                    (e.path, read)
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        let mut index = TextIndex::default();
        for (path, read) in reads {
            match read {
                Ok(data) => index.add(path, String::from_utf8_lossy(&data).into_owned()),
                Err(e) => {
                    tracing::warn!(%path, error = %e, "knowledge entry left out of the search index");
                    index.skip();
                }
            }
        }
        index
    }

    async fn load(&self, repo: &str, commit: Option<&str>) -> Result<Arc<Loaded>, KnowledgeError> {
        check_repo(repo)?;
        let signer = self
            .trust
            .signer(repo)
            .ok_or_else(|| KnowledgeError::UnknownRepo(repo.to_string()))?
            .clone();
        let commit = match commit {
            Some(c) => c.to_string(),
            None => self.latest(repo).await?,
        };
        check_commit(&commit)?;
        let key = (repo.to_string(), commit.clone());
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let no_manifest = || KnowledgeError::NoManifest {
            repo: repo.to_string(),
            commit: commit.clone(),
        };
        let bytes = match self.store.get(&self.layout.manifest(repo, &commit)).await {
            Ok((b, _)) => b,
            Err(StoreError::NotFound { .. }) => return Err(no_manifest()),
            Err(e) => return Err(e.into()),
        };
        let bundle = match self.store.get(&self.layout.bundle(repo, &commit)).await {
            Ok((b, _)) => Some(b),
            Err(StoreError::NotFound { .. }) => None,
            Err(e) => return Err(e.into()),
        };
        let manifest = VerifiedManifest::verify(
            &bytes,
            bundle.as_deref(),
            self.verifier.as_ref(),
            &signer,
            repo,
            &commit,
        )?;
        let loaded = Arc::new(Loaded {
            manifest,
            index: tokio::sync::OnceCell::new(),
        });
        let mut map = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        Ok(map.entry(key).or_insert(loaded).clone())
    }

    fn cached(&self, key: &(String, String)) -> Option<Arc<Loaded>> {
        let map = self.loaded.lock().unwrap_or_else(|p| p.into_inner());
        map.get(key).cloned()
    }

    async fn latest(&self, repo: &str) -> Result<String, KnowledgeError> {
        match self.store.get(&self.layout.latest(repo)).await {
            Ok((b, _)) => Ok(String::from_utf8_lossy(&b).trim().to_string()),
            Err(StoreError::NotFound { .. }) => Err(KnowledgeError::NoManifest {
                repo: repo.to_string(),
                commit: "latest".into(),
            }),
            Err(e) => Err(e.into()),
        }
    }
}
