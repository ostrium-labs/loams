//! The intermediate's key at rest (design §83 §6.2).
//!
//! Sealed the way the meter seals webhook secrets and `pg-control` seals role
//! passwords (D2205, `loams_meter::notify::sealing::AgeSealer`): age, to one
//! X25519 identity kept in a 0600 key file. What the CA document holds is
//! `age:<base64 ciphertext>`; the key file never goes there. Every `meta`
//! node that serves the CA needs the same key file (a mounted Secret in a
//! cluster; the engine's data volume in compose).
//!
//! Unlike the meter's key file, this one is created only by `loams ca init`
//! ([`KeySealer::create`]). A node that serves the CA opens an existing file
//! ([`KeySealer::open_file`]): a key made on the spot could never open the
//! document's sealed key.

use std::fmt;
use std::io::Write;
use std::path::Path;

use age::secrecy::ExposeSecret;
use age::x25519::Identity;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// The prefix of a sealed value.
pub const SEALED_PREFIX: &str = "age:";

/// Seals the intermediate's key for the document and opens it again.
pub struct KeySealer {
    identity: Identity,
}

impl fmt::Debug for KeySealer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeySealer")
            .field("recipient", &self.identity.to_public().to_string())
            .finish_non_exhaustive()
    }
}

fn io_err(path: &Path, e: &std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

impl KeySealer {
    pub fn new(identity: Identity) -> Self {
        KeySealer { identity }
    }

    /// A sealer with a fresh identity (tests).
    pub fn generate() -> Self {
        KeySealer {
            identity: Identity::generate(),
        }
    }

    /// The identity in `path`, which must exist and be private (0600 on
    /// Unix).
    pub fn open_file(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| io_err(path, &e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .map_err(|e| io_err(path, &e))?
                .permissions()
                .mode();
            if mode & 0o077 != 0 {
                return Err(format!(
                    "{} is accessible by others (mode {:o}): chmod 600 it",
                    path.display(),
                    mode & 0o777
                ));
            }
        }
        let identity = text
            .trim()
            .parse::<Identity>()
            .map_err(|e| format!("{} does not hold an age identity: {e}", path.display()))?;
        Ok(KeySealer { identity })
    }

    /// The identity in `path` if the file exists (it must be private), or a
    /// new one written there with mode 0600.
    pub fn create(path: &Path) -> Result<Self, String> {
        if path.exists() {
            return Self::open_file(path);
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_err(dir, &e))?;
        }
        let identity = Identity::generate();
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let mut f = o.open(path).map_err(|e| io_err(path, &e))?;
        f.write_all(identity.to_string().expose_secret().as_bytes())
            .and_then(|()| f.write_all(b"\n"))
            .and_then(|()| f.sync_all())
            .map_err(|e| io_err(path, &e))?;
        Ok(KeySealer { identity })
    }

    /// `plain` as the document stores it: `age:<base64>`.
    pub fn seal(&self, plain: &str) -> Result<String, String> {
        let sealed = age::encrypt(&self.identity.to_public(), plain.as_bytes())
            .map_err(|e| format!("sealing the intermediate's key: {e}"))?;
        Ok(format!("{SEALED_PREFIX}{}", STANDARD.encode(sealed)))
    }

    /// What `stored` seals. A value without the prefix is refused: the
    /// intermediate's key is never stored in plaintext.
    pub fn open(&self, stored: &str) -> Result<String, String> {
        let b64 = stored
            .strip_prefix(SEALED_PREFIX)
            .ok_or_else(|| "the intermediate's key is not sealed".to_string())?;
        let sealed = STANDARD
            .decode(b64)
            .map_err(|e| format!("the sealed intermediate key: {e}"))?;
        let plain = age::decrypt(&self.identity, &sealed).map_err(|e| {
            format!("the intermediate's key does not open with this node's key file: {e}")
        })?;
        String::from_utf8(plain).map_err(|_| "the intermediate's key is not UTF-8".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_keys_open_only_with_their_key() {
        let s = KeySealer::generate();
        let sealed = s.seal("-----BEGIN PRIVATE KEY-----").expect("sealed");
        assert!(sealed.starts_with(SEALED_PREFIX));
        assert!(!sealed.contains("PRIVATE KEY"));
        assert_eq!(
            s.open(&sealed).expect("opened"),
            "-----BEGIN PRIVATE KEY-----"
        );
        assert!(KeySealer::generate().open(&sealed).is_err());
        assert!(s.open("-----BEGIN PRIVATE KEY-----").is_err(), "plaintext");
    }
}
