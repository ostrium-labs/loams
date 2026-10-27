//! Where an instance's credentials live between operations. The adapter is
//! stateless towards Loams (ENG81 R1: it never writes the instance store),
//! so the meta store's and the service users' passwords are kept beside
//! the runtime: a private JSON file per instance on the controller's volume
//! for compose ([`FileCredentialStore`]). The Kubernetes deploy keeps them
//! in the instance's Secret, read through the same seam (RW1).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::backend::{MetaDb, ObjectStore};
use crate::bootstrap::ServiceUsers;
use crate::model::InstanceKey;

/// Everything an instance's cluster logs in with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub meta_db: MetaDb,
    pub store: ObjectStore,
    pub users: ServiceUsers,
    /// The bootstrap ran: `root` has `users.root_password`.
    pub users_set: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("credentials: {0}")]
pub struct CredentialError(pub String);

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn load(&self, key: &InstanceKey) -> Result<Option<Credentials>, CredentialError>;
    async fn save(&self, key: &InstanceKey, creds: &Credentials) -> Result<(), CredentialError>;
    /// Idempotent.
    async fn delete(&self, key: &InstanceKey) -> Result<(), CredentialError>;
}

/// `<dir>/rw-<key>.json`, mode 0600.
#[derive(Debug, Clone)]
pub struct FileCredentialStore {
    pub dir: PathBuf,
}

impl FileCredentialStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, key: &InstanceKey) -> PathBuf {
        self.dir.join(format!("rw-{key}.json"))
    }
}

fn err(e: impl std::fmt::Display) -> CredentialError {
    CredentialError(e.to_string())
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self, key: &InstanceKey) -> Result<Option<Credentials>, CredentialError> {
        match tokio::fs::read(self.path(key)).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(err),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    async fn save(&self, key: &InstanceKey, creds: &Credentials) -> Result<(), CredentialError> {
        tokio::fs::create_dir_all(&self.dir).await.map_err(err)?;
        let text = serde_json::to_vec_pretty(creds).map_err(err)?;
        let path = self.path(key);
        let tmp = path.with_extension("tmp");
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut f = opts.open(&tmp)?;
            f.write_all(&text)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &path)
        })
        .await
        .map_err(err)?
        .map_err(err)
    }

    async fn delete(&self, key: &InstanceKey) -> Result<(), CredentialError> {
        match tokio::fs::remove_file(self.path(key)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(err(e)),
        }
    }
}

/// In memory, for tests.
#[derive(Debug, Default, Clone)]
pub struct MemoryCredentialStore {
    pub map: Arc<Mutex<HashMap<InstanceKey, Credentials>>>,
}

#[async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn load(&self, key: &InstanceKey) -> Result<Option<Credentials>, CredentialError> {
        Ok(self.map.lock().await.get(key).cloned())
    }

    async fn save(&self, key: &InstanceKey, creds: &Credentials) -> Result<(), CredentialError> {
        self.map.lock().await.insert(key.clone(), creds.clone());
        Ok(())
    }

    async fn delete(&self, key: &InstanceKey) -> Result<(), CredentialError> {
        self.map.lock().await.remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::tests::{key, meta_db, store};

    #[tokio::test]
    async fn file_store_round_trips_privately() {
        let dir = std::env::temp_dir().join(format!("loams-rw-creds-{}", std::process::id()));
        let fs = FileCredentialStore::new(&dir);
        let k = key();
        assert_eq!(fs.load(&k).await.unwrap(), None);
        let creds = Credentials {
            meta_db: meta_db(),
            store: store(),
            users: ServiceUsers::generate(),
            users_set: true,
        };
        fs.save(&k, &creds).await.unwrap();
        assert_eq!(fs.load(&k).await.unwrap(), Some(creds));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(fs.path(&k)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs.delete(&k).await.unwrap();
        fs.delete(&k).await.unwrap();
        assert_eq!(fs.load(&k).await.unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
