//! The read views of §49 §7 (HS1 Task 11): the DDL a worker's control
//! connection runs so that a lake table's name reads its pinned snapshot.
//!
//! A table `d.t` becomes a view in a `Memory` database `d` of the worker:
//!
//! ```text
//! CREATE VIEW `d`.`t` AS
//! SELECT CAST(`a`, 'UInt64') AS `a`, …
//! FROM icebergS3('http://127.0.0.1:9180/<bucket>/<table location>/', 'dummy', 'dummy',
//!                SETTINGS iceberg_metadata_file_path = 'metadata/<pinned>.metadata.json')
//! SETTINGS iceberg_snapshot_id = <pinned>
//! ```
//!
//! * The bucket is read through `house-cache` only: the worker's endpoint is its
//!   forwarder (`Ready::s3_endpoint`), and the keys are dummies that never reach
//!   the store (§49 §13.1, D766).
//! * The metadata file **and** the snapshot are pinned, so a commit that lands
//!   while the query runs is never seen and nothing is listed (R1.2). Iceberg
//!   time travel is the same view with an older snapshot: `iceberg_snapshot_id`
//!   is not a setting of the table function in chDB 26.9, but a view's own
//!   `SETTINGS` apply to its read alone (measured: two views of one table at two
//!   snapshots in one query each read their own).
//! * ClickHouse types are restored with `CAST`s (Iceberg has no `UInt8`, `Enum`,
//!   `LowCardinality` …, R10.4), and a column added by `ALTER` (an optional
//!   Iceberg field) reads its `DEFAULT` for rows written before it.
//! * `FINAL` (or `final = 1`) on a `ReplacingMergeTree` or `SummingMergeTree`
//!   table reads a **merge view** (§49 §6.2): Replacing keeps, per sorting key,
//!   the row with the greatest `(ver, file sequence number, row number)` and
//!   drops it when its `is_deleted` is 1; Summing groups by the key and sums;
//!   VersionedCollapsing cancels rows of opposite sign per key and version
//!   (D1570, [`collapsing`]).
//!   chDB ignores `FINAL` on a view (measured), so the user's text runs as it is.
//! * An external table (another writer's) reads as chDB reads Iceberg: no casts.
//! * A view that reads the bucket names its columns and is `SQL SECURITY NONE`
//!   (HS1 R-ACL.1): the worker's `default` holds no grant on any source, so the
//!   view, which only the control connection can create, is the one way a
//!   statement reaches the forwarder. Naming the columns is what lets the
//!   control connection create it without a grant (chDB analyses a view's
//!   `SELECT` at creation only when the columns are not given, measured).

use loams_house_lake::files::LiveFile;

use crate::catalog::LakeRoot;
use crate::ddl::{self, LakeEngine, TableClass, quote_name, quote_string};
use crate::errors::{ChError, HouseError};
use crate::expr;
use crate::read::ReadPin;
use crate::types::ChType;

/// `CREATE DATABASE IF NOT EXISTS d ENGINE = Memory`: where a database's views
/// live in the worker.
pub fn database_ddl(database: &str) -> String {
    format!(
        "CREATE DATABASE IF NOT EXISTS {} ENGINE = Memory",
        quote_name(database)
    )
}

/// `DROP TABLE IF EXISTS d.t`: a name the catalog does not have must not read
/// a view an earlier statement left in a worker that shares its engine.
pub fn drop_ddl(database: &str, name: &str) -> String {
    format!(
        "DROP TABLE IF EXISTS {}.{}",
        quote_name(database),
        quote_name(name)
    )
}

/// A catalog view (HE1 R9.13) in the worker, over the read views of what it
/// names: `CREATE OR REPLACE VIEW d.v AS\n<select>`, the control connection's
/// one shape for a view without a column list. It runs with the invoker's
/// rights, so it reads only what the statement could.
pub fn user_view_ddl(database: &str, name: &str, select: &str) -> String {
    format!(
        "CREATE OR REPLACE VIEW {}.{} AS\n{select}",
        quote_name(database),
        quote_name(name)
    )
}

/// The table function that reads a pin through the worker's endpoint.
pub fn source(pin: &ReadPin, endpoint: &str, root: &LakeRoot) -> Result<String, HouseError> {
    let unreadable = |why: &str| {
        HouseError::from(ChError::access_denied(format!(
            "Table {}.{} cannot be read: {why}",
            pin.table.database, pin.table.name
        )))
    };
    let key = root
        .relative(&pin.location)
        .ok_or_else(|| unreadable("its location is outside the House's store"))?;
    let location = pin.location.trim_end_matches('/');
    let metadata = pin
        .metadata_file
        .strip_prefix(location)
        .and_then(|rest| rest.strip_prefix('/'))
        .ok_or_else(|| unreadable("its metadata file is outside its location"))?;
    let mut url = endpoint.trim_end_matches('/').to_string();
    url.push('/');
    url.push_str(root.bucket());
    let key = key.trim_matches('/');
    if !key.is_empty() {
        url.push('/');
        url.push_str(key);
    }
    url.push('/');
    Ok(format!(
        "icebergS3({}, 'dummy', 'dummy', SETTINGS iceberg_metadata_file_path = {})",
        quote_string(&url),
        quote_string(metadata)
    ))
}

