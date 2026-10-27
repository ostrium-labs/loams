//! The mapping between `loams.agentd.v1` messages and the ndjson wire's Rust
//! types (`loams-agentd-proto`).
//!
//! Every handler turns its request into the ndjson twin's params type and the
//! twin's result into its response through [`Wire`], so the two wires carry
//! the same values (Task 6's parity suite compares them through these same
//! impls). Each struct impl destructures the Rust type without `..`, so a
//! field added to the ndjson wire fails to compile here until it is mapped.
//!
//! Conventions (see `proto/loams/agentd/v1/studio.proto`):
//! - an RFC 3339 instant (`DateTime<Utc>`) is `int64` milliseconds;
//! - `usize` is `uint64` and `u16` is `uint32`, range-checked on the way in;
//! - an internally tagged enum is a message with a `oneof`;
//! - a JSON value is `google.protobuf.Value`;
//! - an enum value the daemon does not know, or `*_UNSPECIFIED` where a value
//!   is required, is a [`MapError`] (`bad_params`).

use chrono::{DateTime, Utc};

/// A request the daemon cannot map to its ndjson params (an unset required
/// field, an enum value it does not know, a number out of range).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapError(pub String);

impl std::fmt::Display for MapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MapError {}

impl MapError {
    /// A required message field that is unset.
    pub fn missing(field: &str) -> Self {
        Self(format!("{field} is required"))
    }

    /// An enum value the daemon does not know, or an unset required one.
    pub fn enum_value(name: &str, value: i32) -> Self {
        Self(format!("{name} {value} is not a value this daemon accepts"))
    }

    /// A `oneof` with no field set.
    pub fn empty_oneof(name: &str) -> Self {
        Self(format!("{name} has no variant set"))
    }
}

/// One Rust wire type and its `loams.agentd.v1` message, scalar or enum.
pub trait Wire: Sized {
    /// The proto side.
    type Pb;
    /// The proto value of `self`.
    fn to_pb(&self) -> Self::Pb;
    /// The Rust value of a proto value.
    ///
    /// # Errors
    ///
    /// [`MapError`] when the proto value has no Rust counterpart.
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError>;
}

macro_rules! identity {
    ($($t:ty),* $(,)?) => {$(
        impl Wire for $t {
            type Pb = $t;
            fn to_pb(&self) -> $t {
                self.clone()
            }
            fn from_pb(pb: $t) -> Result<Self, MapError> {
                Ok(pb)
            }
        }
    )*};
}

identity!(String, bool, i32, i64, u32, u64, f32, f64);

impl Wire for usize {
    type Pb = u64;
    fn to_pb(&self) -> u64 {
        u64::try_from(*self).unwrap_or(u64::MAX)
    }
    fn from_pb(pb: u64) -> Result<Self, MapError> {
        usize::try_from(pb).map_err(|_| MapError(format!("{pb} is out of range")))
    }
}

impl Wire for u16 {
    type Pb = u32;
    fn to_pb(&self) -> u32 {
        u32::from(*self)
    }
    fn from_pb(pb: u32) -> Result<Self, MapError> {
        u16::try_from(pb).map_err(|_| MapError(format!("{pb} is out of range")))
    }
}

impl Wire for DateTime<Utc> {
    type Pb = i64;
    fn to_pb(&self) -> i64 {
        self.timestamp_millis()
    }
    fn from_pb(pb: i64) -> Result<Self, MapError> {
        DateTime::from_timestamp_millis(pb)
            .ok_or_else(|| MapError(format!("{pb} ms is not a valid instant")))
    }
}

impl Wire for serde_json::Value {
    type Pb = buffa_types::google::protobuf::Value;
    fn to_pb(&self) -> Self::Pb {
        // `google.protobuf.Value`'s serde form is the JSON value itself.
        serde_json::from_value(self.clone()).unwrap_or_default()
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        let mut value = serde_json::to_value(&pb).map_err(|error| MapError(error.to_string()))?;
        integral_numbers(&mut value);
        Ok(value)
    }
}

