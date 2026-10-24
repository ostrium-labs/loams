//! Query parameters (plan HE1 Task 22, step 2): `{name:Type}` bound as
//! ClickHouse binds them, `_CAST(<value>, 'Type')` in place of the
//! placeholder (HE0 R2.8: `query_log` shows the bound text), so the column
//! name is ClickHouse's too. `{name:Identifier}` becomes the quoted name.
//!
//! The placeholder is found outside string literals, quoted names and
//! comments; an unbound parameter is `456 UNKNOWN_QUERY_PARAMETER`.

use loams_house_types::ty::{quote, quote_identifier};
use loams_house_types::{ChError, ChType};

/// `456 UNKNOWN_QUERY_PARAMETER`.
fn unknown(name: &str) -> ChError {
    ChError {
        code: 456,
        name: "UNKNOWN_QUERY_PARAMETER",
        message: format!(
            "Substitution `{name}` is not set. You can specify it with the `param_{name}` setting"
        ),
    }
}

/// `sql` with every `{name:Type}` bound from `params` (`param_<name>` values,
/// as the client sent them).
pub fn bind(sql: &str, params: &[(String, String)]) -> Result<String, ChError> {
    let b = sql.as_bytes();
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"' | b'`') => {
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == q {
                        if b.get(i + 1) == Some(&q) {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'{' => {
                if let Some((end, name, ty)) = placeholder(sql, i) {
                    out.push_str(&sql[copied..i]);
                    let value = params
                        .iter()
                        .rev()
                        .find(|(n, _)| *n == name)
                        .map(|(_, v)| v.as_str())
                        .ok_or_else(|| unknown(&name))?;
                    out.push_str(&substitute(&name, &ty, value)?);
                    i = end;
                    copied = end;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.push_str(&sql[copied.min(sql.len())..]);
    Ok(out)
}

/// `{name:Type}` at `start`: its end, name and type text.
fn placeholder(sql: &str, start: usize) -> Option<(usize, String, String)> {
    let rest = &sql[start + 1..];
    let colon = rest.find(':')?;
    let name = rest[..colon].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut depth = 0i32;
    for (k, c) in rest[colon + 1..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '}' if depth == 0 => {
                let ty = rest[colon + 1..colon + 1 + k].trim().to_string();
                return (!ty.is_empty())
                    .then(|| (start + 1 + colon + 1 + k + 1, name.to_string(), ty));
            }
            _ => {}
        }
    }
    None
}

fn substitute(name: &str, ty: &str, value: &str) -> Result<String, ChError> {
    if ty == "Identifier" {
        return Ok(quote_identifier(value));
    }
    let t: ChType = loams_house_types::parse(ty)?;
    let literal = if (t.is_nullable() && (value == "\\N" || value.eq_ignore_ascii_case("null")))
        || value.is_empty() && t.is_nullable()
    {
        "NULL".to_string()
    } else {
        match t.base() {
            ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Float32
            | ChType::Float64
            | ChType::Decimal { .. } => {
                let ok = value.parse::<f64>().is_ok() || value.parse::<i128>().is_ok();
                if !ok {
                    return Err(ChError::cannot_parse_text(format!(
                        "Value {value} cannot be parsed as {ty} for query parameter '{name}'"
                    )));
                }
                value.to_string()
            }
            ChType::Bool => value.to_string(),
            // Composite values are written as ClickHouse literals by the client.
            ChType::Array(_) | ChType::Map(..) | ChType::Tuple(_) => value.to_string(),
            _ => quote(value),
        }
    };
    Ok(format!(
        "_CAST({literal}, {})",
        quote(&t.to_compact_string())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn parameters_bind_as_clickhouse_binds_them() {
        let sql = bind(
            "SELECT {p:UInt8}, {s:String} FROM {t:Identifier} WHERE x = '{p:UInt8}'",
            &p(&[("p", "7"), ("s", "it's"), ("t", "tbl")]),
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT _CAST(7, 'UInt8'), _CAST('it\\'s', 'String') FROM tbl WHERE x = '{p:UInt8}'"
        );
        let sql = bind(
            "SELECT {d:DateTime64(3, 'UTC')}",
            &p(&[("d", "2026-01-01 00:00:00")]),
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT _CAST('2026-01-01 00:00:00', 'DateTime64(3, \\'UTC\\')')"
        );
        let sql = bind("SELECT {n:Nullable(String)}", &p(&[("n", "\\N")])).unwrap();
        assert_eq!(sql, "SELECT _CAST(NULL, 'Nullable(String)')");
    }

    #[test]
    fn unbound_parameters_are_456() {
        assert_eq!(bind("SELECT {x:UInt8}", &[]).unwrap_err().code, 456);
        assert_eq!(
            bind("SELECT {x:UInt8}", &p(&[("x", "abc")]))
                .unwrap_err()
                .code,
            6
        );
    }

    #[test]
    fn braces_that_are_not_parameters_stay() {
        assert_eq!(
            bind("SELECT map('a', 1) -- {x:UInt8}\n", &[]).unwrap(),
            "SELECT map('a', 1) -- {x:UInt8}\n"
        );
    }
}
