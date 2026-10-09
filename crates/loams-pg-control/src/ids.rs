//! Resource ids (design §46 §3): `prj-`, `br-`, `ep-` and `cmp-`, each
//! followed by a 26-character Crockford ULID in upper case, and the Neon
//! tenant and timeline ids derived from them.
//!
//! The text form is canonical: parsing accepts exactly what `Display`
//! writes (upper case, the right prefix, 26 characters), so an id has one
//! spelling, one store key and one derived tenant or timeline id.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use ulid::Ulid;

/// A malformed resource id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {what} id {text:?}: {why}")]
pub struct IdError {
    pub what: &'static str,
    pub text: String,
    pub why: &'static str,
}

macro_rules! prefixed_id {
    ($name:ident, $prefix:literal, $what:literal) => {
        #[doc = concat!("A ", $what, " id: `", $prefix, "<ULID>`.")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Ulid);

        impl $name {
            /// The text prefix, dash included.
            pub const PREFIX: &'static str = $prefix;

            /// A fresh id (the current time and random bits).
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                Self(Ulid::generate())
            }

            pub const fn from_ulid(u: Ulid) -> Self {
                Self(u)
            }

            pub const fn ulid(&self) -> Ulid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({self})", stringify!($name))
            }
        }

        impl FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, IdError> {
                parse_prefixed(s, $prefix, $what).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

prefixed_id!(ProjectId, "prj-", "project");
prefixed_id!(BranchId, "br-", "branch");
prefixed_id!(EndpointId, "ep-", "endpoint");
prefixed_id!(ComputeId, "cmp-", "compute");

fn parse_prefixed(s: &str, prefix: &str, what: &'static str) -> Result<Ulid, IdError> {
    let bad = |why| IdError {
        what,
        text: s.to_string(),
        why,
    };
    let rest = s.strip_prefix(prefix).ok_or_else(|| bad("wrong prefix"))?;
    if rest.len() != ulid::ULID_LEN {
        return Err(bad("not 26 characters after the prefix"));
    }
    let u = Ulid::from_string(rest).map_err(|_| bad("not a Crockford ULID"))?;
    // Ulid::from_string accepts lower case; the canonical form does not.
    if u.to_string() != rest {
        return Err(bad("not in canonical (upper-case) form"));
    }
    Ok(u)
}

fn derive(domain: &str, id: &str) -> [u8; 16] {
    let mut h = Sha256::new();
    h.update(domain.as_bytes());
    h.update(id.as_bytes());
    let digest = h.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// The project's Neon tenant id: `SHA-256("loams/pg/tenant/" ‖ project_id)[0..16]`,
/// over the id's canonical text (`prj-<ULID>`).
pub fn tenant_id(project: &ProjectId) -> [u8; 16] {
    derive("loams/pg/tenant/", &project.to_string())
}

/// The branch's Neon timeline id: `SHA-256("loams/pg/timeline/" ‖ branch_id)[0..16]`,
/// over the id's canonical text (`br-<ULID>`).
pub fn timeline_id(branch: &BranchId) -> [u8; 16] {
    derive("loams/pg/timeline/", &branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex16(s: &str) -> [u8; 16] {
        hex::decode(s).unwrap().try_into().unwrap()
    }

    // The vectors were computed outside Rust, with
    // `printf 'loams/pg/tenant/prj-…' | sha256sum | cut -c1-32`.
    #[test]
    fn tenant_id_is_deterministic_and_matches_vector() {
        let cases = [
            (
                "prj-01ARZ3NDEKTSV4RRFFQ69G5FAV",
                "1d5de71c1e7a2ef2a926f44027dac477",
            ),
            (
                "prj-00000000000000000000000000",
                "c679878df0bddfadb26141a5f0e1e39c",
            ),
        ];
        for (id, want) in cases {
            let p: ProjectId = id.parse().unwrap();
            assert_eq!(tenant_id(&p), hex16(want), "{id}");
            assert_eq!(tenant_id(&p), tenant_id(&id.parse().unwrap()));
        }
    }

    #[test]
    fn timeline_id_matches_vector() {
        let cases = [
            (
                "br-01ARZ3NDEKTSV4RRFFQ69G5FAV",
                "b836f70bf260e531d3aedff4b021e904",
            ),
            (
                "br-7ZZZZZZZZZZZZZZZZZZZZZZZZZ",
                "8a42c68d5aba4c4f0d01af7f5f2d8bac",
            ),
        ];
        for (id, want) in cases {
            let b: BranchId = id.parse().unwrap();
            assert_eq!(timeline_id(&b), hex16(want), "{id}");
        }
    }

    #[test]
    fn tenant_and_timeline_domains_differ() {
        let u = Ulid::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        assert_ne!(
            tenant_id(&ProjectId::from_ulid(u)),
            timeline_id(&BranchId::from_ulid(u))
        );
    }

    #[test]
    fn ids_round_trip_through_text_and_serde() {
        let p = ProjectId::new();
        let b = BranchId::new();
        let e = EndpointId::new();
        let c = ComputeId::new();
        assert!(p.to_string().starts_with("prj-") && p.to_string().len() == 30);
        assert!(b.to_string().starts_with("br-"));
        assert!(e.to_string().starts_with("ep-"));
        assert!(c.to_string().starts_with("cmp-"));
        assert_eq!(p.to_string().parse::<ProjectId>().unwrap(), p);
        assert_eq!(b.to_string().parse::<BranchId>().unwrap(), b);
        assert_eq!(e.to_string().parse::<EndpointId>().unwrap(), e);
        assert_eq!(c.to_string().parse::<ComputeId>().unwrap(), c);
        let bytes = postcard::to_stdvec(&p).unwrap();
        assert_eq!(postcard::from_bytes::<ProjectId>(&bytes).unwrap(), p);
    }

    #[test]
    fn rejects_non_canonical_ids() {
        for bad in [
            "br-01ARZ3NDEKTSV4RRFFQ69G5FAV",   // wrong prefix
            "prj01ARZ3NDEKTSV4RRFFQ69G5FAV",   // no dash
            "prj-01arz3ndektsv4rrffq69g5fav",  // lower case
            "prj-01ARZ3NDEKTSV4RRFFQ69G5FA",   // 25 characters
            "prj-01ARZ3NDEKTSV4RRFFQ69G5FAVX", // 27 characters
            "prj-01ARZ3NDEKTSV4RRFFQ69G5FAU",  // U is not Crockford
            "prj-81ARZ3NDEKTSV4RRFFQ69G5FAV",  // overflows 128 bits
            "prj-",
            "",
        ] {
            assert!(bad.parse::<ProjectId>().is_err(), "{bad:?} parsed");
        }
    }
}
