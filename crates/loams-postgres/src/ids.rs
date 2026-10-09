//! Neon's ids and LSNs in their wire text forms: tenant and timeline ids are
//! 16 bytes as 32 lower-case hex digits; an LSN is `X/Y` in upper-case hex
//! (`utils::id`, `utils::lsn` in the fork).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A malformed id or LSN.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what} {text:?}: {why}")]
pub struct ParseError {
    pub what: &'static str,
    pub text: String,
    pub why: &'static str,
}

macro_rules! hex_id {
    ($name:ident, $what:literal) => {
        #[doc = concat!("A Neon ", $what, ": 16 bytes, 32 hex digits on the wire.")]
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(pub [u8; 16]);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&hex::encode(self.0))
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({self})", stringify!($name))
            }
        }

        impl FromStr for $name {
            type Err = ParseError;
            fn from_str(s: &str) -> Result<Self, ParseError> {
                let bad = |why| ParseError {
                    what: $what,
                    text: s.to_string(),
                    why,
                };
                let bytes = hex::decode(s).map_err(|_| bad("not hex"))?;
                let bytes: [u8; 16] = bytes.try_into().map_err(|_| bad("not 16 bytes"))?;
                Ok(Self(bytes))
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

hex_id!(TenantId, "tenant id");
hex_id!(TimelineId, "timeline id");

/// A WAL position, `X/Y` on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Lsn(pub u64);

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:X}/{:X}", self.0 >> 32, self.0 & 0xffff_ffff)
    }
}

impl fmt::Debug for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lsn({self})")
    }
}

impl FromStr for Lsn {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        let bad = |why| ParseError {
            what: "LSN",
            text: s.to_string(),
            why,
        };
        let (hi, lo) = s.split_once('/').ok_or_else(|| bad("not X/Y"))?;
        let hi = u32::from_str_radix(hi, 16).map_err(|_| bad("bad high half"))?;
        let lo = u32::from_str_radix(lo, 16).map_err(|_| bad("bad low half"))?;
        Ok(Self((u64::from(hi) << 32) | u64::from(lo)))
    }
}

impl Serialize for Lsn {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Lsn {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}
