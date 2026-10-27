//! The rest of Task 33's functions: `toTypeName` (through `ChType`'s
//! `Display`), `timezone`, `serverTimezone`, `materialize`, `identity` and
//! `ignore`. `version()` is the engine's (`registry.rs`); `currentDatabase`
//! is bound by `loams-house-sql`'s context, which knows the database.

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, StringArray, UInt8Array};
use loams_house_sql::Registry;
use loams_house_types::ChType;

use crate::func::typed::{ChFn, Nulls, SESSION_TIMEZONE, arity, kernel, typer};

fn strings(value: &str, rows: usize) -> ArrayRef {
    Arc::new(StringArray::from(vec![value; rows]))
}

/// Registers the functions.
pub fn register(r: &mut Registry) {
    ChFn::new(
        "toTypeName",
        Nulls::Own,
        typer(|s| {
            arity(s, 1, 1)?;
            Ok(ChType::String)
        }),
        kernel(|c| Ok(strings(&c.types[0].to_string(), c.rows))),
    )
    .plain()
    .register(r);

    for name in ["timezone", "serverTimezone"] {
        ChFn::new(
            name,
            Nulls::Own,
            typer(|s| {
                arity(s, 0, 0)?;
                Ok(ChType::String)
            }),
            kernel(|c| Ok(strings(SESSION_TIMEZONE, c.rows))),
        )
        .plain()
        .register(r);
    }

    for name in ["materialize", "identity"] {
        ChFn::new(
            name,
            Nulls::Own,
            typer(|s| {
                arity(s, 1, 1)?;
                Ok(s.types[0].clone())
            }),
            kernel(|c| Ok(Arc::clone(&c.args[0]))),
        )
        .plain()
        .register(r);
    }

    ChFn::new(
        "ignore",
        Nulls::Own,
        typer(|_| Ok(ChType::UInt8)),
        kernel(|c| Ok(Arc::new(UInt8Array::from(vec![0u8; c.rows])) as ArrayRef)),
    )
    .plain()
    .register(r);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_functions_register() {
        let mut r = Registry::default();
        register(&mut r);
        for n in [
            "toTypeName",
            "timezone",
            "serverTimezone",
            "materialize",
            "identity",
            "ignore",
        ] {
            assert!(r.scalars.contains_key(n), "{n}");
        }
        assert_eq!(strings("UTC", 2).len(), 2);
    }
}
