//! Who is calling (SP-a D1993; design 81 R5, R18).
//!
//! Spark Connect has no authentication of its own. PySpark sends a
//! `sc://host:port/;token=…` URI's token as `authorization: Bearer …`
//! metadata, and every other unknown URI parameter as a metadata header of
//! that name, so a credential arrives either as
//!
//! - `authorization: Bearer <lk_ key or Loams token>`, or
//! - `x-loams-api-key: <lk_ key>` (`sc://…/;x-loams-api-key=lk_…`, for
//!   clients that cannot use TLS on a loopback compose listener: PySpark
//!   refuses `token=` without `use_ssl=true`).
//!
//! Both present and different is refused. The credential becomes a
//! [`Principal`]: the Loams namespace the key belongs to and the subject the
//! authorizer checks. The proxy then removes both headers before forwarding
//! (the backend gets its own pre-shared token, never the user's key).
//!
//! [`KeyTable`] checks `lk_<26-char ULID>_<32 base62>` keys by id and SHA-256
//! in constant time (pg-control's keys-file shape plus `namespace`).
//! [`Authenticator`] is async so the identity service (design 76) can plug in
//! a remote verifier for OIDC tokens.

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use loams_authz::{Subject, SubjectKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tonic::Status;
use tonic::metadata::MetadataMap;

/// The metadata header for a key sent as an `sc://` URI parameter.
pub const API_KEY_HEADER: &str = "x-loams-api-key";

/// The prefix of a Loams API key.
pub const API_KEY_PREFIX: &str = "lk_";

const API_KEY_SECRET_LEN: usize = 32;

/// An authenticated caller.
#[derive(Clone, PartialEq, Eq)]
pub struct Principal {
    /// The Loams namespace the credential belongs to: the tenant (R2).
    pub namespace: String,
    /// Whom the authorizer checks.
    pub subject: Subject,
    /// The key id, when the credential was an API key (for logs; never the key).
    pub credential_id: Option<String>,
}

impl Principal {
    /// The `user_context.user_id` the backend sees: `<kind>:<id>`, so two
    /// subjects never share a Spark session even on one backend.
    pub fn spark_user_id(&self) -> String {
        self.subject.fga_user()
    }
}

impl fmt::Debug for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principal")
            .field("namespace", &self.namespace)
            .field("subject", &self.subject.fga_user())
            .field("credential_id", &self.credential_id)
            .finish()
    }
}

/// A credential taken from request metadata. Its `Debug` never shows the secret.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential(String);

impl Credential {
    /// The secret, for a verifier.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credential(<redacted>)")
    }
}

/// The caller's credential from `authorization: Bearer …` or
/// [`API_KEY_HEADER`].
///
/// # Errors
///
/// `UNAUTHENTICATED` when there is none, when a header is repeated or
/// malformed, or when both are present and differ.
pub fn credential(metadata: &MetadataMap) -> Result<Credential, Status> {
    let bearer = single(metadata, "authorization")?
        .map(|value| {
            let (scheme, token) = value
                .split_once(' ')
                .ok_or_else(|| Status::unauthenticated("authorization is not a bearer token"))?;
            if !scheme.eq_ignore_ascii_case("bearer") {
                return Err(Status::unauthenticated(
                    "authorization is not a bearer token",
                ));
            }
            Ok(token.trim().to_owned())
        })
        .transpose()?;
    let key = single(metadata, API_KEY_HEADER)?.map(|v| v.trim().to_owned());
    match (bearer, key) {
        (Some(a), Some(b)) if a != b => Err(Status::unauthenticated(
            "authorization and x-loams-api-key carry different credentials",
        )),
        (Some(a), _) | (None, Some(a)) if !a.is_empty() => Ok(Credential(a)),
        _ => Err(Status::unauthenticated(
            "missing credential: connect with sc://…/;use_ssl=true;token=<Loams API key>",
        )),
    }
}

