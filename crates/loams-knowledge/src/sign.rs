//! Signing a manifest: a Sigstore bundle over its exact bytes, as the
//! marketplace index is signed (MK1 Task 0 ruling 4; ruling 1 of MS1
//! Task 7). Verification is the marketplace's
//! [`SignatureVerifier`](loams_agentd_marketplace::verify::SignatureVerifier).

use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::KnowledgeError;

/// Signs a manifest's bytes, returning a Sigstore bundle (JSON).
#[async_trait]
pub trait ManifestSigner: Send + Sync {
    /// The bundle over exactly `bytes`.
    async fn sign(&self, bytes: &[u8]) -> Result<Vec<u8>, KnowledgeError>;
}

/// Keyless signing through the `cosign` CLI (`cosign sign-blob --yes
/// --bundle`), with the ambient OIDC identity: in GitHub Actions, the
/// workflow's token (`id-token: write`). The CI pins cosign v3.0.6
/// (`nf1-pins.yml`).
#[derive(Clone, Debug)]
pub struct CosignSigner {
    cosign: PathBuf,
}

impl Default for CosignSigner {
    fn default() -> Self {
        Self {
            cosign: PathBuf::from("cosign"),
        }
    }
}

impl CosignSigner {
    /// A signer running the `cosign` binary at `path`.
    pub fn with_binary(path: impl Into<PathBuf>) -> Self {
        Self {
            cosign: path.into(),
        }
    }
}

#[async_trait]
impl ManifestSigner for CosignSigner {
    async fn sign(&self, bytes: &[u8]) -> Result<Vec<u8>, KnowledgeError> {
        let err = |e: std::io::Error| KnowledgeError::Sign(e.to_string());
        let dir = tempfile::tempdir().map_err(err)?;
        let blob = dir.path().join("manifest.json");
        let bundle = dir.path().join("manifest.json.sigstore.json");
        tokio::fs::write(&blob, bytes).await.map_err(err)?;
        let out = tokio::process::Command::new(&self.cosign)
            .arg("sign-blob")
            .arg("--yes")
            .arg("--bundle")
            .arg(&bundle)
            .arg(&blob)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| KnowledgeError::Sign(format!("running {}: {e}", self.cosign.display())))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(KnowledgeError::Sign(format!(
                "cosign sign-blob failed ({}): {}",
                out.status,
                stderr.trim()
            )));
        }
        tokio::fs::read(&bundle).await.map_err(err)
    }
}
