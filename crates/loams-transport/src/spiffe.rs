//! SPIFFE-style workload identities (design §83 §6.1, D2308).
//!
//! ```text
//! spiffe://<cluster-id>/node/<node-id>
//! spiffe://<cluster-id>/role/<role>
//! spiffe://<cluster-id>/svc/<service>
//! ```
//!
//! The format is SPIFFE's (a trust domain and a path), so SPIRE or
//! cert-manager can issue the same IDs (D2314). Loams uses exactly two path
//! segments: a kind and a name.

use std::fmt;
use std::str::FromStr;

/// The longest SPIFFE ID accepted (SPIFFE's own limit is 2048 bytes).
pub const MAX_LEN: usize = 2048;

/// What an identity names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    /// A Loams cluster node (`node/<node-id>`).
    Node,
    /// A product role a node runs (`role/<role>`).
    Role,
    /// A separate service (`svc/<service>`).
    Svc,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Node => "node",
            Kind::Role => "role",
            Kind::Svc => "svc",
        }
    }
}

/// A parsed `spiffe://<cluster>/<kind>/<name>` identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpiffeId {
    cluster: String,
    kind: Kind,
    name: String,
}

/// Why a string is not a Loams SPIFFE ID.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpiffeIdError {
    #[error("not a spiffe:// URI")]
    Scheme,
    #[error("longer than {MAX_LEN} bytes")]
    TooLong,
    #[error("invalid trust domain {0:?}")]
    TrustDomain(String),
    #[error("path must be /<node|role|svc>/<name>, got {0:?}")]
    Path(String),
    #[error("invalid name {0:?}")]
    Name(String),
}

/// SPIFFE trust domains: lowercase letters, digits, `.`, `-` and `_`.
fn valid_trust_domain(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_')
        })
}

/// Path segments: SPIFFE's unreserved characters, no `.` or `..` segments.
fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

impl SpiffeId {
    /// An identity from its parts, checked as [`FromStr`] checks them.
    pub fn new(cluster: &str, kind: Kind, name: &str) -> Result<Self, SpiffeIdError> {
        format!("spiffe://{cluster}/{}/{name}", kind.as_str()).parse()
    }

    pub fn cluster(&self) -> &str {
        &self.cluster
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// `node/1`, `role/gateway`, `svc/meter-agent`: the part route policies
    /// name (D2315), without the cluster.
    pub fn short(&self) -> String {
        format!("{}/{}", self.kind.as_str(), self.name)
    }
}

impl FromStr for SpiffeId {
    type Err = SpiffeIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() > MAX_LEN {
            return Err(SpiffeIdError::TooLong);
        }
        let rest = s.strip_prefix("spiffe://").ok_or(SpiffeIdError::Scheme)?;
        let (cluster, path) = rest
            .split_once('/')
            .ok_or_else(|| SpiffeIdError::Path(String::new()))?;
        if !valid_trust_domain(cluster) {
            return Err(SpiffeIdError::TrustDomain(cluster.to_string()));
        }
        let mut parts = path.split('/');
        let (Some(kind), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(SpiffeIdError::Path(path.to_string()));
        };
        let kind = match kind {
            "node" => Kind::Node,
            "role" => Kind::Role,
            "svc" => Kind::Svc,
            _ => return Err(SpiffeIdError::Path(path.to_string())),
        };
        if !valid_segment(name) {
            return Err(SpiffeIdError::Name(name.to_string()));
        }
        Ok(SpiffeId {
            cluster: cluster.to_string(),
            kind,
            name: name.to_string(),
        })
    }
}

impl fmt::Display for SpiffeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "spiffe://{}/{}/{}",
            self.cluster,
            self.kind.as_str(),
            self.name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_kind() {
        for (s, kind, name) in [
            ("spiffe://c0ffee/node/7", Kind::Node, "7"),
            ("spiffe://c0ffee/role/gateway", Kind::Role, "gateway"),
            ("spiffe://c0ffee/svc/meter-agent", Kind::Svc, "meter-agent"),
        ] {
            let id: SpiffeId = s.parse().unwrap();
            assert_eq!(id.kind(), kind);
            assert_eq!(id.name(), name);
            assert_eq!(id.cluster(), "c0ffee");
            assert_eq!(id.to_string(), s);
        }
        assert_eq!(
            SpiffeId::new("c", Kind::Svc, "tikv").unwrap().short(),
            "svc/tikv"
        );
    }

    #[test]
    fn refuses_what_is_not_an_identity() {
        for bad in [
            "https://c/node/1",
            "spiffe://",
            "spiffe://c",
            "spiffe://c/",
            "spiffe://C/node/1",
            "spiffe://c/node",
            "spiffe://c/node/1/extra",
            "spiffe://c/user/1",
            "spiffe://c/role/..",
            "spiffe://c/role/a b",
            "spiffe://c/role/",
        ] {
            assert!(bad.parse::<SpiffeId>().is_err(), "{bad} was accepted");
        }
        let long = format!("spiffe://c/svc/{}", "a".repeat(MAX_LEN));
        assert_eq!(long.parse::<SpiffeId>(), Err(SpiffeIdError::TooLong));
    }
}
