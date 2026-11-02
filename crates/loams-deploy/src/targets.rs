//! The deploy target registry, embedded from `deploy/tofu/targets.toml`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

const EMBEDDED: &str = include_str!("../../../deploy/tofu/targets.toml");

/// What kind of thing a target puts on the cloud account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetClass {
    /// k3s on one VM; the operator runs inside it.
    K3s,
    /// A managed Kubernetes service (EKS, GKE, AKS).
    ManagedK8s,
    /// An existing server reached over SSH.
    Server,
    /// One VM running the compose stack (the `*-vm` modules).
    Compose,
    /// Apps on a cloud's free tier (the `*-free` modules).
    AppFree,
    /// A laptop or edge device (EDGE1).
    Laptop,
}

/// What one node of a size offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct Size {
    /// CPU of one node, in millicores.
    pub cpu_millis: u32,
    /// Memory of one node, in MiB.
    pub memory_mib: u32,
    /// How many nodes of that shape.
    pub nodes: u32,
}

/// A CPU and memory amount, as the platform base reserves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Capacity {
    pub cpu_millis: u32,
    pub memory_mib: u32,
}

/// One `[targets.<kind>]` table.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DeployTarget {
    /// The target kind, from the table's key (for example `aws-k3s`).
    #[serde(default)]
    pub kind: String,
    /// The module under `deploy/tofu/modules/`; empty when there is none.
    pub module: String,
    /// `aws`, `gcp`, `azure`, `server` or `none`.
    pub cloud: String,
    pub class: TargetClass,
    /// Deployed by `loams deploy` with the operator (a `deploy/tofu/examples/<kind>` root).
    pub operator: bool,
    /// Its module exists in the tree.
    pub available: bool,
    pub free_tier: bool,
    pub default_region: String,
    /// `small` and `medium`; empty for targets that are not sized.
    #[serde(default)]
    pub sizes: BTreeMap<String, Size>,
    /// The platform base plus the Access database (an estimate).
    #[serde(default)]
    pub reserve: Capacity,
    /// Price table SKU of the disk; empty when none applies.
    #[serde(default)]
    pub disk_sku: String,
    /// Price table SKU of internet egress; empty when none applies.
    #[serde(default)]
    pub egress_sku: String,
    #[serde(default)]
    pub required_inputs: Vec<String>,
    /// State addresses kept (removed from state, not destroyed) on teardown.
    #[serde(default)]
    pub keep_on_destroy: Vec<String>,
    /// The CLI session the target needs: `aws`, `gcloud`, `az` or `ssh`; empty for none.
    #[serde(default)]
    pub credentials: String,
    pub needs_tailnet: bool,
}

#[derive(Deserialize)]
struct File {
    targets: BTreeMap<String, DeployTarget>,
}

/// The registry of every deploy target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployTargets {
    targets: BTreeMap<String, DeployTarget>,
}

impl DeployTargets {
    /// The registry compiled into the binary.
    pub fn embedded() -> &'static DeployTargets {
        static CELL: OnceLock<DeployTargets> = OnceLock::new();
        CELL.get_or_init(|| {
            DeployTargets::parse(EMBEDDED).expect("deploy/tofu/targets.toml is valid")
        })
    }

    /// Parses a registry; each table's `kind` is its key.
    pub fn parse(text: &str) -> Result<DeployTargets, toml::de::Error> {
        let file: File = toml::from_str(text)?;
        let mut targets = file.targets;
        for (kind, target) in &mut targets {
            target.kind.clone_from(kind);
        }
        Ok(DeployTargets { targets })
    }

    pub fn get(&self, kind: &str) -> Option<&DeployTarget> {
        self.targets.get(kind)
    }

    /// Every target, ordered by kind.
    pub fn iter(&self) -> impl Iterator<Item = &DeployTarget> {
        self.targets.values()
    }
}
