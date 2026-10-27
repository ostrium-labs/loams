//! The Kubernetes [`ClusterRuntime`]: applies [`crate::k8s::objects`] with
//! server-side apply (field manager `loams-flink`), scales Deployments with
//! a merge patch, reads readiness from Deployment status and deletes the
//! tenant's namespace.
//!
//! The controller's ServiceAccount needs, cluster-wide: namespaces
//! (get/create/patch/delete) and, in `loams-flink-*` namespaces,
//! serviceaccounts, roles, rolebindings, configmaps, secrets, services,
//! networkpolicies and deployments (get/create/patch), see
//! `deploy/flink/k8s/controller-rbac.yaml`.

use async_trait::async_trait;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Secret, Service, ServiceAccount};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::api::rbac::v1::{Role as RbacRole, RoleBinding};
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, Patch, PatchParams};
use serde_json::Value;

use crate::config::{ClusterSpec, FlinkConfig, Platform, S3Credentials, addr};
use crate::controller::{ClusterEndpoints, ClusterRuntime, Replicas, RuntimeError};
use crate::k8s::{
    FIELD_MANAGER, Role, config_hash, objects_with_replicas, scale_patch, stamp_config_hash,
};
use crate::storage::TenantId;

/// Session clusters on Kubernetes, one namespace per tenant.
#[derive(Clone)]
pub struct KubeRuntime {
    client: kube::Client,
    /// The cluster DNS suffix (`svc.cluster.local` by default; `svc` works
    /// with the usual search path).
    dns_suffix: String,
}

impl std::fmt::Debug for KubeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KubeRuntime")
            .field("dns_suffix", &self.dns_suffix)
            .finish_non_exhaustive()
    }
}

fn api_error(what: &str, e: &kube::Error) -> RuntimeError {
    match e {
        kube::Error::Api(s) => {
            RuntimeError::Failed(format!("{what}: {} {}: {}", s.code, s.reason, s.message))
        }
        other => RuntimeError::Failed(format!("{what}: {other}")),
    }
}

fn is_not_found(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(s) if s.code == 404)
}

/// The API resource of a rendered object's kind.
fn resource_of(kind: &str) -> Option<(ApiResource, bool)> {
    // (resource, namespaced)
    Some(match kind {
        "Namespace" => (ApiResource::erase::<Namespace>(&()), false),
        "ServiceAccount" => (ApiResource::erase::<ServiceAccount>(&()), true),
        "Role" => (ApiResource::erase::<RbacRole>(&()), true),
        "RoleBinding" => (ApiResource::erase::<RoleBinding>(&()), true),
        "ConfigMap" => (ApiResource::erase::<ConfigMap>(&()), true),
        "Secret" => (ApiResource::erase::<Secret>(&()), true),
        "NetworkPolicy" => (ApiResource::erase::<NetworkPolicy>(&()), true),
        "Service" => (ApiResource::erase::<Service>(&()), true),
        "Deployment" => (ApiResource::erase::<Deployment>(&()), true),
        _ => return None,
    })
}

impl KubeRuntime {
    /// The runtime over `client`.
    pub fn new(client: kube::Client) -> Self {
        Self {
            client,
            dns_suffix: "svc.cluster.local".to_owned(),
        }
    }

    /// The runtime through the in-cluster ServiceAccount or `KUBECONFIG`.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::Failed`] when no client can be configured.
    pub async fn from_environment() -> Result<Self, RuntimeError> {
        let client = kube::Client::try_default()
            .await
            .map_err(|e| RuntimeError::Failed(format!("no Kubernetes client: {e}")))?;
        Ok(Self::new(client))
    }

    /// Another cluster DNS suffix.
    pub fn with_dns_suffix(mut self, suffix: impl Into<String>) -> Self {
        self.dns_suffix = suffix.into();
        self
    }

    fn deployments(&self, tenant: &TenantId) -> Api<Deployment> {
        Api::namespaced(self.client.clone(), &tenant.cluster_name())
    }