fn single<'a>(metadata: &'a MetadataMap, name: &str) -> Result<Option<&'a str>, Status> {
    let mut values = metadata.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Status::unauthenticated(format!("{name} is repeated")));
    }
    first
        .to_str()
        .map(Some)
        .map_err(|_| Status::unauthenticated(format!("{name} is not ASCII")))
}

/// Turns a credential into a principal.
#[async_trait]
pub trait Authenticator: Send + Sync + 'static {
    /// The caller, or `UNAUTHENTICATED`. `UNAVAILABLE` when a remote
    /// verifier cannot answer (never an allow).
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, Status>;
}

/// One API key in a [`KeyTable`]: never the key, only its id and digest.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyEntry {
    /// The key's ULID, as in the key.
    pub id: String,
    /// Lower-case hex SHA-256 of the whole key.
    pub sha256: String,
    /// The Loams namespace the key belongs to.
    pub namespace: String,
    /// The subject's id.
    pub subject: String,
    /// `user`, `service_account` or `agent`.
    #[serde(default = "default_kind")]
    pub kind: SubjectKind,
}

fn default_kind() -> SubjectKind {
    SubjectKind::ServiceAccount
}

impl fmt::Debug for KeyEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyEntry")
            .field("id", &self.id)
            .field("namespace", &self.namespace)
            .field("subject", &self.subject)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// API keys checked against a fixed table of digests.
#[derive(Debug, Clone, Default)]
pub struct KeyTable {
    by_id: BTreeMap<String, KeyEntry>,
}

impl KeyTable {
    /// A table of `entries`.
    ///
    /// # Errors
    ///
    /// A message naming the first malformed entry or repeated id.
    pub fn new(entries: impl IntoIterator<Item = KeyEntry>) -> Result<Self, String> {
        let mut by_id = BTreeMap::new();
        for mut entry in entries {
            entry.sha256 = entry.sha256.to_ascii_lowercase();
            if entry.id.len() != 26 || !entry.id.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err(format!("key id {:?} is not a ULID", entry.id));
            }
            if entry.sha256.len() != 64 || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("key {}: sha256 is not 64 hex digits", entry.id));
            }
            if entry.namespace.is_empty() || entry.subject.is_empty() {
                return Err(format!(
                    "key {}: namespace and subject are required",
                    entry.id
                ));
            }
            if let Some(prev) = by_id.insert(entry.id.clone(), entry) {
                return Err(format!("key id {} is repeated", prev.id));
            }
        }
        Ok(Self { by_id })
    }

    /// The SHA-256 hex of a key, as the table stores it.
    pub fn hash(key: &str) -> String {
        hex::encode(Sha256::digest(key.as_bytes()))
    }

    /// How many keys the table holds.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The id in a well-formed key: `lk_<26-char ULID>_<32 base62>`.
pub fn key_id(key: &str) -> Option<&str> {
    let rest = key.strip_prefix(API_KEY_PREFIX)?;
    let (id, secret) = rest.split_once('_')?;
    (id.len() == 26
        && secret.len() == API_KEY_SECRET_LEN
        && secret.bytes().all(|b| b.is_ascii_alphanumeric()))
    .then_some(id)
}

#[async_trait]
impl Authenticator for KeyTable {
    async fn authenticate(&self, credential: &Credential) -> Result<Principal, Status> {
        let key = credential.expose();
        let id = key_id(key).ok_or_else(|| Status::unauthenticated("not a Loams API key"))?;
        let entry = self
            .by_id
            .get(id)
            .ok_or_else(|| Status::unauthenticated("unknown API key"))?;
        let digest = Self::hash(key);
        if !bool::from(digest.as_bytes().ct_eq(entry.sha256.as_bytes())) {
            return Err(Status::unauthenticated("unknown API key"));
        }
        Ok(Principal {
            namespace: entry.namespace.clone(),
            subject: Subject::new(entry.kind, entry.subject.clone()),
            credential_id: Some(entry.id.clone()),
        })
    }
}

