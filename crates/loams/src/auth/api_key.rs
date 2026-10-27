//! API keys (API2 Task 6): `lk_<26-char ULID>_<32 base62>`, stored only as
//! argon2id hashes: in `LOAMS_API_KEYS_FILE` (the operator's bootstrap keys,
//! this module's [`ApiKeyAuthenticator`]) and in the metastore (the keys
//! `KeyService/CreateKey` makes, [`super::directory`]).
//!
//! The file has one key per line, `<id> <argon2id PHC string> [label]`, where
//! `<id>` is the key's ULID (not secret: it only picks the hash to check).
//! Blank lines and `#` comments are ignored. `loams auth new-key` prints a
//! new key once and the line to append.
//!
//! Argon2id is deliberately slow, so a key that verified once is remembered
//! as its SHA-256 and later requests compare that in constant time; a
//! changed file (checked at most every [`RELOAD_EVERY`]) is re-read and
//! forgets every remembered key, so a deleted line revokes its key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;

use super::{AuthError, Authenticator, Credential, Principal, PrincipalKind, reason};

/// The key prefix.
pub const PREFIX: &str = "lk_";
const ID_LEN: usize = 26;
const SECRET_LEN: usize = 32;
/// `lk_` + id + `_` + secret.
pub const KEY_LEN: usize = PREFIX.len() + ID_LEN + 1 + SECRET_LEN;
/// How often the key file's metadata is checked for changes.
pub const RELOAD_EVERY: Duration = Duration::from_secs(5);
/// At most this many argon2id verifications run at once (each takes ~19 MiB).
const MAX_VERIFICATIONS: usize = 4;
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Whether `value` looks like an API key (the shape only).
pub fn looks_like_key(value: &str) -> bool {
    parse_key(value).is_some()
}

/// The key's id, when `key` has the right shape.
fn parse_key(key: &str) -> Option<&str> {
    let rest = key.strip_prefix(PREFIX)?;
    if key.len() != KEY_LEN || !key.is_ascii() {
        return None;
    }
    let (id, secret) = rest.split_at(ID_LEN);
    let secret = secret.strip_prefix('_')?;
    let ulid_ok = id
        .bytes()
        .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase());
    let secret_ok = secret.bytes().all(|b| b.is_ascii_alphanumeric());
    (ulid_ok && secret_ok && secret.len() == SECRET_LEN).then_some(id)
}

/// `len` uniformly random base62 characters from the OS.
pub(crate) fn random_base62(len: usize) -> String {
    // 62 * 4 = 248: rejecting bytes >= 248 keeps every character uniform.
    let mut out = String::with_capacity(len);
    let mut pool: Vec<u8> = Vec::new();
    while out.len() < len {
        if pool.is_empty() {
            pool = vec![0u8; len.max(16)];
            getrandom::fill(&mut pool).unwrap_or_else(|e| panic!("OS randomness unavailable: {e}"));
        }
        let byte = pool.pop().unwrap_or_default();
        if byte < 248 {
            out.push(BASE62[usize::from(byte % 62)] as char);
        }
    }
    out
}

/// A new key and its id: `(lk_<id>_<secret>, id)`. Only the key's
/// [`hash_key`] is ever stored.
pub fn generate() -> (String, String) {
    let id = ulid::Ulid::generate().to_string();
    let key = format!("{PREFIX}{id}_{}", random_base62(SECRET_LEN));
    (key, id)
}

/// The id of `key`, when it has an API key's shape.
pub fn key_id(key: &str) -> Option<&str> {
    parse_key(key)
}

/// At most [`MAX_VERIFICATIONS`] argon2id checks run at once, process-wide
/// (each takes ~19 MiB): the key file's keys, the metastore's keys and the
/// console's passwords.
static VERIFICATIONS: Semaphore = Semaphore::const_new(MAX_VERIFICATIONS);

