//! `GET /_cat/indices[/{index}]` (plan MA1 Task 24, design §66 §6): one row
//! per index, as JSON (`format=json`, an array of objects with string
//! values) or ES's aligned text table (the default; `v` adds the header).
//! `h` picks columns (names, aliases or `*` patterns), `s` sorts, `bytes`
//! fixes the size unit and `health` filters. `pri` and `rep` are fixed,
//! health is always green and sizes are Loams's `size_bytes`.

use std::cmp::Ordering;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value};

use crate::EsGateway;
use crate::error::{ErrorContext, EsError};
use crate::http::{Params, RequestCtx, fail, respond};
use crate::mapping::index_uuid;
use crate::names::{IndexExpr, ResolveOptions, glob, resolve};

/// The parameters `_cat/indices` accepts. `local`, `master_timeout`,
/// `pri`, `time` and `include_unloaded_segments` change nothing here.
const CAT_PARAMS: &[&str] = &[
    "format",
    "h",
    "v",
    "s",
    "bytes",
    "health",
    "expand_wildcards",
    "local",
    "master_timeout",
    "pri",
    "time",
    "include_unloaded_segments",
];

/// How a column's cells compare and align.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Bytes,
}

/// A column of the table: its name, its aliases and its kind.
struct Column {
    name: &'static str,
    aliases: &'static [&'static str],
    kind: Kind,
}

/// Every column, in ES 8.19's default order.
const COLUMNS: &[Column] = &[
    Column {
        name: "health",
        aliases: &["h"],
        kind: Kind::Text,
    },
    Column {
        name: "status",
        aliases: &["s"],
        kind: Kind::Text,
    },
    Column {
        name: "index",
        aliases: &["i", "idx"],
        kind: Kind::Text,
    },
    Column {
        name: "uuid",
        aliases: &["id"],
        kind: Kind::Text,
    },
    Column {
        name: "pri",
        aliases: &["p", "shards.primary", "shardsPrimary"],
        kind: Kind::Number,
    },
    Column {
        name: "rep",
        aliases: &["r", "shards.replica", "shardsReplica"],
        kind: Kind::Number,
    },
    Column {
        name: "docs.count",
        aliases: &["dc", "docsCount"],
        kind: Kind::Number,
    },
    Column {
        name: "docs.deleted",
        aliases: &["dd", "docsDeleted"],
        kind: Kind::Number,
    },
    Column {
        name: "store.size",
        aliases: &["ss", "storeSize"],
        kind: Kind::Bytes,
    },
    Column {
        name: "pri.store.size",
        aliases: &[],
        kind: Kind::Bytes,
    },
    Column {
        name: "dataset.size",
        aliases: &[],
        kind: Kind::Bytes,
    },
];

/// One index's values, by column position in [`COLUMNS`].
struct Row {
    index: String,
    uuid: String,
    docs: u64,
    size: u64,
}

impl Row {
    /// The cell of column `i`: its sort key and its text under `bytes`.
    fn cell(&self, i: usize, bytes: Option<u64>) -> (CellKey, String) {
        let text = |s: &str| (CellKey::Text(s.to_string()), s.to_string());
        let number = |n: u64| (CellKey::Number(n), n.to_string());
        match COLUMNS[i].name {
            "health" => text("green"),
            "status" => text("open"),
            "index" => text(&self.index),
            "uuid" => text(&self.uuid),
            "pri" => number(1),
            "rep" | "docs.deleted" => number(0),
            "docs.count" => number(self.docs),
            _ => (CellKey::Number(self.size), size_text(self.size, bytes)),
        }
    }
}

/// A cell's sort key: numbers and sizes compare by value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum CellKey {
    Number(u64),
    Text(String),
}

/// A selected column: its position and the name the answer shows.
struct Display {
    column: usize,
    name: String,
}

/// The size units of `bytes=` and of the human form, largest first.
const UNITS: [(&str, u64); 6] = [
    ("pb", 1 << 50),
    ("tb", 1 << 40),
    ("gb", 1 << 30),
    ("mb", 1 << 20),
    ("kb", 1 << 10),
    ("b", 1),
];

/// `bytes` with a `bytes=` unit (the whole number of units, truncated), or
/// without one in ES's human form: the largest unit it reaches, with one
/// decimal truncated and a trailing `.0` dropped (`1.5kb`, `225b`).
pub fn size_text(bytes: u64, unit: Option<u64>) -> String {
    if let Some(unit) = unit {
        return (bytes / unit).to_string();
    }
    let (suffix, unit) = UNITS
        .iter()
        .copied()
        .find(|(_, unit)| bytes >= *unit)
        .unwrap_or(("b", 1));
    if unit == 1 {
        return format!("{bytes}b");
    }
    let tenths = u128::from(bytes) * 10 / u128::from(unit);
    if tenths % 10 == 0 {
        format!("{}{suffix}", tenths / 10)
    } else {
        format!("{}.{}{suffix}", tenths / 10, tenths % 10)
    }
}

