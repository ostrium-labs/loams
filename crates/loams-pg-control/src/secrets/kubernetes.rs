//! [`KubeSecretStore`]: role secrets as Kubernetes Secrets (PG2 Task 6,
//! feature `kubernetes`). One `Opaque` Secret per reference, named by the
//! reference (which [`SecretRef`] keeps a valid object name), holding the
//! bytes under the data key `secret`, in the namespace the store was given.
//!
//! Writes are server-side apply with the field manager `loams-pg-control`,
//! so a put creates or replaces in one call and a repeated put is a no-op.
//! The ServiceAccount needs `get`, `patch` and `delete` on `secrets` in that
//! namespace (§46 §7.2), nothing cluster-wide. Encryption at rest is the
//! cluster's: an `EncryptionConfiguration` with a KMS provider for
//! `secrets`.

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret as KubeSecret;
use kube::api::{Api, DeleteParams, ObjectMeta, Patch, PatchParams};

use super::{Secret, SecretError, SecretRef, SecretStore};

/// The data key a Secret holds its bytes under.
pub const DATA_KEY: &str = "secret";

/// The server-side apply field manager.
pub const FIELD_MANAGER: &str = "loams-pg-control";

/// Role secrets in one Kubernetes namespace.
#[derive(Clone)]
pub struct KubeSecretStore {
    api: Api<KubeSecret>,
    namespace: String,
}

impl fmt::Debug for KubeSecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KubeSecretStore")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

impl KubeSecretStore {
    /// The store in `namespace`, through `client`.
    pub fn new(client: kube::Client, namespace: &str) -> Self {
        KubeSecretStore {
            api: Api::namespaced(client, namespace),
            namespace: namespace.to_string(),
        }
    }

    /// What a failed call tells: the API's status and reason, or the
    /// client's error. Neither carries the Secret's data.
    fn error(&self, op: &str, r: &SecretRef, e: &kube::Error) -> SecretError {
        let why = match e {
            kube::Error::Api(status) => format!("{} {}", status.code, status.reason),
            other => other.to_string(),
        };
        SecretError::Unavailable(format!(
            "{op} secret {}/{r}: kubernetes answered {why}",
            self.namespace
        ))
    }
}

fn not_found(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(status) if status.code == 404)
}

#[async_trait]
impl SecretStore for KubeSecretStore {
    async fn put(&self, r: &SecretRef, s: Secret<Vec<u8>>) -> Result<(), SecretError> {
        let object = KubeSecret {
            metadata: ObjectMeta {
                name: Some(r.as_str().to_string()),
                namespace: Some(self.namespace.clone()),
                labels: Some(BTreeMap::from([
                    (
                        "app.kubernetes.io/managed-by".to_string(),
                        FIELD_MANAGER.to_string(),
                    ),
                    (
                        "app.kubernetes.io/component".to_string(),
                        "postgres-role-secret".to_string(),
                    ),
                ])),
                ..ObjectMeta::default()
            },
            type_: Some("Opaque".to_string()),
            data: Some(BTreeMap::from([(
                DATA_KEY.to_string(),
                ByteString(s.expose().clone()),
            )])),
            ..KubeSecret::default()
        };
        self.api
            .patch(
                r.as_str(),
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(&object),
            )
            .await
            .map(|_| ())
            .map_err(|e| self.error("writing", r, &e))
    }

    async fn get(&self, r: &SecretRef) -> Result<Secret<Vec<u8>>, SecretError> {
        let object = match self.api.get(r.as_str()).await {
            Ok(o) => o,
            Err(e) if not_found(&e) => return Err(SecretError::NotFound(r.to_string())),
            Err(e) => return Err(self.error("reading", r, &e)),
        };
        object
            .data
            .and_then(|mut d| d.remove(DATA_KEY))
            .map(|b| Secret::new(b.0))
            .ok_or_else(|| {
                SecretError::Corrupt(format!(
                    "secret {}/{r} has no data key {DATA_KEY}",
                    self.namespace
                ))
            })
    }

    async fn delete(&self, r: &SecretRef) -> Result<(), SecretError> {
        match self.api.delete(r.as_str(), &DeleteParams::default()).await {
            Ok(_) => Ok(()),
            Err(e) if not_found(&e) => Ok(()),
            Err(e) => Err(self.error("deleting", r, &e)),
        }
    }
}
