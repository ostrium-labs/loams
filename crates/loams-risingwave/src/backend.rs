//! The two stores a cluster lives on, as the renderer takes them: the meta
//! store, a database on Loams Postgres (D1951), and the Hummock state store,
//! a prefix of a Loams object-storage bucket (D1952). The provisioners that
//! create them are [`crate::provision`].

use std::fmt;

use rand::Rng;
use rand::distr::Alphanumeric;
use serde::{Deserialize, Serialize};

use crate::model::InstanceKey;

/// A credential. Its `Debug` never shows the value.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// A fresh random secret of `len` characters from `[A-Za-z0-9]`, safe
    /// inside a URL without encoding (RisingWave splices the meta store's
    /// password into a `postgres://` URL as is).
    pub fn generate(len: usize) -> Self {
        let value: String = rand::rng()
            .sample_iter(Alphanumeric)
            .take(len)
            .map(char::from)
            .collect();
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Only `[A-Za-z0-9]`, never empty.
    pub fn is_url_safe(&self) -> bool {
        !self.0.is_empty() && self.0.bytes().all(|b| b.is_ascii_alphanumeric())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

/// The length of every password the crate generates.
pub const PASSWORD_LEN: usize = 32;

/// The meta store: a per-instance database and its owner role on Loams
/// Postgres, through the direct (session) endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaDb {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: Secret,
    /// `RW_SQL_URL_PARAMS`, such as `sslmode=require`.
    #[serde(default)]
    pub url_params: Option<String>,
}

impl MetaDb {
    /// The database and role name of an instance's meta store, `rw_<key>`
    /// (ENG81 R2).
    pub fn name_for(key: &InstanceKey) -> String {
        format!("rw_{}", key.sql_part())
    }

    /// `RW_SQL_ENDPOINT`: `host:port`, with brackets for an IPv6 host.
    pub fn endpoint(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// The Hummock state store: a prefix of a bucket, with credentials that
/// reach only that prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectStore {
    pub bucket: String,
    /// An S3-compatible endpoint (RustFS, MinIO); `None` for AWS S3.
    #[serde(default)]
    pub endpoint: Option<String>,
    pub region: String,
    /// Path-style addressing; RustFS and MinIO need it.
    #[serde(default)]
    pub force_path_style: bool,
    /// The instance's prefix, `engines/<ns>/<id>` (its `state_prefix`
    /// without the trailing `/`, ENG81 R6).
    pub prefix: String,
    pub access_key_id: String,
    pub secret_access_key: Secret,
}

impl ObjectStore {
    /// The prefix of an instance's `state_prefix` (`engines/<ns>/<id>/`):
    /// without leading or trailing `/`. `None` for an empty or unsafe one.
    pub fn prefix_from_state(state_prefix: &str) -> Option<String> {
        let p = state_prefix.trim_matches('/');
        let ok = !p.is_empty()
            && p.split('/').all(|seg| {
                !seg.is_empty()
                    && seg != "."
                    && seg != ".."
                    && seg
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
            });
        ok.then(|| p.to_owned())
    }

    /// `--state-store`.
    pub fn state_store_url(&self) -> String {
        format!("hummock+s3://{}", self.bucket)
    }

    /// `--data-directory`: Hummock's objects.
    pub fn data_directory(&self) -> String {
        format!("{}/hummock", self.prefix)
    }

    /// `--backup-storage-url`.
    pub fn backup_storage_url(&self) -> String {
        format!("s3://{}", self.bucket)
    }

    /// `--backup-storage-directory`: meta snapshots.
    pub fn backup_directory(&self) -> String {
        format!("{}/backup", self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_and_url_safe() {
        let s = Secret::generate(PASSWORD_LEN);
        assert_eq!(s.expose().len(), PASSWORD_LEN);
        assert!(s.is_url_safe());
        assert_eq!(format!("{s:?}"), "Secret(***)");
        assert!(!Secret::new("a@b").is_url_safe());
        assert!(!Secret::new("").is_url_safe());
    }

    #[test]
    fn names_and_paths() {
        let t = InstanceKey::new("acme-1").unwrap();
        assert_eq!(MetaDb::name_for(&t), "rw_acme_1");
        assert_eq!(
            ObjectStore::prefix_from_state("engines/dev/eng_01jb/").as_deref(),
            Some("engines/dev/eng_01jb")
        );
        for bad in ["", "/", "engines//x", "engines/../x", "engines/a b"] {
            assert_eq!(ObjectStore::prefix_from_state(bad), None, "{bad:?}");
        }
        let store = ObjectStore {
            bucket: "loams-engines".into(),
            endpoint: Some("http://rustfs:9000".into()),
            region: "us-east-1".into(),
            force_path_style: true,
            prefix: "engines/dev/eng_01jb".into(),
            access_key_id: "k".into(),
            secret_access_key: Secret::new("s"),
        };
        assert_eq!(store.state_store_url(), "hummock+s3://loams-engines");
        assert_eq!(store.data_directory(), "engines/dev/eng_01jb/hummock");
        assert_eq!(store.backup_directory(), "engines/dev/eng_01jb/backup");
        let db = MetaDb {
            host: "::1".into(),
            port: 5432,
            database: "d".into(),
            username: "u".into(),
            password: Secret::new("p"),
            url_params: None,
        };
        assert_eq!(db.endpoint(), "[::1]:5432");
    }
}