/// The column `name` names (by name or alias).
fn column_named(name: &str) -> Option<usize> {
    COLUMNS
        .iter()
        .position(|c| c.name == name || c.aliases.contains(&name))
}

/// The columns `h` selects: names and aliases keep the spelling given;
/// a `*` pattern adds every column whose name matches it; unknown names
/// are left out. Without `h`, every column.
fn displays(h: Option<Vec<String>>) -> Vec<Display> {
    let Some(items) = h else {
        return COLUMNS
            .iter()
            .enumerate()
            .map(|(column, c)| Display {
                column,
                name: c.name.to_string(),
            })
            .collect();
    };
    let mut out = Vec::new();
    for item in items {
        if item.contains('*') {
            for (column, c) in COLUMNS.iter().enumerate() {
                if glob(&item, c.name) || c.aliases.iter().any(|a| glob(&item, a)) {
                    out.push(Display {
                        column,
                        name: c.name.to_string(),
                    });
                }
            }
        } else if let Some(column) = column_named(&item) {
            out.push(Display { column, name: item });
        }
    }
    out
}

/// The `s` keys: a column and whether it sorts descending.
fn sort_keys(s: Option<Vec<String>>) -> Result<Vec<(usize, bool)>, EsError> {
    s.unwrap_or_default()
        .iter()
        .map(|item| {
            let (name, order) = match item.split_once(':') {
                Some((name, order)) => (name, order),
                None => (item.as_str(), "asc"),
            };
            let desc = match order {
                "asc" => false,
                "desc" => true,
                other => {
                    return Err(EsError::illegal_argument(format!(
                        "Unsupported sort order [{other}] for sort key [{name}]; use asc or desc"
                    )));
                }
            };
            column_named(name)
                .map(|column| (column, desc))
                .ok_or_else(|| {
                    EsError::illegal_argument(format!(
                        "Unable to sort by unknown sort key `{name}`"
                    ))
                })
        })
        .collect()
}

/// The `bytes` unit.
fn bytes_unit(params: &Params) -> Result<Option<u64>, EsError> {
    let Some(value) = params.str("bytes") else {
        return Ok(None);
    };
    UNITS
        .iter()
        .find(|(suffix, _)| *suffix == value)
        .map(|(_, unit)| Some(*unit))
        .ok_or_else(|| {
            EsError::illegal_argument(format!(
                "failed to parse setting [bytes] with value [{value}] as a size in bytes: unit is missing or unrecognized"
            ))
        })
}

/// Whether the answer is JSON: `format=json`, or no `format` and an
/// `Accept` whose first type is JSON; `text`/`txt` or anything else is
/// the table. Other formats are refused.
fn wants_json(params: &Params, headers: &HeaderMap) -> Result<bool, EsError> {
    match params.str("format") {
        Some("json") => Ok(true),
        Some("text" | "txt") => Ok(false),
        Some(other) => Err(EsError::illegal_argument(format!(
            "Loams supports only format json or text in [_cat/indices], not [{other}]"
        ))),
        None => Ok(headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .is_some_and(|first| first.split(';').next().unwrap_or("").contains("json"))),
    }
}

/// `GET /_cat/indices`.
pub(crate) async fn cat_indices_all(
    State(gw): State<EsGateway>,
    ctx: RequestCtx,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    cat_indices(gw, ctx, headers, uri, IndexExpr::All).await
}

/// `GET /_cat/indices/{index}`.
pub(crate) async fn cat_indices_named(
    State(gw): State<EsGateway>,
    ctx: RequestCtx,
    headers: HeaderMap,
    uri: Uri,
    Path(index): Path<String>,
) -> Response {
    let expr = IndexExpr::parse(&index);
    cat_indices(gw, ctx, headers, uri, expr).await
}

async fn cat_indices(
    gw: EsGateway,
    ctx: RequestCtx,
    headers: HeaderMap,
    uri: Uri,
    expr: IndexExpr,
) -> Response {
    match table(&gw, &ctx, &headers, &uri, expr).await {
        Ok(Answer::Json(body)) => respond(&ctx, 200, &body),
        Ok(Answer::Text(text)) => {
            let mut response = (StatusCode::OK, text).into_response();
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=UTF-8"),
            );
            response
        }
        Err(err) => fail(&ctx, &err),
    }
}

enum Answer {
    Json(Value),
    Text(String),
}