/// `google.protobuf.Value` holds every number as a double, so `10` comes back
/// as `10.0`. A whole number within a double's exact range is written back as
/// an integer, as JSON (and the ndjson wire) has it; JavaScript cannot tell
/// the two apart either way.
fn integral_numbers(value: &mut serde_json::Value) {
    const EXACT: f64 = 9_007_199_254_740_992.0; // 2^53
    match value {
        serde_json::Value::Number(number) => {
            if let Some(float) = number.as_f64()
                && number.is_f64()
                && float.fract() == 0.0
                && float.abs() <= EXACT
            {
                // In range by the check above, so the cast is exact.
                #[allow(clippy::cast_possible_truncation)]
                let integer = float as i64;
                *number = serde_json::Number::from(integer);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(integral_numbers),
        serde_json::Value::Object(map) => map.values_mut().for_each(integral_numbers),
        _ => {}
    }
}

impl<T: Wire> Wire for Option<T> {
    type Pb = Option<T::Pb>;
    fn to_pb(&self) -> Self::Pb {
        self.as_ref().map(Wire::to_pb)
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        pb.map(T::from_pb).transpose()
    }
}

impl<T: Wire> Wire for Vec<T> {
    type Pb = Vec<T::Pb>;
    fn to_pb(&self) -> Self::Pb {
        self.iter().map(Wire::to_pb).collect()
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        pb.into_iter().map(T::from_pb).collect()
    }
}

/// A message field from a required Rust value.
pub fn msg<T, P>(value: &T) -> buffa::MessageField<T::Pb, P>
where
    T: Wire,
    T::Pb: Default,
    P: buffa::ProtoBox<T::Pb>,
{
    buffa::MessageField::some(value.to_pb())
}

/// A message field from an optional Rust value; `None` is unset.
pub fn opt_msg<T, P>(value: &Option<T>) -> buffa::MessageField<T::Pb, P>
where
    T: Wire,
    T::Pb: Default,
    P: buffa::ProtoBox<T::Pb>,
{
    value.as_ref().map(Wire::to_pb).into()
}

/// The Rust value of a required message field.
///
/// # Errors
///
/// [`MapError::missing`] when the field is unset.
pub fn from_msg<T, P>(field: buffa::MessageField<T::Pb, P>, name: &str) -> Result<T, MapError>
where
    T: Wire,
    T::Pb: Default,
    P: buffa::ProtoBox<T::Pb>,
{
    T::from_pb(field.into_option().ok_or_else(|| MapError::missing(name))?)
}

/// The Rust value of an optional message field.
///
/// # Errors
///
/// As [`Wire::from_pb`].
pub fn from_opt_msg<T, P>(field: buffa::MessageField<T::Pb, P>) -> Result<Option<T>, MapError>
where
    T: Wire,
    T::Pb: Default,
    P: buffa::ProtoBox<T::Pb>,
{
    field.into_option().map(T::from_pb).transpose()
}

/// The Rust value of a message field whose unset state means the default
/// (a `WorkspaceTarget` that names nothing).
///
/// # Errors
///
/// As [`Wire::from_pb`].
pub fn from_default_msg<T, P>(field: buffa::MessageField<T::Pb, P>) -> Result<T, MapError>
where
    T: Wire,
    T::Pb: Default,
    P: buffa::ProtoBox<T::Pb>,
{
    T::from_pb(field.into_option().unwrap_or_default())
}

/// A proto map from a Rust map of [`Wire`] values.
pub fn map_to<'a, V, I, M>(entries: I) -> M
where
    V: Wire + 'a,
    I: IntoIterator<Item = (&'a String, &'a V)>,
    M: FromIterator<(String, V::Pb)>,
{
    entries
        .into_iter()
        .map(|(key, value)| (key.clone(), value.to_pb()))
        .collect()
}

/// A Rust map from a proto map.
///
/// # Errors
///
/// As [`Wire::from_pb`] for any value.
pub fn map_from<V, I, M>(entries: I) -> Result<M, MapError>
where
    V: Wire,
    I: IntoIterator<Item = (String, V::Pb)>,
    M: FromIterator<(String, V)>,
{
    entries
        .into_iter()
        .map(|(key, value)| Ok((key, V::from_pb(value)?)))
        .collect()
}

/// `impl Wire` for a fieldless Rust enum and its proto enum, variant by
/// variant. The proto side is `buffa::EnumValue`, so an unknown number is a
/// [`MapError`].
macro_rules! wire_enum {
    ($($rp:ident)::+ => $($pp:ident)::+ { $($rv:ident => $pv:ident),* $(,)? }) => {
        impl $crate::map::Wire for $($rp)::+ {
            type Pb = ::buffa::EnumValue<$($pp)::+>;
            fn to_pb(&self) -> Self::Pb {
                type P = $($pp)::+;
                ::buffa::EnumValue::Known(match self {
                    $(Self::$rv => P::$pv,)*
                })
            }
            fn from_pb(pb: Self::Pb) -> Result<Self, $crate::map::MapError> {
                type P = $($pp)::+;
                match pb {
                    $(::buffa::EnumValue::Known(P::$pv) => Ok(Self::$rv),)*
                    #[allow(unreachable_patterns)]
                    other => Err($crate::map::MapError::enum_value(
                        stringify!($($pp)::+),
                        other.to_i32(),
                    )),
                }
            }
        }
    };
}

/// The conversion of one field, by mode: `Wire` (no mode), `msg`, `opt_msg`
/// or `map`.
macro_rules! field_to {
    (; $value:expr) => {
        $crate::map::Wire::to_pb($value)
    };
    (msg; $value:expr) => {
        $crate::map::msg($value)
    };
    (opt_msg; $value:expr) => {
        $crate::map::opt_msg($value)
    };
    (default_msg; $value:expr) => {
        $crate::map::msg($value)
    };
    (map; $value:expr) => {
        $crate::map::map_to($value)
    };
    (opt_string; $value:expr) => {
        $value.clone().unwrap_or_default()
    };
}

macro_rules! field_from {
    (; $value:expr, $name:expr) => {
        $crate::map::Wire::from_pb($value)
    };
    (msg; $value:expr, $name:expr) => {
        $crate::map::from_msg($value, $name)
    };
    (opt_msg; $value:expr, $name:expr) => {
        $crate::map::from_opt_msg($value)
    };
    (default_msg; $value:expr, $name:expr) => {
        $crate::map::from_default_msg($value)
    };
    (map; $value:expr, $name:expr) => {
        $crate::map::map_from($value)
    };
    // An optional Rust string as a plain proto `string`: empty is `None`.
    (opt_string; $value:expr, $name:expr) => {
        Ok::<_, $crate::map::MapError>(Some($value).filter(|v: &String| !v.is_empty()))
    };
}

/// `impl Wire` for a Rust struct and its message, field by field:
/// `rust_field => proto_field` with an optional `as msg | opt_msg | map`.
/// The Rust side is destructured without `..`, so every field is named.
macro_rules! wire_struct {
    ($($rp:ident)::+ => $($pp:ident)::+ { $($rf:ident => $pf:ident $(as $conv:ident)?),* $(,)? }) => {
        impl $crate::map::Wire for $($rp)::+ {
            type Pb = $($pp)::+;
            #[allow(clippy::needless_update)]
            fn to_pb(&self) -> Self::Pb {
                let $($rp)::+ { $($rf),* } = self;
                $($pp)::+ {
                    $($pf: field_to!($($conv)?; $rf),)*
                    ..Default::default()
                }
            }
            fn from_pb(pb: Self::Pb) -> Result<Self, $crate::map::MapError> {
                Ok($($rp)::+ {
                    $($rf: field_from!($($conv)?; pb.$pf, stringify!($pf))?,)*
                })
            }
        }
    };
}

mod approvals;
mod attachments;
mod checkpoints;
mod diff;
mod files;
mod git;
mod groups;
mod harness;
pub mod marketplace;
mod notify;
mod projects;
mod queue;
mod review;
mod studio;
mod terminals;
mod threads;
