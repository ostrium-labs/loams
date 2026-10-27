//! Where the factory's configuration lives (§50 §12): secrets in the OS keyring
//! (service `dev.loams.agentd`, accounts `factory:<app>:<field>`), the rest in
//! `factory.json` in the daemon's data directory. Without a keyring everything is
//! kept in memory for the daemon's lifetime and nothing is written to disk.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::apps::AppId;

/// The keyring service of the daemon's secrets.
pub const KEYRING_SERVICE: &str = "dev.loams.agentd";

/// `factory:<app>:<field>`.
pub fn account(app: AppId, field: &str) -> String {
    format!("factory:{}:{field}", app.as_str())
}

/// A secret store. `get` answers `Err` when the store cannot be read.
pub trait SecretStore: Send + Sync + std::fmt::Debug {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
    /// Whether the secrets outlive the daemon.
    fn persistent(&self) -> bool;
}

/// Secrets in memory: tests, and a host without a keyring.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemorySecrets {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self
            .values
            .lock()
            .map_err(|e| e.to_string())?
            .get(account)
            .cloned())
    }

    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        self.values
            .lock()
            .map_err(|e| e.to_string())?
            .insert(account.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        self.values
            .lock()
            .map_err(|e| e.to_string())?
            .remove(account);
        Ok(())
    }

    fn persistent(&self) -> bool {
        false
    }
}

/// The OS keyring.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringSecrets;

impl KeyringSecrets {
    /// The keyring when it answers, else `None` (a headless session, no Secret
    /// Service).
    pub fn probe() -> Option<Self> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, "factory:probe").ok()?;
        match entry.get_password() {
            Ok(_) | Err(keyring::Error::NoEntry) => Some(Self),
            Err(_) => None,
        }
    }
}

impl SecretStore for KeyringSecrets {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(KEYRING_SERVICE, account)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        match keyring::Entry::new(KEYRING_SERVICE, account).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn persistent(&self) -> bool {
        true
    }
}

/// One app's non-secret configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub url: String,
    /// The non-secret fields.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// The secret fields stored in the keyring for it.
    #[serde(default)]
    pub secrets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    apps: BTreeMap<String, AppEntry>,
}

/// The non-secret part, on disk when the secrets are persistent.
#[derive(Debug)]
pub struct ConfigStore {
    path: Option<PathBuf>,
    apps: Mutex<BTreeMap<AppId, AppEntry>>,
}

impl ConfigStore {
    /// Read `path` (absent means empty); `None` keeps the configuration in memory.
    pub fn open(path: Option<PathBuf>) -> Self {
        let mut apps = BTreeMap::new();
        if let Some(p) = &path
            && let Ok(text) = std::fs::read_to_string(p)
        {
            match serde_json::from_str::<File>(&text) {
                Ok(file) => {
                    for (id, entry) in file.apps {
                        if let Some(app) = AppId::parse(&id) {
                            apps.insert(app, entry);
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "factory.json is not readable; starting empty")
                }
            }
        }
        Self {
            path,
            apps: Mutex::new(apps),
        }
    }

    pub fn get(&self, app: AppId) -> Option<AppEntry> {
        self.apps.lock().ok()?.get(&app).cloned()
    }

    /// # Errors
    ///
    /// The file could not be written.
    pub fn set(&self, app: AppId, entry: AppEntry) -> Result<(), String> {
        let mut apps = self.apps.lock().map_err(|e| e.to_string())?;
        apps.insert(app, entry);
        self.save(&apps)
    }

    /// # Errors
    ///
    /// The file could not be written.
    pub fn remove(&self, app: AppId) -> Result<(), String> {
        let mut apps = self.apps.lock().map_err(|e| e.to_string())?;
        apps.remove(&app);
        self.save(&apps)
    }

    fn save(&self, apps: &BTreeMap<AppId, AppEntry>) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let file = File {
            apps: apps
                .iter()
                .map(|(a, e)| (a.as_str().to_owned(), e.clone()))
                .collect(),
        };
        let text = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_its_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("factory.json");
        let store = ConfigStore::open(Some(path.clone()));
        store
            .set(
                AppId::Forgejo,
                AppEntry {
                    url: "http://f.test".into(),
                    fields: BTreeMap::new(),
                    secrets: vec!["token".into()],
                },
            )
            .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("http://f.test"));
        let again = ConfigStore::open(Some(path));
        assert_eq!(
            again.get(AppId::Forgejo).unwrap().secrets,
            vec!["token".to_owned()]
        );
        again.remove(AppId::Forgejo).unwrap();
        assert!(
            ConfigStore::open(Some(dir.path().join("factory.json")))
                .get(AppId::Forgejo)
                .is_none()
        );
    }

    #[test]
    fn accounts_are_factory_app_field() {
        assert_eq!(account(AppId::Forgejo, "token"), "factory:forgejo:token");
    }
}
