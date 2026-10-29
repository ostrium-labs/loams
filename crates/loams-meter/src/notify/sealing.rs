//! Webhook signing secrets at rest (design §63 §15; ME1 role wiring, part
//! 2c, D2205).
//!
//! Endpoint records live in the notifications document, which every meter
//! instance reads from the metastore and the bucket. The secrets in them
//! are sealed the way `pg-control` seals its role passwords: age, to one
//! X25519 identity kept in a 0600 key file that is created on first use
//! (`loams_pg_control::secrets::FileSecretStore`'s key handling). What the
//! document holds is `age:<base64 ciphertext>`; the key never goes there.
//! Every instance that delivers webhooks needs the same key file (a mounted
//! Kubernetes Secret in a cluster).
//!
//! A value without the `age:` prefix is a secret stored in plaintext before
//! this module existed. It still signs, and the next rotation seals its
//! successor.

use std::fmt;
use std::io::Write;
use std::path::Path;

use age::secrecy::ExposeSecret;
use age::x25519::Identity;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::error::{MeterError, Result};

/// The prefix of a sealed secret.
pub const SEALED_PREFIX: &str = "age:";

/// Seals secrets for storage and opens them for signing.
pub trait SecretSealer: Send + Sync + fmt::Debug {
    /// `plain` as it is stored.
    fn seal(&self, plain: &str) -> Result<String>;
    /// The secret `stored` holds.
    fn open(&self, stored: &str) -> Result<String>;
}

/// Seals nothing: for tests, and for hosts that keep the notifications
/// document somewhere already encrypted at rest. Opens sealed values never.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unsealed;

impl SecretSealer for Unsealed {
    fn seal(&self, plain: &str) -> Result<String> {
        Ok(plain.to_string())
    }

    fn open(&self, stored: &str) -> Result<String> {
        if stored.starts_with(SEALED_PREFIX) {
            return Err(MeterError::Unavailable(
                "a sealed webhook secret, and no key to open it".into(),
            ));
        }
        Ok(stored.to_string())
    }
}

/// age to one X25519 identity.
pub struct AgeSealer {
    identity: Identity,
}

impl fmt::Debug for AgeSealer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgeSealer")
            .field("recipient", &self.identity.to_public().to_string())
            .finish_non_exhaustive()
    }
}

fn io_err(path: &Path, e: &std::io::Error) -> MeterError {
    MeterError::Unavailable(format!("{}: {e}", path.display()))
}

impl AgeSealer {
    pub fn new(identity: Identity) -> Self {
        AgeSealer { identity }
    }

    /// The identity in `path`, which must be private (0600 on Unix); a new
    /// one written there (0600) when the file does not exist.
    pub fn from_key_file(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mode = std::fs::metadata(path)
                        .map_err(|e| io_err(path, &e))?
                        .permissions()
                        .mode();
                    if mode & 0o077 != 0 {
                        return Err(MeterError::Unavailable(format!(
                            "{} is accessible by others (mode {:o}): chmod 600 it",
                            path.display(),
                            mode & 0o777
                        )));
                    }
                }
                let identity = text.trim().parse::<Identity>().map_err(|e| {
                    MeterError::Unavailable(format!(
                        "{} does not hold an age identity: {e}",
                        path.display()
                    ))
                })?;
                Ok(AgeSealer { identity })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
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
                Ok(AgeSealer { identity })
            }
            Err(e) => Err(io_err(path, &e)),
        }
    }
}

impl SecretSealer for AgeSealer {
    fn seal(&self, plain: &str) -> Result<String> {
        let sealed = age::encrypt(&self.identity.to_public(), plain.as_bytes())
            .map_err(|e| MeterError::Unavailable(format!("sealing a webhook secret: {e}")))?;
        Ok(format!("{SEALED_PREFIX}{}", STANDARD.encode(sealed)))
    }

    fn open(&self, stored: &str) -> Result<String> {
        let Some(b64) = stored.strip_prefix(SEALED_PREFIX) else {
            return Ok(stored.to_string());
        };
        let sealed = STANDARD
            .decode(b64)
            .map_err(|e| MeterError::Unavailable(format!("a sealed webhook secret: {e}")))?;
        let plain = age::decrypt(&self.identity, &sealed).map_err(|e| {
            MeterError::Unavailable(format!(
                "a webhook secret does not open with this instance's key: {e}"
            ))
        })?;
        String::from_utf8(plain)
            .map_err(|_| MeterError::Unavailable("a webhook secret is not UTF-8".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_secrets_open_and_say_nothing() {
        let s = AgeSealer::new(Identity::generate());
        let sealed = s.seal("whsec_abc").expect("sealed");
        assert!(sealed.starts_with(SEALED_PREFIX));
        assert!(!sealed.contains("whsec_abc"));
        assert_eq!(s.open(&sealed).expect("opened"), "whsec_abc");
        // A plaintext secret from before sealing still opens.
        assert_eq!(s.open("whsec_old").expect("legacy"), "whsec_old");
        // Another key does not open it.
        let other = AgeSealer::new(Identity::generate());
        assert!(other.open(&sealed).is_err());
        assert!(Unsealed.open(&sealed).is_err());
    }

    #[test]
    fn a_key_file_is_made_private_and_read_back() {
        let dir = std::env::temp_dir().join(format!("meter-sealing-{}", ulid::Ulid::generate()));
        let path = dir.join("secrets.key");
        let a = AgeSealer::from_key_file(&path).expect("created");
        let sealed = a.seal("whsec_x").expect("sealed");
        let b = AgeSealer::from_key_file(&path).expect("read back");
        assert_eq!(b.open(&sealed).expect("opened"), "whsec_x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