/// Whether `secret` matches the argon2id PHC string `hash`, checked off the
/// async runtime.
pub(crate) async fn verify_hash(secret: &str, hash: &str) -> bool {
    let Ok(_permit) = VERIFICATIONS.acquire().await else {
        return false;
    };
    let hash = hash.to_owned();
    let candidate = secret.to_owned();
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|h| {
                Argon2::default()
                    .verify_password(candidate.as_bytes(), &h)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// A new key: `(key, file line)`. The key is shown once; only the line
/// (with its argon2id hash) is stored.
pub fn new_key(label: &str) -> (String, String) {
    let (key, id) = generate();
    let label = label.split_whitespace().collect::<Vec<_>>().join("-");
    let line = format!("{id} {} {label}", hash_key(&key))
        .trim_end()
        .to_owned();
    (key, line)
}

/// The argon2id PHC hash of `key` with a random salt.
pub fn hash_key(key: &str) -> String {
    let mut salt = [0u8; 16];
    getrandom::fill(&mut salt).unwrap_or_else(|e| panic!("OS randomness unavailable: {e}"));
    let hash: PasswordHash = Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
        .hash_password_with_salt(key.as_bytes(), &salt)
        .unwrap_or_else(|e| panic!("argon2: {e}"));
    hash.to_string()
}

/// One line of the key file.
#[derive(Clone, Debug)]
struct Entry {
    hash: String,
    label: String,
}

/// Parses the key file; the error names the line.
fn parse_file(path: &Path, text: &str) -> Result<HashMap<String, Entry>, String> {
    let mut keys = HashMap::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = || format!("{}:{}", path.display(), n + 1);
        let mut fields = line.split_whitespace();
        let (Some(id), Some(hash)) = (fields.next(), fields.next()) else {
            return Err(format!("{}: expected `<id> <argon2id hash> [label]`", at()));
        };
        if id.len() != ID_LEN || id.parse::<ulid::Ulid>().is_err() {
            return Err(format!("{}: the key id is not a ULID", at()));
        }
        if !hash.starts_with("$argon2id$") || PasswordHash::new(hash).is_err() {
            return Err(format!("{}: the hash is not an argon2id PHC string", at()));
        }
        let label = fields.collect::<Vec<_>>().join(" ");
        let entry = Entry {
            hash: hash.to_owned(),
            label: if label.is_empty() {
                id.to_owned()
            } else {
                label
            },
        };
        if keys.insert(id.to_owned(), entry).is_some() {
            return Err(format!("{}: key id {id} appears twice", at()));
        }
    }
    Ok(keys)
}

/// The file's identity for change detection.
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Why the key file at `path` could not be read. Permission denied also says
/// whom the engine runs as and whom the file belongs to, as the engine sees
/// them: `loams auth new-key --append` creates the file as its writer, mode
/// 0600, which an engine running as another user (the release image's uid
/// 10001) cannot read (#881).
pub(crate) fn read_error(path: &Path, err: &std::io::Error) -> String {
    let mut message = format!("LOAMS_API_KEYS_FILE {}: {err}", path.display());
    if err.kind() != std::io::ErrorKind::PermissionDenied {
        return message;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if let Some((uid, gid)) = process_ids() {
            message.push_str(&format!("; the engine runs as uid {uid} gid {gid}"));
        }
        // The file, or else the directory the engine cannot search.
        let owner = |p: &Path| {
            let meta = std::fs::metadata(p).ok()?;
            Some(format!(
                "{} is uid {} gid {} mode {:04o}",
                p.display(),
                meta.uid(),
                meta.gid(),
                meta.mode() & 0o7777
            ))
        };
        if let Some(seen) = owner(path).or_else(|| path.parent().and_then(owner)) {
            message.push_str(&format!("; {seen}"));
        }
    }
    message.push_str(
        ". Let the engine's uid or gid read the file (and search its directory) without making \
         it world-readable, for example group-readable (0640) by the engine's gid: see \
         \"API keys\" in deploy/release/README.md",
    );
    message
}

/// The effective uid and gid, from `/proc/self/status` (Linux; `None`
/// elsewhere).
#[cfg(unix)]
fn process_ids() -> Option<(u32, u32)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    // `Uid:` and `Gid:` list the real, effective, saved and filesystem ids.
    let effective = |name: &str| -> Option<u32> {
        let ids = status.lines().find_map(|line| line.strip_prefix(name))?;
        ids.split_whitespace().nth(1)?.parse().ok()
    };
    Some((effective("Uid:")?, effective("Gid:")?))
}

