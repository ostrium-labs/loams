//! Custom resource types of the Loams operator v2 (design 86, plan OP2-A).
//!
//! Five kinds in API group `loams.dev`, version `v1alpha1`, all namespaced
//! with a `status` subresource: [`LoamsCluster`], [`LoamsInstall`],
//! [`StorageBinding`], [`LoamsDevice`] and [`LoamsFleet`]. Specs carry references to Secrets, never secret
//! values. This API is a preview and may change.
//!
//! [`all_crds`] returns the generated CustomResourceDefinitions. The
//! `crdgen` binary writes them to `deploy/helm/loams-operator/crds/`, and
//! `crdgen --check` fails when those files drift.

pub mod cluster;
pub mod common;
pub mod device;
pub mod install;
pub mod storage;

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::CustomResourceExt;

pub use cluster::*;
pub use common::*;
pub use device::*;
pub use install::*;
pub use storage::*;

/// The API group of every Loams operator kind.
pub const GROUP: &str = "loams.dev";
/// The API version of every Loams operator kind.
pub const VERSION: &str = "v1alpha1";

/// Every CustomResourceDefinition of this crate, in a fixed order.
pub fn all_crds() -> Vec<CustomResourceDefinition> {
    vec![
        LoamsCluster::crd(),
        LoamsInstall::crd(),
        StorageBinding::crd(),
        LoamsDevice::crd(),
        LoamsFleet::crd(),
    ]
}
