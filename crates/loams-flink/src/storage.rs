//! Tenants and their layout on Loams object storage (D1973).
//!
//! One bucket holds every tenant; each tenant has the prefix
//! `flink/<tenant>/`:
//!
//! | Prefix | What |
//! |---|---|
//! | `checkpoints/` | `execution.checkpointing.dir` (retained checkpoints) |
//! | `savepoints/` | `execution.checkpointing.savepoint-dir` |
//! | `ha/` | `high-availability.storageDir` (Kubernetes HA) |
//! | `recovery/` | the controller's recovery record (compose, D1974) |
//! | `scripts/` | scripts for `deploy-script` (application mode) |

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::FlinkError;

/// A tenant: a Loams project id, `[a-z0-9-]{1,40}`, starting with a letter
/// or digit and not ending with `-` (a DNS label once prefixed).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TenantId(String);

impl TenantId {
    /// The longest tenant id.
    pub const MAX_LEN: usize = 40;

    /// Checks `value`.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] for anything but `[a-z0-9-]{1,40}` without a
    /// leading or trailing `-`.
    pub fn new(value: impl Into<String>) -> Result<Self, FlinkError> {
        let value = value.into();
        let ok = !value.is_empty()
            && value.len() <= Self::MAX_LEN
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !value.starts_with('-')
            && !value.ends_with('-');
        if ok {
            Ok(Self(value))
        } else {
            Err(FlinkError::Invalid(format!(
                "a tenant id is 1-40 of [a-z0-9-], not starting or ending with '-': {value:?}"
            )))
        }
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The tenant's cluster name, `loams-flink-<tenant>`: its Kubernetes
    /// namespace and `kubernetes.cluster-id`.
    pub fn cluster_name(&self) -> String {
        format!("loams-flink-{}", self.0)
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for TenantId {
    type Error = FlinkError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TenantId> for String {
    fn from(value: TenantId) -> Self {
        value.0
    }
}

/// Where the cluster's state lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectStore {
    /// The bucket (default `flink`).
    pub bucket: String,
    /// The S3 endpoint for RustFS/MinIO (`http://rustfs:9000`); `None` for
    /// AWS S3.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// The region.
    #[serde(default = "default_region")]
    pub region: String,
    /// Path-style requests (RustFS, MinIO).
    #[serde(default = "default_true")]
    pub path_style: bool,
    /// The prefix all tenants live under (default `flink`).
    #[serde(default = "default_root")]
    pub root: String,
}

fn default_region() -> String {
    "us-east-1".to_owned()
}

fn default_true() -> bool {
    true
}

fn default_root() -> String {
    "flink".to_owned()
}

impl Default for ObjectStore {
    fn default() -> Self {
        Self {
            bucket: "flink".to_owned(),
            endpoint: Some("http://rustfs:9000".to_owned()),
            region: default_region(),
            path_style: true,
            root: default_root(),
        }
    }
}

impl ObjectStore {
    /// Checks the bucket (S3 naming) and the root prefix.
    ///
    /// # Errors
    ///
    /// [`FlinkError::Invalid`] naming the bad field.
    pub fn check(&self) -> Result<(), FlinkError> {
        let b = &self.bucket;
        let bucket_ok = (3..=63).contains(&b.len())
            && b.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.')
            && !b.starts_with(['-', '.'])
            && !b.ends_with(['-', '.']);
        if !bucket_ok {
            return Err(FlinkError::Invalid(format!("bucket name {b:?}")));
        }
        let root_ok = !self.root.is_empty()
            && self.root.split('/').all(|seg| {
                !seg.is_empty()
                    && seg != "."
                    && seg != ".."
                    && seg
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
            });
        if !root_ok {
            return Err(FlinkError::Invalid(format!(
                "root prefix {:?}: segments of [A-Za-z0-9._-], no leading or trailing '/'",
                self.root
            )));
        }
        if let Some(endpoint) = &self.endpoint
            && !(endpoint.starts_with("http://") || endpoint.starts_with("https://"))
        {
            return Err(FlinkError::Invalid(format!("S3 endpoint {endpoint:?}")));
        }
        Ok(())
    }

    /// The tenant's paths.
    pub fn tenant(&self, tenant: &TenantId) -> TenantPaths {
        TenantPaths {
            bucket: self.bucket.clone(),
            prefix: format!("{}/{}", self.root, tenant.as_str()),
        }
    }
}

/// One tenant's prefix in the bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantPaths {
    bucket: String,
    /// `<root>/<tenant>`, no slashes at either end.
    prefix: String,
}

impl TenantPaths {
    /// The bucket.
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The key prefix inside the bucket, `<root>/<tenant>/`.
    pub fn key_prefix(&self) -> String {
        format!("{}/", self.prefix)
    }

    fn uri(&self, leaf: &str) -> String {
        format!("s3://{}/{}/{leaf}", self.bucket, self.prefix)
    }

    /// `execution.checkpointing.dir`.
    pub fn checkpoints(&self) -> String {
        self.uri("checkpoints")
    }

    /// `execution.checkpointing.savepoint-dir`.
    pub fn savepoints(&self) -> String {
        self.uri("savepoints")
    }

    /// `high-availability.storageDir`.
    pub fn ha(&self) -> String {
        self.uri("ha")
    }

    /// Where `deploy-script` scripts may come from.
    pub fn scripts(&self) -> String {
        self.uri("scripts")
    }

    /// The recovery record's key in the bucket (D1974).
    pub fn recovery_key(&self) -> String {
        format!("{}/recovery/jobs.json", self.prefix)
    }

    /// The tenant's whole prefix as a URI (`s3://<bucket>/<root>/<tenant>/`).
    pub fn root_uri(&self) -> String {
        format!("s3://{}/{}/", self.bucket, self.prefix)
    }

    /// Whether `uri` is inside this tenant's prefix (any of the `s3://`,
    /// `s3a://` and `s3p://` schemes Flink's S3 file systems register),
    /// with no `..` segment.
    pub fn contains(&self, uri: &str) -> bool {
        let Some(rest) = ["s3://", "s3a://", "s3p://"]
            .iter()
            .find_map(|scheme| uri.strip_prefix(scheme))
        else {
            return false;
        };
        let Some(key) = rest
            .strip_prefix(self.bucket.as_str())
            .and_then(|r| r.strip_prefix('/'))
        else {
            return false;
        };
        key.starts_with(&self.key_prefix()) && !key.split('/').any(|seg| seg == ".." || seg == ".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_ids_are_dns_label_safe() {
        assert!(TenantId::new("acme-prod-1").is_ok());
        let long = "a".repeat(41);
        for bad in ["", "-a", "a-", "Acme", "a_b", "a.b", long.as_str()] {
            assert!(TenantId::new(bad).is_err(), "{bad:?}");
        }
        let t: TenantId = serde_json::from_str("\"t1\"").unwrap();
        assert_eq!(t.cluster_name(), "loams-flink-t1");
        assert!(serde_json::from_str::<TenantId>("\"T1\"").is_err());
    }

    #[test]
    fn tenant_paths_are_under_the_tenant_prefix() {
        let store = ObjectStore::default();
        store.check().unwrap();
        let p = store.tenant(&TenantId::new("t1").unwrap());
        assert_eq!(p.checkpoints(), "s3://flink/flink/t1/checkpoints");
        assert_eq!(p.savepoints(), "s3://flink/flink/t1/savepoints");
        assert_eq!(p.ha(), "s3://flink/flink/t1/ha");
        assert_eq!(p.recovery_key(), "flink/t1/recovery/jobs.json");
        assert!(p.contains("s3://flink/flink/t1/scripts/a.sql"));
        assert!(p.contains("s3p://flink/flink/t1/savepoints/x"));
        assert!(!p.contains("s3://flink/flink/t10/scripts/a.sql"));
        assert!(!p.contains("s3://flink/flink/t1/../t2/a.sql"));
        assert!(!p.contains("s3://other/flink/t1/a.sql"));
        assert!(!p.contains("file:///etc/passwd"));
    }

    #[test]
    fn object_store_settings_are_checked() {
        let mut s = ObjectStore::default();
        s.bucket = "A".into();
        assert!(s.check().is_err());
        let mut s = ObjectStore::default();
        s.root = "a/../b".into();
        assert!(s.check().is_err());
        let mut s = ObjectStore::default();
        s.endpoint = Some("rustfs:9000".into());
        assert!(s.check().is_err());
    }
}
