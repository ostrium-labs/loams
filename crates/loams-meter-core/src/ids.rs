//! Identifiers and time (design §63 §9.2).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Why an identifier or a timestamp was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("{kind} must not be empty")]
    Empty { kind: &'static str },
    #[error("{kind} '{value}' is longer than {max} bytes")]
    TooLong {
        kind: &'static str,
        value: String,
        max: usize,
    },
    #[error("{kind} '{value}' is malformed: {rule}")]
    Malformed {
        kind: &'static str,
        value: String,
        rule: &'static str,
    },
    #[error("not an RFC 3339 timestamp: '{0}'")]
    Time(String),
}

/// Nanoseconds since the Unix epoch, UTC (D374).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct UtcNanos(pub i64);

impl UtcNanos {
    pub const SECOND: i64 = 1_000_000_000;
    pub const MINUTE: i64 = 60 * Self::SECOND;
    pub const HOUR: i64 = 60 * Self::MINUTE;
    pub const DAY: i64 = 24 * Self::HOUR;
    pub const WEEK: i64 = 7 * Self::DAY;

    pub const fn from_secs(secs: i64) -> UtcNanos {
        UtcNanos(secs.saturating_mul(Self::SECOND))
    }

    /// Parses an RFC 3339 timestamp with up to nanosecond precision.
    pub fn from_rfc3339(text: &str) -> Result<UtcNanos, IdError> {
        let time =
            OffsetDateTime::parse(text, &Rfc3339).map_err(|_| IdError::Time(text.to_owned()))?;
        i64::try_from(time.unix_timestamp_nanos())
            .map(UtcNanos)
            .map_err(|_| IdError::Time(text.to_owned()))
    }

    /// RFC 3339 in UTC with nanoseconds (D364's form).
    pub fn to_rfc3339(self) -> String {
        match self.to_datetime() {
            Some(t) => {
                let (y, mo, d) = (t.year(), u8::from(t.month()), t.day());
                let (h, mi, s, n) = (t.hour(), t.minute(), t.second(), t.nanosecond());
                format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{n:09}Z")
            }
            None => String::from("1970-01-01T00:00:00.000000000Z"),
        }
    }

    pub fn to_datetime(self) -> Option<OffsetDateTime> {
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.0)).ok()
    }

    pub fn from_datetime(t: OffsetDateTime) -> Option<UtcNanos> {
        i64::try_from(t.unix_timestamp_nanos()).ok().map(UtcNanos)
    }

    pub const fn add_nanos(self, nanos: i64) -> UtcNanos {
        UtcNanos(self.0.saturating_add(nanos))
    }

    /// The start of the minute holding `self`.
    pub const fn floor_minute(self) -> UtcNanos {
        UtcNanos(self.0 - self.0.rem_euclid(Self::MINUTE))
    }

    /// The next whole minute at or after `self` (grants' `effective_at`).
    pub const fn ceil_minute(self) -> UtcNanos {
        let r = self.0.rem_euclid(Self::MINUTE);
        if r == 0 {
            self
        } else {
            UtcNanos(self.0 - r + Self::MINUTE)
        }
    }
}

