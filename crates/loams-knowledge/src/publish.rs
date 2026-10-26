//! Publishing a public repository's knowledge set at one commit.

use std::sync::Arc;

use bytes::Bytes;
use loams_store::Store;
use loams_walrus::{ContentDigest, StoreOpts, Walrus};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::KnowledgeError;
use crate::manifest::{
    KnowledgeManifest, Layout, MANIFEST_SCHEMA, ManifestEntry, check_commit, check_repo,
};
use crate::select::{MARKER_PATH, marker, select};
use crate::sign::ManifestSigner;
use crate::source::KnowledgeSource;

/// What the forge says about a repository's visibility. Only `Public`
/// publishes; `Unknown` is refused like `Private`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Public on its forge.
    Public,
    /// Private or internal.
    Private,
    /// Not known: refused.
    Unknown,
}

/// One publish.
#[derive(Clone, Debug)]
pub struct PublishRequest {
    /// The repository's name, as readers ask for it.
    pub repo: String,
    /// The full commit id.
    pub commit: String,
    /// What the forge says.
    pub visibility: Visibility,
    /// How long, and whether permanently, to store the quilt.
    pub store: StoreOpts,
}

/// What a publish wrote.
#[derive(Clone, Debug)]
pub struct Published {
    /// The manifest.
    pub manifest: KnowledgeManifest,
    /// The manifest's bytes, as signed.
    pub manifest_bytes: Bytes,
    /// Its Sigstore bundle.
    pub bundle: Bytes,
    /// The manifest's key in the bucket.
    pub manifest_key: String,
    /// The bundle's key in the bucket.
    pub bundle_key: String,
}

/// Publishes knowledge sets: Walrus for the bytes, the bucket for the
/// signed manifest.
#[derive(Clone)]
pub struct Publisher {
    store: Store,
    walrus: Walrus,
    signer: Arc<dyn ManifestSigner>,
    layout: Layout,
}

impl std::fmt::Debug for Publisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Publisher")
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}

impl Publisher {
    /// A publisher writing manifests to `store` under `layout`.
    pub fn new(
        store: Store,
        walrus: Walrus,
        signer: Arc<dyn ManifestSigner>,
        layout: Layout,
    ) -> Self {
        Self {
            store,
            walrus,
            signer,
            layout,
        }
    }

    /// Publishes `source` at `req.commit`. Refused with
    /// [`KnowledgeError::NotPublic`] before any request unless the forge
    /// says public and `.loams/knowledge.toml` at the commit says
    /// `public = true`.
    pub async fn publish(
        &self,
        source: &dyn KnowledgeSource,
        req: &PublishRequest,
    ) -> Result<Published, KnowledgeError> {
        check_repo(&req.repo)?;
        check_commit(&req.commit)?;
        let not_public = |reason: &str| KnowledgeError::NotPublic {
            repo: req.repo.clone(),
            reason: reason.to_string(),
        };
        match req.visibility {
            Visibility::Public => {}
            Visibility::Private => return Err(not_public("the forge reports it private")),
            Visibility::Unknown => {
                return Err(not_public("its visibility on the forge is unknown"));
            }
        }
        let settings = match marker(source, &req.commit).await? {
            Some(s) if s.public => s,
            Some(_) => {
                return Err(not_public(&format!(
                    "{MARKER_PATH} does not say public = true"
                )));
            }
            None => return Err(not_public(&format!("it has no {MARKER_PATH}"))),
        };

        let files = select(source, &req.commit, &settings).await?;
        let total: u64 = files.iter().map(|(_, d)| d.len() as u64).sum();
        let digests: Vec<(String, ContentDigest, u64)> = files
            .iter()
            .map(|(p, d)| (p.clone(), ContentDigest::of(d), d.len() as u64))
            .collect();
        let stored = self
            .walrus
            .store_quilt(files, req.store)
            .await
            .map_err(|e| match e {
                loams_walrus::WalrusError::TooLarge { max, .. } => {
                    KnowledgeError::TooLarge { size: total, max }
                }
                other => other.into(),
            })?;

        let mut entries = Vec::with_capacity(digests.len());
        for (path, sha256, bytes) in digests {
            let patch = stored
                .patches
                .iter()
                .find(|(id, _)| *id == path)
                .map(|(_, p)| p.clone())
                .ok_or_else(|| {
                    KnowledgeError::Invalid(format!("the quilt has no patch for {path}"))
                })?;
            entries.push(ManifestEntry {
                path,
                quilt_patch_id: patch,
                sha256,
                bytes,
            });
        }
        let created = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|e| KnowledgeError::Invalid(format!("timestamp: {e}")))?;
        let manifest = KnowledgeManifest {
            schema: MANIFEST_SCHEMA.into(),
            repo: req.repo.clone(),
            commit: req.commit.clone(),
            quilt_id: stored.blob.blob_id.clone(),
            created,
            end_epoch: stored.blob.end_epoch,
            entries,
        };
        let manifest_bytes = Bytes::from(manifest.to_bytes()?);
        let bundle = Bytes::from(self.signer.sign(&manifest_bytes).await?);

        let manifest_key = self.layout.manifest(&req.repo, &req.commit);
        let bundle_key = self.layout.bundle(&req.repo, &req.commit);
        // The bundle first: a reader that sees the new manifest with the
        // old bundle refuses it, which is safe.
        self.store.put(&bundle_key, bundle.clone()).await?;
        self.store
            .put(&manifest_key, manifest_bytes.clone())
            .await?;
        self.store
            .put(
                &self.layout.latest(&req.repo),
                Bytes::from(format!("{}\n", req.commit)),
            )
            .await?;
        tracing::info!(
            repo = %req.repo,
            commit = %req.commit,
            quilt = %manifest.quilt_id,
            entries = manifest.entries.len(),
            "published the knowledge cache"
        );
        Ok(Published {
            manifest,
            manifest_bytes,
            bundle,
            manifest_key,
            bundle_key,
        })
    }
}