/// Everyone is the given principal: the loopback-only dev mode
/// (`insecure_dev`), never on a public listener. A credential is still
/// required to be present, so clients are configured as in production.
#[derive(Debug, Clone)]
pub struct DevAuthenticator(pub Principal);

#[async_trait]
impl Authenticator for DevAuthenticator {
    async fn authenticate(&self, _: &Credential) -> Result<Principal, Status> {
        Ok(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::metadata::MetadataValue;

    const KEY: &str = "lk_01JABCDEFGHJKMNPQRSTVWXYZ0_abcdefghijklmnopqrstuvwxyz012345";

    fn table() -> KeyTable {
        KeyTable::new([KeyEntry {
            id: "01JABCDEFGHJKMNPQRSTVWXYZ0".into(),
            sha256: KeyTable::hash(KEY),
            namespace: "ns-a".into(),
            subject: "ci-bot".into(),
            kind: SubjectKind::ServiceAccount,
        }])
        .unwrap()
    }

    fn md(pairs: &[(&'static str, &str)]) -> MetadataMap {
        let mut m = MetadataMap::new();
        for (k, v) in pairs {
            m.append(*k, MetadataValue::try_from(*v).unwrap());
        }
        m
    }

    #[test]
    fn bearer_and_header_credentials() {
        let bearer = format!("Bearer {KEY}");
        assert_eq!(
            credential(&md(&[("authorization", &bearer)]))
                .unwrap()
                .expose(),
            KEY
        );
        assert_eq!(
            credential(&md(&[(API_KEY_HEADER, KEY)])).unwrap().expose(),
            KEY
        );
        // The same key both ways is fine.
        assert!(credential(&md(&[("authorization", &bearer), (API_KEY_HEADER, KEY)])).is_ok());
    }

    #[test]
    fn credential_errors() {
        let missing = credential(&MetadataMap::new()).unwrap_err();
        assert_eq!(missing.code(), tonic::Code::Unauthenticated);
        assert!(credential(&md(&[("authorization", "Basic abc")])).is_err());
        assert!(credential(&md(&[("authorization", "Bearer a"), (API_KEY_HEADER, "b")])).is_err());
        assert!(
            credential(&md(&[
                ("authorization", "Bearer a"),
                ("authorization", "Bearer a")
            ]))
            .is_err()
        );
        assert!(credential(&md(&[("authorization", "Bearer  ")])).is_err());
    }

    #[test]
    fn credential_debug_is_redacted() {
        let c = credential(&md(&[(API_KEY_HEADER, KEY)])).unwrap();
        assert!(!format!("{c:?}").contains("lk_"));
    }

    #[tokio::test]
    async fn key_table_maps_a_key_to_its_namespace() {
        let p = table().authenticate(&Credential(KEY.into())).await.unwrap();
        assert_eq!(p.namespace, "ns-a");
        assert_eq!(p.spark_user_id(), "service_account:ci-bot");
        assert_eq!(
            p.credential_id.as_deref(),
            Some("01JABCDEFGHJKMNPQRSTVWXYZ0")
        );
    }

    #[tokio::test]
    async fn key_table_refuses_wrong_secret_and_garbage() {
        let wrong = "lk_01JABCDEFGHJKMNPQRSTVWXYZ0_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345";
        for bad in [wrong, "lk_short", "not-a-key", ""] {
            let err = table()
                .authenticate(&Credential(bad.into()))
                .await
                .unwrap_err();
            assert_eq!(err.code(), tonic::Code::Unauthenticated, "{bad}");
        }
    }

    #[test]
    fn key_table_validates_entries() {
        let entry = |id: &str| KeyEntry {
            id: id.into(),
            sha256: "0".repeat(64),
            namespace: "n".into(),
            subject: "s".into(),
            kind: SubjectKind::User,
        };
        assert!(KeyTable::new([entry("short")]).is_err());
        let id = "01JABCDEFGHJKMNPQRSTVWXYZ0";
        assert!(KeyTable::new([entry(id), entry(id)]).is_err());
        assert_eq!(KeyTable::new([entry(id)]).unwrap().len(), 1);
    }
}