    async fn apply_value(&self, value: &Value) -> Result<(), RuntimeError> {
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (resource, namespaced) = resource_of(kind)
            .ok_or_else(|| RuntimeError::Failed(format!("no API resource for kind {kind:?}")))?;
        let obj: DynamicObject = serde_json::from_value(value.clone())
            .map_err(|e| RuntimeError::Failed(format!("rendered {kind}: {e}")))?;
        let name = obj
            .metadata
            .name
            .clone()
            .ok_or_else(|| RuntimeError::Failed(format!("rendered {kind} has no name")))?;
        let api: Api<DynamicObject> = if namespaced {
            let ns = obj.metadata.namespace.clone().ok_or_else(|| {
                RuntimeError::Failed(format!("rendered {kind}/{name} has no namespace"))
            })?;
            Api::namespaced_with(self.client.clone(), &ns, &resource)
        } else {
            Api::all_with(self.client.clone(), &resource)
        };
        api.patch(
            &name,
            &PatchParams::apply(FIELD_MANAGER).force(),
            &Patch::Apply(&obj),
        )
        .await
        .map_err(|e| api_error(&format!("applying {kind}/{name}"), &e))?;
        Ok(())
    }
}

#[async_trait]
impl ClusterRuntime for KubeRuntime {
    fn platform(&self) -> Platform {
        Platform::Kubernetes
    }

    async fn apply(
        &self,
        spec: &ClusterSpec,
        config: &FlinkConfig,
        creds: &S3Credentials,
        replicas: Replicas,
    ) -> Result<(), RuntimeError> {
        let mut objs = objects_with_replicas(spec, config, creds, |role| replicas.of(role))
            .map_err(|e| RuntimeError::Failed(e.to_string()))?;
        // A config change rolls the pods.
        stamp_config_hash(&mut objs, &config_hash(config));
        for obj in &objs {
            self.apply_value(obj).await?;
        }
        Ok(())
    }

    async fn scale(
        &self,
        tenant: &TenantId,
        role: Role,
        replicas: u32,
    ) -> Result<(), RuntimeError> {
        self.deployments(tenant)
            .patch(
                role.name(),
                &PatchParams::default(),
                &Patch::Merge(&scale_patch(replicas)),
            )
            .await
            .map_err(|e| api_error(&format!("scaling {}", role.name()), &e))?;
        Ok(())
    }

    async fn ready(&self, tenant: &TenantId) -> Result<Replicas, RuntimeError> {
        let api = self.deployments(tenant);
        let mut out = Replicas::default();
        for role in Role::ALL {
            let ready = match api.get_opt(role.name()).await {
                Ok(Some(d)) => d
                    .status
                    .and_then(|s| s.ready_replicas)
                    .and_then(|n| u32::try_from(n).ok())
                    .unwrap_or(0),
                Ok(None) => 0,
                Err(e) => return Err(api_error(&format!("reading {}", role.name()), &e)),
            };
            match role {
                Role::JobManager => out.jobmanager = ready,
                Role::TaskManager => out.taskmanager = ready,
                Role::SqlGateway => out.sql_gateway = ready,
            }
        }
        Ok(out)
    }

    async fn delete(&self, tenant: &TenantId) -> Result<(), RuntimeError> {
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        match namespaces
            .delete(&tenant.cluster_name(), &DeleteParams::default())
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(api_error("deleting the tenant's namespace", &e)),
        }
    }

    fn endpoints(&self, tenant: &TenantId) -> ClusterEndpoints {
        let ns = tenant.cluster_name();
        ClusterEndpoints {
            rest: format!(
                "http://{}.{ns}.{}:{}",
                addr::JOBMANAGER,
                self.dns_suffix,
                addr::REST_PORT
            ),
            sql_gateway: format!(
                "http://{}.{ns}.{}:{}",
                addr::SQL_GATEWAY,
                self.dns_suffix,
                addr::SQL_GATEWAY_PORT
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rendered_kind_has_a_resource() {
        use crate::config::{ClusterSpec, Size, render};
        let spec = ClusterSpec::new(
            TenantId::new("acme").unwrap(),
            Platform::Kubernetes,
            Size::Dev,
        );
        let creds = S3Credentials {
            access_key: "a".into(),
            secret_key: "b".into(),
        };
        let objs = crate::k8s::objects(&spec, &render(&spec).unwrap(), &creds).unwrap();
        for o in &objs {
            let kind = o["kind"].as_str().unwrap();
            let (resource, namespaced) = resource_of(kind).unwrap();
            assert_eq!(resource.kind, kind);
            assert_eq!(namespaced, kind != "Namespace");
            // Every object deserializes as a DynamicObject.
            let d: DynamicObject = serde_json::from_value(o.clone()).unwrap();
            assert!(d.metadata.name.is_some());
        }
        assert!(resource_of("Pod").is_none());
    }
}