macro_rules! string_id {
    ($(#[$doc:meta])* $name:ident, $kind:literal, $check:expr) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                let check: fn(&'static str, &str) -> Result<(), IdError> = $check;
                check($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:?})"), self.0)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                $name::new(s).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn bounded(kind: &'static str, value: &str, max: usize) -> Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::Empty { kind });
    }
    if value.len() > max {
        return Err(IdError::TooLong {
            kind,
            value: value.to_owned(),
            max,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(IdError::Malformed {
            kind,
            value: value.to_owned(),
            rule: "no control characters",
        });
    }
    Ok(())
}

/// `^[a-z0-9]+(?:_[a-z0-9]+)*$`, at most 64 bytes (OpenMeter's meter slug).
pub fn valid_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.split('_').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn slug_rule(kind: &'static str, value: &str) -> Result<(), IdError> {
    bounded(kind, value, 64)?;
    if valid_slug(value) {
        Ok(())
    } else {
        Err(IdError::Malformed {
            kind,
            value: value.to_owned(),
            rule: "lower-case letters and digits in words joined by '_'",
        })
    }
}

string_id!(
    /// The billing subject an event carries in `subject`: `org_<id>` for
    /// Loams' own billing, a user's customer key for user meters.
    SubjectKey,
    "subject",
    |kind, v| bounded(kind, v, 256)
);

string_id!(
    /// `<org>/<namespace>` (D364), set from the credential.
    TenantId,
    "tenantid",
    |kind, v| {
        bounded(kind, v, 256)?;
        match v.split_once('/') {
            Some((org, ns)) if !org.is_empty() && !ns.is_empty() && !ns.contains('/') => Ok(()),
            _ => Err(IdError::Malformed {
                kind,
                value: v.to_owned(),
                rule: "'<org>/<namespace>'",
            }),
        }
    }
);

impl TenantId {
    pub fn org(&self) -> &str {
        self.0.split_once('/').map_or("", |(org, _)| org)
    }

    pub fn namespace(&self) -> &str {
        self.0.split_once('/').map_or("", |(_, ns)| ns)
    }
}

string_id!(
    /// A meter's slug.
    Slug,
    "slug",
    slug_rule
);

string_id!(
    /// A feature's key; the slug rule (`house_cpu`, `llm_output_tokens`).
    FeatureKey,
    "feature key",
    slug_rule
);

string_id!(
    /// The lease that admitted work, carried in `loamslease`: 1 to 10
    /// lower-case letters and digits.
    LeaseId,
    "lease id",
    |kind, v| {
        bounded(kind, v, 10)?;
        if v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            Ok(())
        } else {
            Err(IdError::Malformed {
                kind,
                value: v.to_owned(),
                rule: "lower-case letters and digits",
            })
        }
    }
);

string_id!(EntitlementId, "entitlement id", |kind, v| bounded(
    kind, v, 128
));
string_id!(GrantId, "grant id", |kind, v| bounded(kind, v, 128));
string_id!(PlanId, "plan id", slug_rule);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_rule_matches_openmeter() {
        for ok in ["house_cpu_ms", "a", "llm_output_tokens", "a1_b2"] {
            assert!(Slug::new(ok).is_ok(), "{ok}");
        }
        for bad in ["", "_a", "a_", "a__b", "A", "a-b", "a".repeat(65).as_str()] {
            assert!(Slug::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tenant_is_org_slash_namespace() {
        let t = TenantId::new("42/7").expect("tenant");
        assert_eq!((t.org(), t.namespace()), ("42", "7"));
        for bad in ["42", "/7", "42/", "4/2/7"] {
            assert!(TenantId::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn lease_id_fits_the_extension() {
        assert!(LeaseId::new("l9k2m4q8x1").is_ok());
        assert!(LeaseId::new("l9k2m4q8x1z").is_err());
        assert!(LeaseId::new("L9").is_err());
    }

    #[test]
    fn rfc3339_round_trip_keeps_nanoseconds() {
        let t = UtcNanos::from_rfc3339("2026-10-19T09:14:03.218734Z").expect("time");
        assert_eq!(t.to_rfc3339(), "2026-10-19T09:14:03.218734000Z");
        assert_eq!(UtcNanos::from_rfc3339(&t.to_rfc3339()), Ok(t));
        let offset = UtcNanos::from_rfc3339("2026-10-19T11:14:03.218734+02:00").expect("offset");
        assert_eq!(offset, t);
    }

    #[test]
    fn minute_rounding() {
        let t = UtcNanos(UtcNanos::MINUTE * 10 + 5);
        assert_eq!(t.floor_minute(), UtcNanos(UtcNanos::MINUTE * 10));
        assert_eq!(t.ceil_minute(), UtcNanos(UtcNanos::MINUTE * 11));
        assert_eq!(
            UtcNanos(UtcNanos::MINUTE).ceil_minute(),
            UtcNanos(UtcNanos::MINUTE)
        );
        assert_eq!(UtcNanos(-1).floor_minute(), UtcNanos(-UtcNanos::MINUTE));
    }
}
