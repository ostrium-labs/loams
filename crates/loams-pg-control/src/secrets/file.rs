//! [`FileSecretStore`]: single-node mode's secret store (PG2 Task 6). Every
//! secret sits in one file, encrypted to one age X25519 identity: a format
//! byte and the postcard map from reference to bytes, the whole of it
//! age-encrypted. Each write re-encrypts the file and replaces it atomically
//! (a 0600 temporary file, fsync, rename, fsync of the directory).
//!
//! The key is the identity's text (`AGE-SECRET-KEY-1…`), from a [`KeySource`]:
//! a file that must be mode 0600 (it is created so on first use; a key file
//! others can read is refused, as ssh refuses one), or, with the feature
//! `keyring`, an OS keyring entry (created on first use).
//!
//! One process owns the file: calls are serialized in-process, and nothing
//! locks it against a second process (single-node mode runs one `pg-control`).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use age::secrecy::ExposeSecret;
use age::x25519::Identity;
use async_trait::async_trait;
use zeroize::{Zeroize, Zeroizing};

use super::{Secret, SecretError, SecretRef, SecretStore};

/// The format byte in front of the decrypted map.
const FILE_FORMAT: u8 = 1;

/// Where the file store's key lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// A file holding the identity, mode 0600; created on first use.
    File(PathBuf),
    /// An OS keyring entry (macOS Keychain, Windows Credential Manager, the
    /// Secret Service); created on first use.
    #[cfg(feature = "keyring")]
    Keyring { service: String, account: String },
}

/// The secrets of a single-node `pg-control` in one age-encrypted file.
#[derive(Clone)]
pub struct FileSecretStore {
    path: PathBuf,
    identity: Arc<Identity>,
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl fmt::Debug for FileSecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileSecretStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The decrypted map, wiped when dropped.
#[derive(Default)]
struct Plain(BTreeMap<String, Vec<u8>>);

impl Drop for Plain {
    fn drop(&mut self) {
        for v in self.0.values_mut() {
            v.zeroize();
        }
    }
}

fn io(what: &Path, e: &std::io::Error) -> SecretError {
    SecretError::Unavailable(format!("{}: {e}", what.display()))
}

impl FileSecretStore {
    /// The store in `path` (created on the first write), with its key from
    /// `key` (created when absent).
    ///
    /// # Errors
    ///
    /// [`SecretError::Config`] for a key file others can read or a key that
    /// does not parse; [`SecretError::Unavailable`] when the key cannot be
    /// read or created.
    pub fn open(path: impl Into<PathBuf>, key: &KeySource) -> Result<Self, SecretError> {
        let identity = match key {
            KeySource::File(p) => key_from_file(p)?,
            #[cfg(feature = "keyring")]
            KeySource::Keyring { service, account } => key_from_keyring(service, account)?,
        };
        Ok(FileSecretStore {
            path: path.into(),
            identity: Arc::new(identity),
            lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Runs `f` on the decrypted map, under the in-process lock, off the
    /// async runtime; writes the map back when `f` says it changed it.
    async fn with_map<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Plain) -> Result<(T, bool), SecretError> + Send + 'static,
    ) -> Result<T, SecretError> {
        // An owned guard, moved into the blocking task: it is released when
        // the file work ends, even if the caller's future is dropped first.
        let guard = self.lock.clone().lock_owned().await;
        let (path, identity) = (self.path.clone(), self.identity.clone());
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let mut map = load(&path, &identity)?;
            let (out, changed) = f(&mut map)?;
            if changed {
                save(&path, &identity, &map)?;
            }
            Ok(out)
        })
        .await
        .map_err(|e| SecretError::Unavailable(format!("the secret file's task: {e}")))?
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn put(&self, r: &SecretRef, s: Secret<Vec<u8>>) -> Result<(), SecretError> {
        let key = r.as_str().to_string();
        self.with_map(move |map| {
            if let Some(mut old) = map.0.insert(key, s.expose().clone()) {
                old.zeroize();
            }
            Ok(((), true))
        })
        .await
    }

    async fn get(&self, r: &SecretRef) -> Result<Secret<Vec<u8>>, SecretError> {
        let key = r.as_str().to_string();
        self.with_map(move |map| match map.0.get(&key) {
            Some(v) => Ok((Secret::new(v.clone()), false)),
            None => Err(SecretError::NotFound(key)),
        })
        .await
    }

