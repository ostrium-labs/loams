//! The reader's config file: what agentd's MCP shim loads from
//! `LOAMS_AGENTD_KNOWLEDGE_CONFIG`.

use std::path::Path;
use std::sync::Arc;

use loams_agentd_marketplace::verify::{
    GITHUB_ACTIONS_ISSUER, Identity, SignatureVerifier, SigstoreVerifier,
};
use loams_store::Store;
use loams_walrus::{Walrus, WalrusConfig};
use serde::Deserialize;

use crate::error::KnowledgeError;
use crate::manifest::{Layout, check_repo};
use crate::read::{KnowledgeReader, TrustPolicy};

/// The whole file:
///
/// ```toml
/// [walrus]
/// enabled = true
/// network = "testnet"
///
/// [knowledge]
/// bucket = "s3://loams-bucket/ns/default"
/// prefix = "knowledge"
///
/// [[knowledge.signers]]
/// repo = "ostrium-labs/loams"
/// identity = "https://github.com/ostrium-labs/loams/.github/workflows/knowledge.yml@refs/heads/dev"
/// issuer = "https://token.actions.githubusercontent.com"   # the default
/// ```
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderConfig {
    /// The Walrus fast path; it must be enabled for the reader to work.
    #[serde(default)]
    pub walrus: WalrusConfig,
    /// Where manifests live and who signs them.
    pub knowledge: KnowledgeConfig,
}

/// `[knowledge]`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeConfig {
    /// The bucket's URL, as `loams-store`'s `Store::from_url` takes it.
    pub bucket: String,
    /// The key prefix of the knowledge cache in the bucket.
    #[serde(default = "default_prefix")]
    pub prefix: String,
    /// The pinned signer of each repository readers may read.
    #[serde(default)]
    pub signers: Vec<SignerRule>,
}

/// One repository's pinned signer.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignerRule {
    /// The repository.
    pub repo: String,
    /// The certificate identity (for GitHub Actions, the workflow URL with
    /// its ref).
    pub identity: String,
    /// The OIDC issuer.
    #[serde(default = "default_issuer")]
    pub issuer: String,
}

fn default_prefix() -> String {
    "knowledge".into()
}

fn default_issuer() -> String {
    GITHUB_ACTIONS_ISSUER.into()
}

impl ReaderConfig {
    /// Parses the file's text.
    pub fn parse(text: &str) -> Result<Self, KnowledgeError> {
        toml::from_str(text).map_err(|e| KnowledgeError::Invalid(format!("knowledge config: {e}")))
    }

    /// Reads and parses the file at `path`.
    pub async fn load(path: &Path) -> Result<Self, KnowledgeError> {
        let text = tokio::fs::read_to_string(path).await.map_err(|e| {
            KnowledgeError::Invalid(format!("knowledge config {}: {e}", path.display()))
        })?;
        Self::parse(&text)
    }

    /// The trust policy the signers describe.
    pub fn trust(&self) -> Result<TrustPolicy, KnowledgeError> {
        let mut trust = TrustPolicy::new();
        for s in &self.knowledge.signers {
            check_repo(&s.repo)?;
            trust = trust.pin(
                &s.repo,
                Identity {
                    identity: s.identity.clone(),
                    issuer: s.issuer.clone(),
                },
            );
        }
        Ok(trust)
    }

    /// A reader verifying with Sigstore's public-good root (offline).
    pub async fn reader(&self) -> Result<KnowledgeReader, KnowledgeError> {
        let verifier = SigstoreVerifier::production().map_err(|error| {
            KnowledgeError::Invalid(format!("the Sigstore trusted root: {error}"))
        })?;
        self.reader_with(Arc::new(verifier)).await
    }

    /// A reader verifying with `verifier`. Refused while `[walrus]` is
    /// disabled: the cache has nothing to read from.
    pub async fn reader_with(
        &self,
        verifier: Arc<dyn SignatureVerifier>,
    ) -> Result<KnowledgeReader, KnowledgeError> {
        if !self.walrus.enabled {
            return Err(KnowledgeError::Invalid(
                "the knowledge cache needs [walrus] enabled = true".into(),
            ));
        }
        let trust = self.trust()?;
        let layout = Layout::new(&self.knowledge.prefix)?;
        let store = Store::from_url(&self.knowledge.bucket, Vec::<(String, String)>::new())?;
        let walrus = Walrus::new(&self.walrus).await?;
        Ok(KnowledgeReader::new(store, walrus, verifier, trust, layout))
    }
}