#[derive(Debug)]
struct Loaded {
    keys: HashMap<String, Entry>,
    /// SHA-256 of keys that verified, by id.
    verified: HashMap<String, [u8; 32]>,
    stamp: Option<(SystemTime, u64)>,
    checked: Instant,
}

/// Checks `lk_…` keys against `LOAMS_API_KEYS_FILE`.
#[derive(Debug)]
pub struct ApiKeyAuthenticator {
    path: PathBuf,
    state: Mutex<Loaded>,
}

impl ApiKeyAuthenticator {
    /// Reads `path`; a missing or malformed file is an error (the server
    /// refuses to start).
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| read_error(path, &e))?;
        let keys = parse_file(path, &text)?;
        if keys.is_empty() {
            tracing::warn!(
                path = %path.display(),
                "the API key file has no keys: every API key is refused"
            );
        }
        Ok(Self {
            path: path.to_owned(),
            state: Mutex::new(Loaded {
                keys,
                verified: HashMap::new(),
                stamp: stamp(path),
                checked: Instant::now(),
            }),
        })
    }

    /// Re-reads the file when it changed; keeps the old keys on an error.
    fn reload_if_changed(&self, state: &mut Loaded) {
        if state.checked.elapsed() < RELOAD_EVERY {
            return;
        }
        state.checked = Instant::now();
        let now = stamp(&self.path);
        if now == state.stamp {
            return;
        }
        match std::fs::read_to_string(&self.path)
            .map_err(|e| read_error(&self.path, &e))
            .and_then(|text| parse_file(&self.path, &text))
        {
            Ok(keys) => {
                tracing::info!(path = %self.path.display(), keys = keys.len(), "reloaded API keys");
                state.keys = keys;
                state.verified.clear();
                state.stamp = now;
            }
            Err(err) => {
                tracing::warn!(%err, "the API key file changed but cannot be read; keeping the old keys");
            }
        }
    }

    /// The principal for `key`.
    pub async fn verify(&self, key: &str) -> Result<Principal, AuthError> {
        let invalid =
            || AuthError::unauthenticated(reason::API_KEY_INVALID, "the API key is not valid");
        let id = parse_key(key).ok_or_else(|| {
            AuthError::unauthenticated(
                reason::TOKEN_MALFORMED,
                "an API key is lk_<26-char id>_<32 characters>",
            )
        })?;
        let digest: [u8; 32] = Sha256::digest(key.as_bytes()).into();
        let entry = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            self.reload_if_changed(&mut state);
            let Some(entry) = state.keys.get(id).cloned() else {
                return Err(invalid());
            };
            if let Some(known) = state.verified.get(id)
                && bool::from(known.ct_eq(&digest))
            {
                return Ok(principal(id, &entry));
            }
            entry
        };
        if !verify_hash(key, &entry.hash).await {
            return Err(invalid());
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Only if the line is still the one that verified (no reload since).
        if state.keys.get(id).is_some_and(|e| e.hash == entry.hash) {
            state.verified.insert(id.to_owned(), digest);
        }
        Ok(principal(id, &entry))
    }
}

fn principal(id: &str, entry: &Entry) -> Principal {
    Principal {
        subject: format!("apikey:{id}"),
        kind: PrincipalKind::ApiKey,
        issuer: None,
        display_name: entry.label.clone(),
        email: None,
        // A key file names no org, environment or scope (the self-hosted
        // default, LV1 plan Task 14).
        grant: super::Grant::default(),
    }
}