    async fn delete(&self, r: &SecretRef) -> Result<(), SecretError> {
        let key = r.as_str().to_string();
        self.with_map(move |map| match map.0.remove(&key) {
            Some(mut old) => {
                old.zeroize();
                Ok(((), true))
            }
            None => Ok(((), false)),
        })
        .await
    }
}

/// The map in `path`; empty when the file does not exist yet.
fn load(path: &Path, identity: &Identity) -> Result<Plain, SecretError> {
    let sealed = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Plain::default()),
        Err(e) => return Err(io(path, &e)),
    };
    let plain = Zeroizing::new(age::decrypt(identity, &sealed).map_err(|e| {
        SecretError::Corrupt(format!(
            "{} does not decrypt with its key: {e}",
            path.display()
        ))
    })?);
    match plain.split_first() {
        Some((&FILE_FORMAT, body)) => postcard::from_bytes(body)
            .map(Plain)
            .map_err(|e| SecretError::Corrupt(format!("{} does not decode: {e}", path.display()))),
        _ => Err(SecretError::Corrupt(format!(
            "{} has an unknown format",
            path.display()
        ))),
    }
}

/// Encrypts `map` and replaces `path` with it atomically.
fn save(path: &Path, identity: &Identity, map: &Plain) -> Result<(), SecretError> {
    let mut plain = Zeroizing::new(vec![FILE_FORMAT]);
    let body = Zeroizing::new(
        postcard::to_stdvec(&map.0)
            .map_err(|e| SecretError::Corrupt(format!("the secrets do not encode: {e}")))?,
    );
    plain.extend_from_slice(&body);
    let sealed = age::encrypt(&identity.to_public(), &plain)
        .map_err(|e| SecretError::Unavailable(format!("encrypting the secrets: {e}")))?;
    let dir = parent(path);
    fs::create_dir_all(dir).map_err(|e| io(dir, &e))?;
    let tmp = path.with_extension("tmp");
    let mut f = private_file(&tmp, true).map_err(|e| io(&tmp, &e))?;
    f.write_all(&sealed).map_err(|e| io(&tmp, &e))?;
    f.sync_all().map_err(|e| io(&tmp, &e))?;
    drop(f);
    fs::rename(&tmp, path).map_err(|e| io(path, &e))?;
    // The rename is durable once the directory is.
    sync_dir(dir)
}

/// The directory `path` is in (`.` for a bare file name).
fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Makes the entries of `dir` durable (a no-op where directories cannot be
/// opened for that).
fn sync_dir(dir: &Path) -> Result<(), SecretError> {
    #[cfg(unix)]
    fs::File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| io(dir, &e))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Opens `path` for writing, mode 0600 on Unix; `truncate` replaces it,
/// otherwise it must not exist.
fn private_file(path: &Path, truncate: bool) -> std::io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true);
    if truncate {
        o.create(true).truncate(true);
    } else {
        o.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

fn parse_identity(text: &str, from: &str) -> Result<Identity, SecretError> {
    text.trim()
        .parse::<Identity>()
        .map_err(|e| SecretError::Config(format!("{from} does not hold an age identity: {e}")))
}

/// The identity in `path`, which must be 0600; a new one written there
/// (0600) when the file does not exist.
fn key_from_file(path: &Path) -> Result<Identity, SecretError> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let text = Zeroizing::new(text);
            check_private(path)?;
            parse_identity(&text, &path.display().to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let dir = parent(path);
            fs::create_dir_all(dir).map_err(|e| io(dir, &e))?;
            let identity = Identity::generate();
            let mut f = private_file(path, false).map_err(|e| io(path, &e))?;
            let text = identity.to_string();
            f.write_all(text.expose_secret().as_bytes())
                .and_then(|()| f.write_all(b"\n"))
                .and_then(|()| f.sync_all())
                .map_err(|e| io(path, &e))?;
            // Durable before any secret is encrypted to it: a key lost to a
            // crash would leave the secret file unreadable.
            sync_dir(dir)?;
            Ok(identity)
        }
        Err(e) => Err(io(path, &e)),
    }
}

/// A key file only its owner can read or write.
fn check_private(path: &Path) -> Result<(), SecretError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|e| io(path, &e))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(SecretError::Config(format!(
                "{} is accessible by others (mode {:o}): chmod 600 it",
                path.display(),
                mode & 0o777
            )));
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(feature = "keyring")]
fn key_from_keyring(service: &str, account: &str) -> Result<Identity, SecretError> {
    let from = format!("keyring entry {service}/{account}");
    let unavailable = |e: keyring::Error| SecretError::Unavailable(format!("{from}: {e}"));
    let entry = keyring::Entry::new(service, account).map_err(unavailable)?;
    match entry.get_password() {
        Ok(text) => parse_identity(&Zeroizing::new(text), &from),
        Err(keyring::Error::NoEntry) => {
            let identity = Identity::generate();
            entry
                .set_password(identity.to_string().expose_secret())
                .map_err(unavailable)?;
            Ok(identity)
        }
        Err(e) => Err(unavailable(e)),
    }
}
