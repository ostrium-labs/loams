//! Argument validators (LV1 plan Task 4): the descriptors `loams:server`'s
//! `v` builds, read into [`Validator`]s when a bundle loads, and the check
//! of a call's arguments before its handler runs.
//!
//! A descriptor is a frozen object `{ kind, … }`: `null`, `int64`,
//! `float64`, `boolean`, `string`, `bytes` and `any` have nothing else;
//! `array` has `element`, `object` has `fields` (name → descriptor),
//! `literal` has `value`, `union` has `members`, `optional` has `inner` and
//! `id` has `table`. `optional` marks an object field that may be missing,
//! so it is refused anywhere else.

use std::collections::BTreeMap;

use loams_live::validate::{self, Validator};
use loams_live::{LiveError, LiveTxn, LiveValue};

/// The deepest descriptor read: deeper ones are refused (the value
/// conversion already stops at 64 levels).
const MAX_DEPTH: usize = 64;

/// The validator a descriptor describes; the error names what is wrong.
pub(crate) fn parse(descriptor: &LiveValue) -> Result<Validator, String> {
    parse_at(descriptor, false, 0)
}

fn parse_at(d: &LiveValue, field: bool, depth: usize) -> Result<Validator, String> {
    if depth > MAX_DEPTH {
        return Err(format!("a validator nested deeper than {MAX_DEPTH}"));
    }
    let LiveValue::Object(d) = d else {
        return Err(format!(
            "a validator descriptor is an object, not {}",
            d.type_name()
        ));
    };
    let kind = match d.get("kind") {
        Some(LiveValue::Str(kind)) => kind.as_str(),
        _ => return Err("a validator descriptor has no kind".into()),
    };
    let part = |name: &str| {
        d.get(name)
            .ok_or_else(|| format!("a v.{kind}() descriptor has no {name}"))
    };
    let inner = |name: &str| parse_at(part(name)?, false, depth + 1);
    Ok(match kind {
        "null" => Validator::Null,
        "int64" => Validator::Int64,
        "float64" => Validator::Float64,
        "boolean" => Validator::Boolean,
        "string" => Validator::String,
        "bytes" => Validator::Bytes,
        "any" => Validator::Any,
        "array" => Validator::Array(Box::new(inner("element")?)),
        "optional" if field => Validator::Optional(Box::new(inner("inner")?)),
        "optional" => {
            return Err(
                "v.optional() marks an object field that may be missing; use it only as a \
                 field of v.object() or args"
                    .into(),
            );
        }
        "object" => {
            let LiveValue::Object(fields) = part("fields")? else {
                return Err("v.object() fields are an object".into());
            };
            Validator::Object(
                fields
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), parse_at(v, true, depth + 1)?)))
                    .collect::<Result<BTreeMap<_, _>, String>>()?,
            )
        }
        "union" => {
            let LiveValue::Array(members) = part("members")? else {
                return Err("v.union() members are an array".into());
            };
            if members.is_empty() {
                return Err("v.union() needs at least one member".into());
            }
            Validator::Union(
                members
                    .iter()
                    .map(|m| parse_at(m, false, depth + 1))
                    .collect::<Result<_, _>>()?,
            )
        }
        "literal" => match part("value")? {
            v @ (LiveValue::Null
            | LiveValue::Bool(_)
            | LiveValue::I64(_)
            | LiveValue::F64(_)
            | LiveValue::Str(_)) => Validator::Literal(v.clone()),
            other => {
                return Err(format!(
                    "v.literal() takes a scalar, not {}",
                    other.type_name()
                ));
            }
        },
        "id" => match part("table")? {
            LiveValue::Str(table) if !table.is_empty() => Validator::Id(table.clone()),
            _ => return Err("v.id(table) needs a table name".into()),
        },
        other => return Err(format!("no validator v.{other}()")),
    })
}

/// Checks a call's arguments against the function's validator, before its
/// handler runs: `INVALID_ARGUMENT` naming each violation's path. The
/// tables an `id(table)` names are looked up in the call's transaction (a
/// missing one joins the read set, so creating it reruns a query).
pub(crate) async fn check_args(
    txn: &mut LiveTxn<'_>,
    path: &str,
    validator: &Validator,
    args: &LiveValue,
) -> Result<(), LiveError> {
    let mut tables = BTreeMap::new();
    for name in validator.id_tables() {
        if let Some(table) = txn.table(&name).await? {
            tables.insert(name, table.id);
        }
    }
    validate::check_ids(validator, args, &tables).map_err(|violations| {
        LiveError::InvalidArgument(format!(
            "{path}: the arguments do not match its validator: {}",
            validate::describe(&violations)
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(pairs: &[(&str, LiveValue)]) -> LiveValue {
        LiveValue::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn kind(k: &str) -> LiveValue {
        d(&[("kind", LiveValue::Str(k.into()))])
    }

    #[test]
    fn descriptors_parse_and_optional_is_for_fields_only() {
        let field = d(&[
            ("kind", LiveValue::Str("optional".into())),
            ("inner", kind("string")),
        ]);
        let object = d(&[
            ("kind", LiveValue::Str("object".into())),
            ("fields", d(&[("a", field.clone())])),
        ]);
        assert_eq!(
            parse(&object),
            Ok(Validator::Object(BTreeMap::from([(
                "a".to_string(),
                Validator::Optional(Box::new(Validator::String))
            )])))
        );
        let array = d(&[("kind", LiveValue::Str("array".into())), ("element", field)]);
        assert!(parse(&array).is_err_and(|e| e.contains("v.optional")));
        assert!(parse(&kind("date")).is_err_and(|e| e.contains("v.date")));
        assert!(parse(&LiveValue::Null).is_err());
    }
}
