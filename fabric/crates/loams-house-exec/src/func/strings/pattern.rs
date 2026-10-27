//! `like`, `notLike`, `ilike`, `notILike` (and the `LIKE`/`ILIKE` operators,
//! which the guard rewrites to them) and `match`
//! (`src/Functions/MatchImpl.h`): each answers `UInt8`.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, UInt8Builder};
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};
use regex::bytes::Regex;

use crate::func::chudf::{ChFunction, check_arity, want_string};
use crate::func::strings::re2;

#[derive(Clone, Copy)]
enum Kind {
    Like { case_insensitive: bool },
    Match,
}

fn compile(kind: Kind, pattern: &str) -> Result<Regex, ChError> {
    match kind {
        Kind::Like { case_insensitive } => {
            let text = re2::like_regex(pattern, case_insensitive)?;
            Regex::new(&text).map_err(|e| re2::cannot_compile(pattern, &e.to_string()))
        }
        Kind::Match => re2::compile(pattern, false),
    }
}

fn matcher(name: &'static str, kind: Kind, negated: bool) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 2, 2)?;
            want_string(name, args, 0)?;
            want_string(name, args, 1)?;
            Ok(ChType::UInt8)
        }),
        Arc::new(move |call| {
            let h = call.strs(0)?;
            let p = call.strs(1)?;
            let mut compiled: HashMap<&str, Regex> = HashMap::new();
            let mut out = UInt8Builder::with_capacity(call.rows);
            for row in 0..call.rows {
                let (Some(hay), Some(pattern)) = (h.get(row), p.get(row)) else {
                    out.append_null();
                    continue;
                };
                if !compiled.contains_key(pattern) {
                    compiled.insert(pattern, compile(kind, pattern)?);
                }
                let hit = compiled[pattern].is_match(hay.as_bytes());
                out.append_value(u8::from(hit != negated));
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

pub(super) fn register(r: &mut Registry) {
    let like = Kind::Like {
        case_insensitive: false,
    };
    let ilike = Kind::Like {
        case_insensitive: true,
    };
    matcher("like", like, false).register(r);
    matcher("notLike", like, true).register(r);
    matcher("ilike", ilike, false).register(r);
    matcher("notILike", ilike, true).register(r);
    matcher("match", Kind::Match, false).register(r);
}