#[async_trait]
impl Authenticator for ApiKeyAuthenticator {
    async fn authenticate(&self, credential: &Credential) -> Option<Result<Principal, AuthError>> {
        match credential {
            Credential::ApiKey(key) => Some(self.verify(key).await),
            Credential::Jwt(_) | Credential::Session(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_with(lines: &[String]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), lines.join("\n")).unwrap();
        file
    }

    #[test]
    fn new_keys_have_the_documented_shape() {
        let (key, line) = new_key("ci runner");
        assert_eq!(key.len(), KEY_LEN);
        assert!(looks_like_key(&key), "{key}");
        assert!(line.contains("$argon2id$"), "{line}");
        assert!(line.ends_with(" ci-runner"), "{line}");
        assert!(!line.contains(&key[30..]), "the line holds no secret");
        assert!(!looks_like_key("lk_short"));
        assert!(!looks_like_key(&key.replace('_', "-")));
    }

    #[tokio::test]
    async fn right_key_verifies_and_wrong_key_does_not() {
        let (key, line) = new_key("cli");
        let (other, _) = new_key("other");
        let file = file_with(&["# keys".to_owned(), line]);
        let auth = ApiKeyAuthenticator::from_file(file.path()).unwrap();
        let p = auth.verify(&key).await.unwrap();
        assert_eq!(p.kind, PrincipalKind::ApiKey);
        assert_eq!(p.display_name, "cli");
        // Cached path.
        auth.verify(&key).await.unwrap();
        // Unknown id.
        let err = auth.verify(&other).await.unwrap_err();
        assert_eq!(err.reason, reason::API_KEY_INVALID);
        // Known id, wrong secret (same id, last char changed).
        let mut wrong = key.clone();
        let last = wrong.pop().unwrap();
        wrong.push(if last == 'a' { 'b' } else { 'a' });
        let err = auth.verify(&wrong).await.unwrap_err();
        assert_eq!(err.reason, reason::API_KEY_INVALID);
        // Malformed.
        let err = auth.verify("lk_nope").await.unwrap_err();
        assert_eq!(err.reason, reason::TOKEN_MALFORMED);
    }

    #[test]
    fn malformed_files_are_refused_with_the_line() {
        let file = file_with(&["01J0000000000000000000000Z notahash".to_owned()]);
        let err = ApiKeyAuthenticator::from_file(file.path()).unwrap_err();
        assert!(err.contains(":1:"), "{err}");
        assert!(err.contains("argon2id"), "{err}");
        let err = ApiKeyAuthenticator::from_file(Path::new("/nonexistent/keys")).unwrap_err();
        assert!(err.contains("LOAMS_API_KEYS_FILE"), "{err}");
    }

    #[test]
    fn an_unreadable_file_says_who_may_read_it() {
        let file = file_with(&[]);
        let path = file.path().display().to_string();
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let err = read_error(file.path(), &denied);
        assert!(
            err.starts_with(&format!("LOAMS_API_KEYS_FILE {path}: ")),
            "{err}"
        );
        assert!(err.contains("0640"), "{err}");
        assert!(err.contains("deploy/release/README.md"), "{err}");
        #[cfg(target_os = "linux")]
        assert!(err.contains("; the engine runs as uid "), "{err}");
        #[cfg(unix)]
        assert!(err.contains(&format!("; {path} is uid ")), "{err}");
        // Any other error is only named.
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        let err = read_error(file.path(), &missing);
        assert!(!err.contains("0640"), "{err}");
    }

    #[tokio::test]
    async fn a_deleted_line_revokes_its_key() {
        let (key, line) = new_key("cli");
        let file = file_with(std::slice::from_ref(&line));
        let auth = ApiKeyAuthenticator::from_file(file.path()).unwrap();
        auth.verify(&key).await.unwrap();
        std::fs::write(file.path(), "# revoked\n").unwrap();
        auth.state.lock().unwrap().checked = Instant::now() - RELOAD_EVERY;
        let err = auth.verify(&key).await.unwrap_err();
        assert_eq!(err.reason, reason::API_KEY_INVALID);
    }
}
