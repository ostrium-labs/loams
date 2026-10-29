//! Where clusters run: the [`Runtime`] seam, with a compose runtime
//! ([`compose`]: one compose project per instance on the Loams stack's
//! network) and a Helm runtime ([`helm`]: one release of
//! `deploy/helm/loams-risingwave` per instance).

pub mod compose;
pub mod helm;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::model::InstanceKey;
use crate::render::RenderedCluster;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("io: {0}")]
    Io(String),
    #[error("`{command}` failed ({status}): {stderr}")]
    Command {
        command: String,
        status: String,
        stderr: String,
    },
    #[error("render: {0}")]
    Render(String),
}

/// Makes a rendered cluster run, or go away.
#[async_trait]
pub trait Runtime: Send + Sync {
    /// Creates or updates every workload to match `cluster` (replicas,
    /// images, environment, config). Returns when the change is accepted,
    /// not when the cluster is ready; the adapter probes for that.
    async fn apply(&self, cluster: &RenderedCluster) -> Result<(), RuntimeError>;

    /// Removes the instance's workloads, config and Secret. Idempotent.
    async fn remove(&self, key: &InstanceKey) -> Result<(), RuntimeError>;
}

/// Runs a command, failing with its stderr.
pub(crate) async fn run(program: &str, args: &[String]) -> Result<String, RuntimeError> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| RuntimeError::Io(format!("{program}: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(RuntimeError::Command {
            command: format!("{program} {}", args.join(" ")),
            status: out.status.to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
        })
    }
}

/// Records what it was asked to do, for tests.
#[derive(Debug, Default, Clone)]
pub struct MemoryRuntime {
    pub applied: Arc<Mutex<Vec<RenderedCluster>>>,
    pub removed: Arc<Mutex<Vec<InstanceKey>>>,
    /// Fail the next `apply` calls while set.
    pub fail_apply: Arc<Mutex<bool>>,
}

#[async_trait]
impl Runtime for MemoryRuntime {
    async fn apply(&self, cluster: &RenderedCluster) -> Result<(), RuntimeError> {
        if *self.fail_apply.lock().await {
            return Err(RuntimeError::Io("injected apply failure".into()));
        }
        self.applied.lock().await.push(cluster.clone());
        Ok(())
    }

    async fn remove(&self, key: &InstanceKey) -> Result<(), RuntimeError> {
        self.removed.lock().await.push(key.clone());
        Ok(())
    }
}