/// How a view reads its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode<'a> {
    /// The rows as stored.
    Plain,
    /// The rows one commit added, for the source's materialized views (HS1
    /// Task 17): as [`Mode::Plain`], and a `Null` table's too (HE1 R9.14).
    Batch,
    /// `FINAL`: the merge view of a Replacing or Summing table, with the live
    /// files of the pinned snapshot (for their sequence numbers).
    Final(&'a [LiveFile]),
    /// The stored rows as written, with each row's place in the table's write
    /// order (`__loams_seq`, its file's sequence number, and `__loams_row`, its
    /// number in its file): what a mutation rewrites (HE1 R9.5), so that rows of
    /// one key keep their order. No `ALIAS` column and no `TTL` filter: the
    /// rewrite keeps what is stored, and reads still compute both.
    Sequenced(&'a [LiveFile]),
}

/// The columns a [`Mode::Sequenced`] view adds.
pub const SEQUENCE_COLUMNS: [&str; 2] = [SEQ, ROW];

/// The view DDL for a pin.
pub fn view_ddl(pin: &ReadPin, source: &str, mode: Mode<'_>) -> Result<String, HouseError> {
    view_ddl_filtered(pin, source, mode, &[])
}

/// The table's `UInt64` columns stored as `decimal(20, 0)` that every row
/// holds (a required field): the columns [`crate::pushdown`] may filter on.
/// An optional field (a column `ALTER` added) reads its default for older
/// rows, which a filter on the stored column would drop, so it is left out.
pub fn uint64_columns(pin: &ReadPin) -> Vec<&str> {
    columns(pin)
        .into_iter()
        .filter(|c| c.ch_type == "UInt64")
        .filter(|c| {
            pin.fields
                .iter()
                .any(|f| f.name == c.name && f.required && !f.long)
        })
        .map(|c| c.name.as_str())
        .collect()
}

/// [`view_ddl`] with `filters` on the stored columns of a plain view's source
/// (aliased [`crate::pushdown::SOURCE_ALIAS`]), so chDB prunes row groups with
/// them (BQ1-house). They must be implied by the statement's own predicates
/// ([`crate::pushdown::key_filters`]); other modes ignore them.
pub fn view_ddl_filtered(
    pin: &ReadPin,
    source: &str,
    mode: Mode<'_>,
    filters: &[String],
) -> Result<String, HouseError> {
    let mut name = format!(
        "{}.{}",
        quote_name(&pin.table.database),
        quote_name(&pin.table.name)
    );
    let settings = pin
        .snapshot_id
        .filter(|_| pin.travel)
        .map(|id| format!("\nSETTINGS iceberg_snapshot_id = {id}"))
        .unwrap_or_default();
    let null_engine =
        mode != Mode::Batch && matches!(LakeEngine::parse(&pin.table.engine), Ok(LakeEngine::Null));
    let body = if pin.snapshot_id.is_none() || null_engine {
        // No snapshot (a new table, or a time before its first): no rows, with
        // the table's columns. A `Null` table reads none ever (HE1 R9.14): its
        // rows are kept only for its materialized views.
        empty(pin)?
    } else if pin.table.class == TableClass::External {
        format!("SELECT * FROM {source}")
    } else {
        let engine = LakeEngine::parse(&pin.table.engine).ok();
        match (mode, engine) {
            (Mode::Sequenced(files), _) => format!(
                "SELECT {}\nFROM {source}",
                sequenced(pin, files)?.join(", ")
            ),
            (Mode::Final(files), Some(engine @ LakeEngine::Replacing { .. })) => {
                replacing(pin, source, &engine, files)?
            }
            (Mode::Final(_), Some(engine @ LakeEngine::Summing { .. })) => {
                summing(pin, source, &engine)?
            }
            (Mode::Final(files), Some(engine @ LakeEngine::VersionedCollapsing { .. })) => {
                collapsing(pin, source, &engine, files)?
            }
            (Mode::Final(_), Some(LakeEngine::Aggregating)) => aggregating(pin, source)?,
            _ if !filters.is_empty() && matches!(mode, Mode::Plain) => format!(
                "SELECT {}\nFROM {source} AS {}\nWHERE {}",
                casts(pin)?.join(", "),
                crate::pushdown::SOURCE_ALIAS,
                filters.join(" AND ")
            ),
            _ => format!("SELECT {}\nFROM {source}", casts(pin)?.join(", ")),
        }
    };
    let sequence = matches!(mode, Mode::Sequenced(_))
        && pin.snapshot_id.is_some()
        && pin.table.class != TableClass::External;
    let body = if sequence { body } else { computed(pin, body)? };
    if pin.snapshot_id.is_some() {
        // It reads the bucket: R-ACL.1's form, columns named.
        let listed = if sequence {
            columns(pin)
        } else {
            view_columns(pin)
        };
        let mut columns: Vec<String> = listed
            .iter()
            .map(|c| Ok(format!("{} {}", quote_name(&c.name), column_type(pin, c)?)))
            .collect::<Result<_, HouseError>>()?;
        if sequence {
            columns.extend(SEQUENCE_COLUMNS.iter().map(|c| format!("{c} Int64")));
        }
        if columns.is_empty() {
            return Err(HouseError::from(ChError::bad_arguments(format!(
                "Table {}.{} has no columns at this snapshot",
                pin.table.database, pin.table.name
            ))));
        }
        name = format!("{name} ({}) SQL SECURITY NONE", columns.join(", "));
    }
    Ok(format!(
        "CREATE OR REPLACE VIEW {name} AS\n{body}{settings}"
    ))
}

/// A column's type as the column list of a view prints it, outside any quotes
/// (R-ACL.1). The text comes from the catalog's properties, which another writer
/// of a shared REST catalog can set, so it must be one ClickHouse type and
/// nothing a lexer could read differently: it parses as a type, and outside its
/// string literals (`Enum` names) it holds only names, numbers, `(`, `)`, `,`,
/// `=`, `-` (never `--`), `+`, `.` and spaces; inside them, no quote, backslash or line
/// break. Anything else is `36`, and the table is not read.
fn column_type(pin: &ReadPin, column: &ddl::ChColumn) -> Result<String, HouseError> {
    if let Some(text) = printable_type(&column.ch_type) {
        return Ok(text);
    }
    Err(HouseError::from(ChError::bad_arguments(format!(
        "Table {}.{} cannot be read: the type of column {} is not one the House \
         can name in a view",
        pin.table.database, pin.table.name, column.name
    ))))
}

/// `text` with its whitespace outside string literals made single spaces (the
/// canonical type of a named `Tuple` spans lines), if it is then a
/// [`safe_type`].
pub fn printable_type(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut in_literal = false;
    for ch in text.chars() {
        if !in_literal && ch.is_whitespace() {
            if !out.ends_with(' ') && !out.ends_with('(') {
                out.push(' ');
            }
            continue;
        }
        if !in_literal && ch == ')' && out.ends_with(' ') {
            out.pop();
        }
        if ch == '\'' {
            in_literal = !in_literal;
        }
        out.push(ch);
    }
    let out = out.trim().to_string();
    safe_type(&out).then_some(out)
}

/// Whether `text` is one ClickHouse type the House may print into a statement
/// it generates (R-ACL.1): it parses as a type, and outside its string
/// literals (`Enum` names) it holds only names, numbers, `(`, `)`, `,`, `=`,
/// `-` (never `--`), `+`, `.` and spaces; inside them, no quote, backslash or
/// line break; a back-quoted name holds letters, digits and `_` only. The read views' column lists and the write path's staging
/// table and casts (HS1 Task 12) hold catalog types to it.
pub fn safe_type(text: &str) -> bool {
    if ChType::parse(text).is_err() {
        return false;
    }
    // Inside `'…'` (an `Enum`'s names) or `` `…` `` (a `Tuple`'s element
    // names, which may be digits), outside them.
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for ch in text.chars() {
        let fine = match quote {
            Some('\'') => !matches!(ch, '\\' | '\n' | '\r'),
            Some(_) => ch.is_ascii_alphanumeric() || ch == '_' || ch == '`',
            // `--` would start a comment.
            None => {
                (ch.is_ascii_alphanumeric() || " _(),=-+.'`".contains(ch))
                    && !(ch == '-' && previous == '-')
            }
        };
        previous = ch;
        if !fine {
            return false;
        }
        if ch == '\'' || ch == '`' {
            quote = match quote {
                None => Some(ch),
                Some(open) if open == ch => None,
                other => other,
            };
        }
    }
    quote.is_none()
}

/// Whether a view of `pin` in `mode` carries expression text from the
/// catalog's properties — a column's `DEFAULT` (for rows written before an
/// `ALTER … ADD COLUMN`) or the sorting key (`FINAL`) — rather than only names,
/// types and literals the House quotes itself. Such a view is checked by the
/// deny list before a worker runs it (HS1 Task 11): another writer of a shared
/// REST catalog could set those properties.
pub fn carries_expressions(pin: &ReadPin, mode: Mode<'_>) -> bool {
    if pin.table.class == TableClass::External {
        return false;
    }
    let computes = !aliases(pin).is_empty() || !pin.table.ttl.trim().is_empty();
    if pin.snapshot_id.is_none() {
        return computes;
    }
    let merge = matches!(mode, Mode::Final(_))
        && LakeEngine::parse(&pin.table.engine).is_ok_and(|e| e.merges());
    merge
        || computes
        || columns(pin).iter().any(|c| {
            !c.default_expression.is_empty()
                && pin.fields.iter().any(|f| f.name == c.name && !f.required)
        })
}

/// The `SELECT` of a view DDL (what follows `AS`), for the deny list.
pub fn view_body(ddl: &str) -> &str {
    ddl.split_once(" AS\n").map_or(ddl, |(_, body)| body)
}

/// A table function the deny list allows and that a syntax-only analysis
/// never opens, standing in for a view's `icebergS3` source while its body is
/// checked.
pub const CHECKED_SOURCE: &str = "null('loams_checked UInt8')";

/// The `ALIAS` columns of a lake table (HE1 R9.12), which a view computes.
fn aliases(pin: &ReadPin) -> Vec<&ddl::ChColumn> {
    if pin.table.class == TableClass::External {
        return Vec::new();
    }
    pin.table.columns.iter().filter(|c| !c.stored()).collect()
}

/// The table `TTL`'s delete rules (HE1 R9.12).
fn ttl_rules(pin: &ReadPin) -> Result<Vec<ddl::TtlRule>, HouseError> {
    if pin.table.class == TableClass::External || pin.table.ttl.trim().is_empty() {
        return Ok(Vec::new());
    }
    ddl::parse_ttl(&pin.table.ttl).map_err(|err| catalog_refusal(pin, &err.to_string()))
}

/// The columns a view has, in the table's order: the stored columns the pin
/// reads, and the `ALIAS` columns.
fn view_columns(pin: &ReadPin) -> Vec<&ddl::ChColumn> {
    let stored = columns(pin);
    pin.table
        .columns
        .iter()
        .filter(|c| {
            stored.iter().any(|s| s.name == c.name)
                || (!c.stored() && pin.table.class != TableClass::External)
        })
        .collect()
}

/// The view's body with what a lake table computes at read time (HE1 R9.12):
/// its `ALIAS` columns, and its `TTL`, whose expired rows a read never sees
/// (ClickHouse deletes them at its next merge; until then they are read, so the
/// House answers as ClickHouse does after that merge). Each expression is
/// catalog text, printed again from its tree (R11.11).
fn computed(pin: &ReadPin, body: String) -> Result<String, HouseError> {
    let aliases = aliases(pin);
    let rules = ttl_rules(pin)?;
    if aliases.is_empty() && rules.is_empty() {
        return Ok(body);
    }
    let reprint = |text: &str| expr::reprint(text).map_err(|why| catalog_refusal(pin, &why));
    let stored: Vec<String> = if pin.snapshot_id.is_none() {
        pin.table
            .columns
            .iter()
            .filter(|c| c.stored())
            .map(|c| c.name.clone())
            .collect()
    } else {
        columns(pin).iter().map(|c| c.name.clone()).collect()
    };
    let mut select = Vec::new();
    for column in &pin.table.columns {
        if column.stored() {
            if stored.contains(&column.name) {
                select.push(quote_name(&column.name));
            }
        } else {
            select.push(format!(
                "CAST({}, {}) AS {}",
                reprint(&column.default_expression)?,
                quote_string(&column.ch_type),
                quote_name(&column.name)
            ));
        }
    }
    let mut filters = Vec::new();
    for rule in rules {
        let expired = format!("lessOrEquals({}, now())", reprint(&rule.expression)?);
        filters.push(match &rule.condition {
            Some(condition) => format!("not(and({expired}, {}))", reprint(condition)?),
            None => format!("not({expired})"),
        });
    }
    let filter = if filters.is_empty() {
        String::new()
    } else {
        format!("\nWHERE {}", filters.join(" AND "))
    };
    Ok(format!(
        "SELECT {}\nFROM (\n{body}\n){filter}",
        select.join(", ")
    ))
}

/// The columns a pin reads: the table's stored columns that the pinned
/// snapshot's schema has (a time-travelled read sees the schema of its time).
fn columns(pin: &ReadPin) -> Vec<&ddl::ChColumn> {
    pin.table
        .columns
        .iter()
        .filter(|c| c.stored() && pin.fields.iter().any(|f| f.name == c.name))
        .collect()
}

/// `CAST(…) AS c` for every column, restoring its ClickHouse type.
fn casts(pin: &ReadPin) -> Result<Vec<String>, HouseError> {
    columns(pin)
        .into_iter()
        .map(|column| {
            let ty = ChType::parse(&column.ch_type)?;
            let field = pin.fields.iter().find(|f| f.name == column.name);
            let required = field.is_some_and(|f| f.required);
            let name = quote_name(&column.name);
            // D1572, HE1 R9.61: nanosecond times stored as a `long`
            // of nanoseconds since the epoch read back as the time, in the
            // column's zone (or, with none, as declared).
            let stored_nanos = field.is_some_and(|f| f.long);
            // A conversion to `DateTime` keeps its source's time zone, and
            // Iceberg's `timestamptz` reads as UTC: a column declared without a
            // zone drops it first, so it reads as declared.
            let read = match (ty.nanos_zone(), ty.base()) {
                (Some(Some(zone)), _) if stored_nanos => {
                    format!("fromUnixTimestamp64Nano({name}, {})", quote_string(zone))
                }
                (Some(None), _) if stored_nanos => format!("fromUnixTimestamp64Nano({name})"),
                (_, ChType::DateTime64(precision, None)) if *precision > 6 => {
                    format!("fromUnixTimestamp64Nano(toUnixTimestamp64Nano({name}))")
                }
                (_, ChType::DateTime(None) | ChType::DateTime64(_, None)) => {
                    format!("fromUnixTimestamp64Micro(toUnixTimestamp64Micro({name}))")
                }
                _ => name.clone(),
            };
            let value = if required || ty.is_nullable() {
                read
            } else {
                // An optional field of a non-Nullable column: one `ALTER … ADD
                // COLUMN` made (R10.6). Rows written before it read its default.
                // The default is catalog text: printed again from its tree,
                // never spliced (R11.11).
                let default =
                    if column.default_kind == "DEFAULT" || column.default_kind == "MATERIALIZED" {
                        let expression = expr::reprint(&column.default_expression)
                            .map_err(|why| catalog_refusal(pin, &why))?;
                        format!("CAST({expression}, {})", quote_string(&column.ch_type))
                    } else {
                        format!("defaultValueOfTypeName({})", quote_string(&column.ch_type))
                    };
                format!("ifNull({read}, {default})")
            };
            Ok(format!(
                "CAST({value}, {}) AS {name}",
                quote_string(&column.ch_type)
            ))
        })
        .collect()
}

/// A view with the table's columns and no rows.
fn empty(pin: &ReadPin) -> Result<String, HouseError> {
    let structure: Vec<String> = pin
        .table
        .columns
        .iter()
        .filter(|c| c.stored())
        .map(|c| format!("{} {}", quote_name(&c.name), c.ch_type))
        .collect();
    if structure.is_empty() {
        return Err(HouseError::from(ChError::bad_arguments(format!(
            "Table {}.{} has no columns",
            pin.table.database, pin.table.name
        ))));
    }
    Ok(format!(
        "SELECT * FROM null({})",
        quote_string(&structure.join(", "))
    ))
}

/// The sorting key's elements, each printed again from its tree (R11.11);
/// `tuple()` has none.
fn key(pin: &ReadPin) -> Result<Vec<(String, Option<String>)>, HouseError> {
    expr::key_elements(&pin.table.order_by).map_err(|why| catalog_refusal(pin, &why))
}

/// `497`: a table's catalog properties hold text the House does not run as an
/// expression (R11.4, R11.11).
pub fn catalog_refusal(pin: &ReadPin, why: &str) -> HouseError {
    HouseError::from(ChError::access_denied(format!(
        "Table {}.{}: its catalog properties hold an expression the House does not run ({why})",
        pin.table.database, pin.table.name
    )))
}

const SEQ: &str = "__loams_seq";
const ROW: &str = "__loams_row";

/// Replacing `FINAL`: per sorting key, the row with the greatest `(ver, file
/// sequence number, row number)` (the last inserted when there is no `ver`), and
/// none when that row's `is_deleted` is 1.
fn replacing(
    pin: &ReadPin,
    source: &str,
    engine: &LakeEngine,
    files: &[LiveFile],
) -> Result<String, HouseError> {
    let LakeEngine::Replacing { ver, is_deleted } = engine else {
        return Err(HouseError::from(ChError::bad_arguments(
            "not a ReplacingMergeTree table",
        )));
    };
    let inner = sequenced(pin, files)?;
    let mut order = Vec::new();
    if let Some(ver) = ver {
        order.push(format!("{} DESC", quote_name(ver)));
    }
    order.push(format!("{SEQ} DESC"));
    order.push(format!("{ROW} DESC"));
    let key: Vec<String> = key(pin)?.into_iter().map(|(text, _)| text).collect();
    let limit_by = if key.is_empty() {
        "tuple()".to_string()
    } else {
        key.join(", ")
    };
    let outer: Vec<String> = columns(pin).iter().map(|c| quote_name(&c.name)).collect();
    let deleted = is_deleted
        .as_ref()
        .map(|c| format!("\nWHERE {} = 0", quote_name(c)))
        .unwrap_or_default();
    Ok(format!(
        "SELECT {}\nFROM (\nSELECT {}\nFROM {source}\nORDER BY {}\nLIMIT 1 BY {limit_by}\n){deleted}",
        outer.join(", "),
        inner.join(", "),
        order.join(", ")
    ))
}

/// The inner columns of a merge view that orders rows as they were written:
/// the casts, the file's sequence number and the row's number in its file.
fn sequenced(pin: &ReadPin, files: &[LiveFile]) -> Result<Vec<String>, HouseError> {
    let mut inner = casts(pin)?;
    let names: Vec<String> = files.iter().map(|f| quote_string(file_name(f))).collect();
    let seqs: Vec<String> = files
        .iter()
        .map(|f| f.sequence_number.to_string())
        .collect();
    inner.push(format!(
        "transform(_file, [{}]::Array(String), [{}]::Array(Int64), toInt64(0)) AS {SEQ}",
        names.join(", "),
        seqs.join(", ")
    ));
    inner.push(format!("ifNull(_row_number, 0) AS {ROW}"));
    Ok(inner)
}

const BALANCE: &str = "__loams_balance";
const TOTAL: &str = "__loams_total";
const RANK: &str = "__loams_rank";

/// VersionedCollapsing `FINAL` (D1570), as ClickHouse 26.9's merge does
/// (measured on the 26.9.2.8 oracle, `collapsing_final_matches_the_oracle`):
/// rows of one sorting key and version, in the order they were written, go
/// through a stack; a row of the sign on the stack (or onto an empty one) is
/// pushed, a row of the other sign cancels the row on top (the most recent, not
/// the oldest). What stays is the stack at the end.
///
/// With the running sum of signs `b` and the group's total `t`, the stack at
/// the end holds rows of sign `sign(t)` at depths `1 ..= |t|`, and the row at
/// depth `d` is the last row of that sign whose push made `sign(t) * b = d`
/// (after it the sum never fell below `d`, or a later push would reach `d`
/// again). So: keep rows with `sign = sign(t)` and `sign * b` in `1 ..= |t|`,
/// and of those the last per `(key, version, b)`.
fn collapsing(
    pin: &ReadPin,
    source: &str,
    engine: &LakeEngine,
    files: &[LiveFile],
) -> Result<String, HouseError> {
    let LakeEngine::VersionedCollapsing { sign, version } = engine else {
        return Err(HouseError::from(ChError::bad_arguments(
            "not a VersionedCollapsingMergeTree table",
        )));
    };
    let inner = sequenced(pin, files)?;
    let mut group: Vec<String> = key(pin)?.into_iter().map(|(text, _)| text).collect();
    group.push(quote_name(version));
    let group = group.join(", ");
    let sign = quote_name(sign);
    let outer: Vec<String> = columns(pin).iter().map(|c| quote_name(&c.name)).collect();
    let outer = outer.join(", ");
    Ok(format!(
        "SELECT {outer}\nFROM (\n\
         SELECT {outer}, row_number() OVER (PARTITION BY {group}, {BALANCE} ORDER BY {SEQ} DESC, \
         {ROW} DESC) AS {RANK}\nFROM (\n\
         SELECT {outer}, {SEQ}, {ROW}, sum({sign}) OVER (PARTITION BY {group} ORDER BY {SEQ}, {ROW} \
         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS {BALANCE}, \
         sum({sign}) OVER (PARTITION BY {group}) AS {TOTAL}\nFROM (\n\
         SELECT {}\nFROM {source}\n)\n)\n\
         WHERE {sign} = sign({TOTAL}) AND {sign} * {BALANCE} BETWEEN 1 AND abs({TOTAL})\n)\n\
         WHERE {RANK} = 1",
        inner.join(", ")
    ))
}

/// Whether a ClickHouse type is one a `SummingMergeTree` sums by default.
pub(crate) fn is_numeric(ty: &str) -> bool {
    let Ok(parsed) = ChType::parse(ty) else {
        return false;
    };
    if parsed.is_nullable() {
        return false;
    }
    match parsed.base() {
        ChType::Simple(name) => {
            name.starts_with("Int")
                || name.starts_with("UInt")
                || name.starts_with("Float")
                || name.starts_with("Decimal")
        }
        ChType::Decimal(..) => true,
        _ => false,
    }
}

/// Summing `FINAL`: rows grouped by the sorting key, the summed columns added
/// (in their own type, wrapping as ClickHouse's merge does), any value of the
/// others, and a group whose sums are all zero dropped.
fn summing(pin: &ReadPin, source: &str, engine: &LakeEngine) -> Result<String, HouseError> {
    let LakeEngine::Summing { columns: summed } = engine else {
        return Err(HouseError::from(ChError::bad_arguments(
            "not a SummingMergeTree table",
        )));
    };
    let key = key(pin)?;
    let key_columns: Vec<&str> = key.iter().filter_map(|(_, c)| c.as_deref()).collect();
    let columns = columns(pin);
    let is_summed = |c: &ddl::ChColumn| match summed {
        Some(list) => list.iter().any(|s| s == &c.name),
        None => !key_columns.contains(&c.name.as_str()) && is_numeric(&c.ch_type),
    };
    let mut select = Vec::new();
    let mut sums = Vec::new();
    for column in &columns {
        let name = quote_name(&column.name);
        if key_columns.contains(&column.name.as_str()) {
            select.push(name);
        } else if is_summed(column) {
            select.push(format!("sumWithOverflow({name}) AS {name}"));
            sums.push(format!("{name} = 0"));
        } else {
            select.push(format!("any({name}) AS {name}"));
        }
    }
    let group_by = if key.is_empty() {
        "tuple()".to_string()
    } else {
        key.iter()
            .map(|(text, _)| text.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let having = if sums.is_empty() {
        String::new()
    } else {
        format!("\nHAVING NOT ({})", sums.join(" AND "))
    };
    Ok(format!(
        "SELECT {}\nFROM (\nSELECT {}\nFROM {source}\n)\nGROUP BY {group_by}{having}",
        select.join(", "),
        casts(pin)?.join(", ")
    ))
}

/// An aggregate function as a column's type names it (`uniq`, `quantiles(0.5)`):
/// its name and its parameters, if both are plain (a name; numbers, strings and
/// commas). Catalog text, so anything else is refused.
pub(crate) fn aggregate_function(text: &str) -> Option<(String, String)> {
    let text = text.trim();
    let (name, params) = match text.split_once('(') {
        Some((name, rest)) => (name.trim(), format!("({rest}")),
        None => (text, String::new()),
    };
    let plain_name = !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let plain_params = params
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || " ._-+,()'".contains(c))
        && !params.contains("--");
    (plain_name && plain_params).then(|| (name.to_string(), params))
}

/// The function of an `AggregateFunction(f, …)` type's text.
pub(crate) fn state_function(ty: &str) -> Option<(String, String)> {
    let inner = ty.trim().strip_prefix("AggregateFunction(")?;
    let mut depth = 0;
    let end = inner.char_indices().find_map(|(i, c)| match c {
        '(' => {
            depth += 1;
            None
        }
        ')' if depth == 0 => Some(i),
        ')' => {
            depth -= 1;
            None
        }
        ',' if depth == 0 => Some(i),
        _ => None,
    })?;
    aggregate_function(&inner[..end])
}

/// Aggregating `FINAL` (HE1 R9.14): rows grouped by the sorting key; a state
/// column merged with its function's `-MergeState` (still a state, as
/// ClickHouse's merge leaves it), a `SimpleAggregateFunction(f, T)` column with
/// `f`, any value of the others.
fn aggregating(pin: &ReadPin, source: &str) -> Result<String, HouseError> {
    let key = key(pin)?;
    let key_columns: Vec<&str> = key.iter().filter_map(|(_, c)| c.as_deref()).collect();
    let mut select = Vec::new();
    for column in columns(pin) {
        let name = quote_name(&column.name);
        if key_columns.contains(&column.name.as_str()) {
            select.push(name);
            continue;
        }
        let refused = || catalog_refusal(pin, &format!("the type of column {}", column.name));
        select.push(match ChType::parse(&column.ch_type)? {
            ChType::AggregateState => {
                let (function, params) = state_function(&column.ch_type).ok_or_else(refused)?;
                format!("{function}MergeState{params}({name}) AS {name}")
            }
            ChType::SimpleAggregate(function, _) => {
                let (function, params) = aggregate_function(&function).ok_or_else(refused)?;
                format!("{function}{params}({name}) AS {name}")
            }
            _ => format!("any({name}) AS {name}"),
        });
    }
    let group_by = if key.is_empty() {
        "tuple()".to_string()
    } else {
        key.iter()
            .map(|(text, _)| text.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    Ok(format!(
        "SELECT {}\nFROM (\nSELECT {}\nFROM {source}\n)\nGROUP BY {group_by}",
        select.join(", "),
        casts(pin)?.join(", ")
    ))
}

/// A file's name, as chDB's `_file` gives it.
fn file_name(file: &LiveFile) -> &str {
    file.location
        .rsplit_once('/')
        .map_or(file.location.as_str(), |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ddl::{ChColumn, ChTable};
    use crate::read::PinnedField;

    fn column(name: &str, ty: &str) -> ChColumn {
        ChColumn {
            name: name.to_string(),
            ch_type: ty.to_string(),
            ..ChColumn::default()
        }
    }

    fn pin(engine: &str, order_by: &str, columns: Vec<ChColumn>) -> ReadPin {
        let fields = columns
            .iter()
            .map(|c| PinnedField {
                name: c.name.clone(),
                required: !c.ch_type.starts_with("Nullable"),
                long: ChType::parse(&c.ch_type).is_ok_and(|t| t.nanos_zone().is_some()),
            })
            .collect();
        ReadPin {
            table: ChTable {
                database: "db".to_string(),
                name: "t".to_string(),
                class: TableClass::Lake,
                engine: engine.to_string(),
                columns,
                order_by: order_by.to_string(),
                primary_key: String::new(),
                partition_by: String::new(),
                settings: Vec::new(),
                comment: String::new(),
                ddl: String::new(),
                ..ChTable::default()
            },
            location: "s3://local/ns/1/house/db/t-1".to_string(),
            metadata_file: "s3://local/ns/1/house/db/t-1/metadata/00002-x.metadata.json"
                .to_string(),
            snapshot_id: Some(42),
            fields,
            travel: true,
        }
    }

    #[test]
    fn source_names_the_endpoint_bucket_and_pinned_file() {
        let p = pin("MergeTree", "k", vec![column("k", "UInt64")]);
        let root = LakeRoot::from_store_url("s3://bkt/pre").expect("root");
        assert!(
            source(&p, "http://127.0.0.1:9180/", &root).is_err(),
            "outside the root"
        );
        let local = source(&p, "http://127.0.0.1:9180/", &LakeRoot::local()).expect("source");
        assert_eq!(
            local,
            "icebergS3('http://127.0.0.1:9180/local/ns/1/house/db/t-1/', 'dummy', 'dummy', \
             SETTINGS iceberg_metadata_file_path = 'metadata/00002-x.metadata.json')"
        );
    }

    #[test]
    fn plain_view_casts_and_pins_the_snapshot() {
        let mut added = column("n", "UInt32");
        added.default_kind = "DEFAULT".to_string();
        added.default_expression = "k + 1".to_string();
        let mut p = pin(
            "MergeTree",
            "k",
            vec![
                column("k", "UInt64"),
                column("s", "Nullable(String)"),
                added,
            ],
        );
        p.fields[2].required = false;
        let ddl = view_ddl(&p, "SRC", Mode::Plain).expect("view");
        assert_eq!(
            ddl,
            "CREATE OR REPLACE VIEW db.t (k UInt64, s Nullable(String), n UInt32) SQL SECURITY \
             NONE AS\nSELECT CAST(k, 'UInt64') AS k, CAST(s, \
             'Nullable(String)') AS s, CAST(ifNull(n, CAST(plus(`k`, 1), 'UInt32')), 'UInt32') AS n\n\
             FROM SRC\nSETTINGS iceberg_snapshot_id = 42"
        );
        assert!(view_body(&ddl).starts_with("SELECT CAST(k"));
        // FINAL on a MergeTree table is the plain view.
        assert_eq!(view_ddl(&p, "SRC", Mode::Final(&[])).expect("view"), ddl);
    }

    #[test]
    fn no_snapshot_reads_no_rows() {
        let mut p = pin("MergeTree", "k", vec![column("k", "UInt64")]);
        p.snapshot_id = None;
        assert_eq!(
            view_ddl(&p, "SRC", Mode::Plain).expect("view"),
            "CREATE OR REPLACE VIEW db.t AS\nSELECT * FROM null('k UInt64')"
        );
    }

    #[test]
    fn merge_views() {
        let p = pin(
            "ReplacingMergeTree(v, del)",
            "(k, toDate(d))",
            vec![
                column("k", "UInt64"),
                column("d", "DateTime"),
                column("v", "UInt32"),
                column("del", "UInt8"),
            ],
        );
        let files = [LiveFile {
            location: "s3://local/x/data/a.parquet".to_string(),
            record_count: 1,
            size_bytes: 1,
            partition: "tuple()".to_string(),
            sequence_number: 3,
            data: true,
        }];
        let ddl = view_ddl(&p, "SRC", Mode::Final(&files)).expect("view");
        assert!(ddl.contains("transform(_file, ['a.parquet']::Array(String), [3]::Array(Int64)"));
        assert!(ddl.contains("ORDER BY v DESC, __loams_seq DESC, __loams_row DESC"));
        assert!(ddl.contains("LIMIT 1 BY `k`, toDate(`d`)"));
        assert!(ddl.contains("WHERE del = 0"));

        let s = pin(
            "SummingMergeTree",
            "k",
            vec![
                column("k", "UInt64"),
                column("n", "Int64"),
                column("label", "String"),
            ],
        );
        let ddl = view_ddl(&s, "SRC", Mode::Final(&[])).expect("view");
        assert!(ddl.starts_with(
            "CREATE OR REPLACE VIEW db.t (k UInt64, n Int64, label String) SQL SECURITY NONE AS\n\
             SELECT k, sumWithOverflow(n) AS n, any(label) AS label"
        ));
        assert!(ddl.contains("GROUP BY `k`\nHAVING NOT (n = 0)"));
    }

    /// D1572, HE1 R9.61: nanosecond times are stored as a long of
    /// nanoseconds and read back in the column's zone, or as declared.
    #[test]
    fn nanosecond_times_read_back_from_longs() {
        let pin = pin(
            "MergeTree",
            "ts",
            vec![
                column("ts", "DateTime64(9)"),
                column("utc", "DateTime64(9, 'UTC')"),
                column("maybe", "Nullable(DateTime64(9, 'Asia/Kolkata'))"),
                column("micro", "DateTime64(6)"),
            ],
        );
        let casts = casts(&pin).expect("casts");
        assert_eq!(
            casts,
            vec![
                "CAST(fromUnixTimestamp64Nano(ts), 'DateTime64(9)') AS ts".to_string(),
                "CAST(fromUnixTimestamp64Nano(utc, 'UTC'), 'DateTime64(9, \\'UTC\\')') AS utc"
                    .to_string(),
                "CAST(fromUnixTimestamp64Nano(maybe, 'Asia/Kolkata'), \
                 'Nullable(DateTime64(9, \\'Asia/Kolkata\\'))') AS maybe"
                    .to_string(),
                "CAST(fromUnixTimestamp64Micro(toUnixTimestamp64Micro(micro)), 'DateTime64(6)') \
                 AS micro"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn collapsing_view_cancels_by_key_and_version() {
        let p = pin(
            "VersionedCollapsingMergeTree(sign, version)",
            "(k, toDate(d))",
            vec![
                column("k", "String"),
                column("d", "DateTime"),
                column("sign", "Int8"),
                column("version", "UInt64"),
            ],
        );
        let files = [LiveFile {
            location: "s3://local/x/data/a.parquet".to_string(),
            record_count: 1,
            size_bytes: 1,
            partition: "tuple()".to_string(),
            sequence_number: 3,
            data: true,
        }];
        assert!(views_carry(&p, &files));
        let ddl = view_ddl(&p, "SRC", Mode::Final(&files)).expect("view");
        assert!(ddl.contains("transform(_file, ['a.parquet']::Array(String), [3]::Array(Int64)"));
        assert!(ddl.contains(
            "sum(sign) OVER (PARTITION BY `k`, toDate(`d`), version ORDER BY __loams_seq, \
             __loams_row ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS __loams_balance"
        ));
        assert!(
            ddl.contains(
                "sum(sign) OVER (PARTITION BY `k`, toDate(`d`), version) AS __loams_total"
            )
        );
        assert!(ddl.contains(
            "row_number() OVER (PARTITION BY `k`, toDate(`d`), version, __loams_balance ORDER BY \
             __loams_seq DESC, __loams_row DESC) AS __loams_rank"
        ));
        assert!(ddl.contains(
            "WHERE sign = sign(__loams_total) AND sign * __loams_balance BETWEEN 1 AND \
             abs(__loams_total)"
        ));
        assert!(ddl.ends_with("WHERE __loams_rank = 1\nSETTINGS iceberg_snapshot_id = 42"));
        // Without FINAL, the rows as stored.
        assert!(
            !view_ddl(&p, "SRC", Mode::Plain)
                .expect("view")
                .contains("__loams_total")
        );
    }

    fn views_carry(p: &ReadPin, files: &[LiveFile]) -> bool {
        carries_expressions(p, Mode::Final(files)) && !carries_expressions(p, Mode::Plain)
    }

    #[test]
    fn catalog_text_is_reprinted_or_refused() {
        // R11.11: a DEFAULT or a sorting key from the catalog never reaches the
        // control connection as text.
        for default in [
            "1); DROP TABLE x; --",
            "(SELECT count() FROM system.one)",
            "numbers(1)",
            "1, 2",
        ] {
            let mut added = column("n", "UInt32");
            added.default_kind = "DEFAULT".to_string();
            added.default_expression = default.to_string();
            let mut p = pin("MergeTree", "k", vec![column("k", "UInt64"), added]);
            p.fields[1].required = false;
            let err = view_ddl(&p, "SRC", Mode::Plain).expect_err(default);
            assert_eq!(err.code(), 497, "{default}: {err}");
        }
        for order_by in [
            "k, (SELECT 1)",
            "(k, url('http://127.0.0.1:1/x', 'CSV', 'a String'))",
            "`k) FROM numbers(1) --`(k)",
            "k); DROP DATABASE default; --",
        ] {
            let p = pin(
                "ReplacingMergeTree",
                order_by,
                vec![column("k", "UInt64"), column("v", "UInt8")],
            );
            let err = view_ddl(&p, "SRC", Mode::Final(&[])).expect_err(order_by);
            assert_eq!(err.code(), 497, "{order_by}: {err}");
            let s = pin("SummingMergeTree", order_by, vec![column("k", "UInt64")]);
            let err = view_ddl(&s, "SRC", Mode::Final(&[])).expect_err(order_by);
            assert_eq!(err.code(), 497, "{order_by}: {err}");
        }
    }

    #[test]
    fn view_column_types_are_held_to_the_safe_form() {
        // R-ACL.1: catalog text printed outside quotes is one plain type.
        for good in [
            "UInt64",
            "Nullable(Decimal(20, 0))",
            "Enum8('a' = 1, 'b c' = -2)",
            "DateTime64(3, 'Asia/Tokyo')",
            "LowCardinality(Nullable(String))",
            // A named tuple's canonical type spans lines; an unnamed tuple's
            // Iceberg struct has digit names, back-quoted (HS1 Task 12).
            "Tuple(\n    a Int32,\n    b String)",
            "Tuple(`1` Int32, `2` String)",
        ] {
            let p = pin("MergeTree", "k", vec![column("k", good)]);
            assert!(view_ddl(&p, "SRC", Mode::Plain).is_ok(), "{good}");
        }
        for bad in [
            "UInt64) SQL SECURITY NONE AS SELECT * FROM file('/etc/passwd') --",
            "UInt64 -- x",
            "Enum8('a\\' = 1)",
            "Enum8('a\n' = 1)",
            "UInt64; DROP DATABASE d",
            "Enum8('a = 1)",
            "Tuple(`a b` Int32)",
            "Tuple(`a` Int32) --`",
            "Tuple(`a\\` Int32)",
        ] {
            let p = pin("MergeTree", "k", vec![column("k", bad)]);
            // `62` when it is no type at all, `36` when it is one but not plain.
            let err = view_ddl(&p, "SRC", Mode::Plain).expect_err(bad);
            assert!([36, 62].contains(&err.code()), "{bad}: {}", err.code());
        }
        assert_eq!(
            printable_type("Tuple(\n    a Int32,\n    b Enum8('x  y' = 1))").as_deref(),
            Some("Tuple(a Int32, b Enum8('x  y' = 1))")
        );
    }
}