async fn table(
    gw: &EsGateway,
    ctx: &RequestCtx,
    headers: &HeaderMap,
    uri: &Uri,
    expr: IndexExpr,
) -> Result<Answer, EsError> {
    let params = Params::parse(uri.query(), uri.path(), CAT_PARAMS)?;
    let json = wants_json(&params, headers)?;
    let unit = bytes_unit(&params)?;
    let header_row = params.bool("v")?.unwrap_or(false);
    let shown = displays(params.list("h"));
    let sorts = sort_keys(params.list("s"))?;
    if let Some(values) = params.list("expand_wildcards") {
        for value in &values {
            if !matches!(
                value.as_str(),
                "all" | "open" | "closed" | "hidden" | "none"
            ) {
                return Err(EsError::illegal_argument(format!(
                    "No enum constant org.elasticsearch.action.support.IndicesOptions.WildcardStates.{}",
                    value.to_ascii_uppercase()
                )));
            }
        }
    }
    // Every Loams index is green.
    let green = match params.str("health") {
        None => true,
        Some(h) => match h.to_ascii_lowercase().as_str() {
            "green" => true,
            "yellow" | "red" => false,
            _ => {
                return Err(EsError::illegal_argument(format!(
                    "unknown health status [{h}]"
                )));
            }
        },
    };

    let service = gw.service();
    let ns = &ctx.namespace;
    let resolved = resolve(service, ns, &expr, ResolveOptions::default()).await?;
    let mut rows = Vec::with_capacity(resolved.len());
    if green {
        for r in resolved {
            let info = service
                .get_collection(ns, &r.name)
                .await
                .map_err(|e| EsError::from_service(e, ErrorContext::Read))?;
            let docs = service
                .count(ns, &r.name, None, ctx.consistency.clone())
                .await
                .map_err(|e| EsError::from_service(e, ErrorContext::Read))?;
            rows.push(Row {
                index: r.name,
                uuid: index_uuid(info.id),
                docs,
                size: info.size_bytes,
            });
        }
    }
    let mut cells: Vec<Vec<(CellKey, String)>> = rows
        .iter()
        .map(|row| (0..COLUMNS.len()).map(|i| row.cell(i, unit)).collect())
        .collect();
    cells.sort_by(|a, b| {
        sorts
            .iter()
            .map(|(column, desc)| {
                let order = a[*column].0.cmp(&b[*column].0);
                if *desc { order.reverse() } else { order }
            })
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    });

    if json {
        let out = cells
            .iter()
            .map(|row| {
                let mut object = Map::new();
                for d in &shown {
                    object.insert(d.name.clone(), Value::String(row[d.column].1.clone()));
                }
                Value::Object(object)
            })
            .collect();
        return Ok(Answer::Json(Value::Array(out)));
    }
    let mut lines: Vec<Vec<String>> = Vec::new();
    if header_row {
        lines.push(shown.iter().map(|d| d.name.clone()).collect());
    }
    for row in &cells {
        lines.push(shown.iter().map(|d| row[d.column].1.clone()).collect());
    }
    Ok(Answer::Text(render_text(&shown, &lines)))
}

/// The aligned table: each column as wide as its widest cell, numbers and
/// sizes right-aligned, one space between columns.
fn render_text(shown: &[Display], lines: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..shown.len())
        .map(|i| {
            lines
                .iter()
                .map(|l| l[i].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = String::new();
    for line in lines {
        let cells: Vec<String> = line
            .iter()
            .zip(shown)
            .zip(&widths)
            .map(|((cell, d), width)| {
                if COLUMNS[d.column].kind == Kind::Text {
                    format!("{cell:<width$}")
                } else {
                    format!("{cell:>width$}")
                }
            })
            .collect();
        out.push_str(cells.join(" ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_sizes_truncate_to_one_decimal() {
        assert_eq!(size_text(0, None), "0b");
        assert_eq!(size_text(225, None), "225b");
        assert_eq!(size_text(1024, None), "1kb");
        assert_eq!(size_text(1536, None), "1.5kb");
        assert_eq!(size_text(1023 + 1024, None), "1.9kb");
        assert_eq!(size_text(5 << 20, None), "5mb");
        assert_eq!(size_text(3 << 30, None), "3gb");
    }

    #[test]
    fn bytes_units_truncate() {
        assert_eq!(size_text(1536, Some(1)), "1536");
        assert_eq!(size_text(1536, Some(1 << 10)), "1");
        assert_eq!(size_text(1536, Some(1 << 20)), "0");
    }

    #[test]
    fn h_keeps_aliases_and_expands_patterns() {
        let names = |h: &[&str]| -> Vec<String> {
            displays(Some(h.iter().map(|s| s.to_string()).collect()))
                .into_iter()
                .map(|d| d.name)
                .collect()
        };
        assert_eq!(names(&["i", "dc", "nope"]), ["i", "dc"]);
        assert_eq!(names(&["docs.*"]), ["docs.count", "docs.deleted"]);
        assert_eq!(displays(None).len(), COLUMNS.len());
    }

    #[test]
    fn unknown_sort_keys_are_refused() {
        let err = sort_keys(Some(vec!["nope".into()])).expect_err("refused");
        assert_eq!(err.reason, "Unable to sort by unknown sort key `nope`");
        assert_eq!(
            sort_keys(Some(vec!["docs.count:desc".into(), "i".into()])).expect("keys"),
            [(6, true), (2, false)]
        );
    }
}
