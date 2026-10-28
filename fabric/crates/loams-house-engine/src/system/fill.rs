//! What the dev server's `system` tables hold, from its [`MemCatalog`] and the
//! engine's [`Registry`] (plan HE1 Task 38). The columns filled are the ones
//! the front fills for chDB (`loams-house/src/system.rs`: `tables_view`,
//! `columns_view`, `parts_view`, `databases`, `clusters_view`), so the same
//! catalog reads the same through both engines; every other column holds its
//! type's default. In Task 39 the front sends these tables itself
//! ([`super::inline`]).

use std::collections::BTreeSet;

use datafusion::arrow::datatypes::Schema;
use loams_house_sql::Registry;
use loams_house_sql::functions::{all, canonical_of_internal};
use loams_house_types::arrow::from_arrow;

use super::build::{Cell, Rows};
use super::shapes;
use crate::catalog::{ColumnInfo, ENGINE_DATABASES, MemCatalog, stats};

/// The `system` tables the engine serves, with their engines as
/// `system.tables.engine` prints them and their columns.
pub const SYSTEM_TABLES: &[(&str, &str, shapes::Shape)] = &[
    ("clusters", "SystemClusters", shapes::CLUSTERS),
    ("columns", "SystemColumns", shapes::COLUMNS),
    ("databases", "SystemDatabases", shapes::DATABASES),
    (
        "distributed_ddl_queue",
        "SystemDDLWorkerQueue",
        shapes::DISTRIBUTED_DDL_QUEUE,
    ),
    ("functions", "SystemFunctions", shapes::FUNCTIONS),
    ("macros", "SystemMacros", shapes::MACROS),
    ("mutations", "SystemMutations", shapes::MUTATIONS),
    ("numbers", "SystemNumbers", shapes::NUMBERS),
    ("numbers_mt", "SystemNumbers", shapes::NUMBERS_MT),
    ("one", "SystemOne", shapes::ONE),
    ("parts", "SystemParts", shapes::PARTS),
    ("query_log", "SystemQueryLog", shapes::QUERY_LOG),
    ("settings", "SystemSettings", shapes::SETTINGS),
    ("tables", "SystemTables", shapes::TABLES),
    ("zeros", "SystemZeros", shapes::ZEROS),
    ("zeros_mt", "SystemZeros", shapes::ZEROS_MT),
];

/// The columns of `system.<name>`, if the engine serves it.
pub fn shape(name: &str) -> Option<shapes::Shape> {
    SYSTEM_TABLES
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, _, s)| *s)
}

/// An engine's name without its arguments (`ReplacingMergeTree(v)`).
fn engine_name(engine: &str) -> String {
    engine
        .split_once('(')
        .map_or(engine, |(name, _)| name)
        .trim()
        .to_string()
}

/// Whether an engine keeps its own data (views do not).
fn has_own_data(engine: &str) -> bool {
    !matches!(
        engine,
        "View" | "MaterializedView" | "LiveView" | "Dictionary"
    )
}

/// Whether an engine has parts (the `MergeTree` family).
fn has_parts(engine: &str) -> bool {
    engine.ends_with("MergeTree")
}

/// A key's columns: its elements that are bare columns, or one column inside
/// a function (`toYYYYMM(ts)`).
pub fn key_columns(key: &str) -> BTreeSet<String> {
    let key = key.trim();
    let key = key
        .strip_prefix("tuple(")
        .and_then(|k| k.strip_suffix(')'))
        .unwrap_or(key);
    let mut out = BTreeSet::new();
    let mut depth = 0i32;
    let mut start = 0;
    let bytes = key.as_bytes();
    let mut elements = Vec::new();
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                elements.push(&key[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    elements.push(&key[start..]);
    for e in elements {
        let e = e.trim();
        if e.is_empty() {
            continue;
        }
        let inner = e
            .split_once('(')
            .and_then(|(_, rest)| rest.strip_suffix(')'))
            .map_or(e, |inner| inner.split(',').next().unwrap_or(inner).trim());
        out.insert(inner.trim_matches('`').to_string());
    }
    out
}

/// The columns of a table: the recorded ones, else its rows' schema.
pub fn columns_of(schema: &Schema, recorded: &[ColumnInfo]) -> Vec<ColumnInfo> {
    if !recorded.is_empty() {
        return recorded.to_vec();
    }
    schema
        .fields()
        .iter()
        .map(|f| ColumnInfo {
            name: f.name().clone(),
            ty: from_arrow(f).map(|t| t.to_string()).unwrap_or_default(),
            ..ColumnInfo::default()
        })
        .collect()
}

/// `system.databases`.
pub fn databases(catalog: &MemCatalog) -> Rows {
    let mut rows = Rows::new(&["name", "engine", "engine_full", "comment"]);
    for db in catalog.databases() {
        rows.push(vec![db.into(), "Atomic".into(), "Atomic".into(), "".into()]);
    }
    for db in ENGINE_DATABASES {
        let engine = if *db == "system" { "Atomic" } else { "Memory" };
        rows.push(vec![(*db).into(), engine.into(), engine.into(), "".into()]);
    }
    rows
}

/// `system.tables`.
pub fn tables(catalog: &MemCatalog) -> Rows {
    let mut rows = Rows::new(&[
        "database",
        "name",
        "engine",
        "is_temporary",
        "create_table_query",
        "engine_full",
        "as_select",
        "partition_key",
        "sorting_key",
        "primary_key",
        "sampling_key",
        "total_rows",
        "total_bytes",
        "parts",
        "active_parts",
        "comment",
        "has_own_data",
        "table",
    ]);
    for (db, name, table) in catalog.list() {
        let info = catalog.info(&db, &name);
        let engine = engine_name(&table.engine);
        let own = has_own_data(&engine);
        let (count, bytes) = stats(&table.data);
        let parts = u64::from(has_parts(&engine) && count > 0);
        let primary = if info.primary_key.is_empty() {
            table.sorting_key.clone()
        } else {
            info.primary_key.clone()
        };
        let sized = |v: u64| if own { Cell::UInt(v) } else { Cell::Null };
        rows.push(vec![
            db.as_str().into(),
            name.as_str().into(),
            engine.as_str().into(),
            false.into(),
            info.create_table_query.as_str().into(),
            info.engine_full.as_str().into(),
            info.as_select.as_str().into(),
            info.partition_key.as_str().into(),
            table.sorting_key.as_str().into(),
            primary.into(),
            info.sampling_key.as_str().into(),
            sized(count),
            sized(bytes),
            sized(parts),
            sized(parts),
            info.comment.as_str().into(),
            own.into(),
            name.as_str().into(),
        ]);
    }
    for (name, engine, _) in SYSTEM_TABLES {
        rows.push(vec![
            "system".into(),
            (*name).into(),
            (*engine).into(),
            false.into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            "".into(),
            Cell::Null,
            Cell::Null,
            Cell::Null,
            Cell::Null,
            "".into(),
            false.into(),
            (*name).into(),
        ]);
    }
    // `uuid` is a `UUID`: it holds its default, the zero UUID (R38.3).
    rows
}

/// `system.columns`.
pub fn columns(catalog: &MemCatalog) -> Rows {
    let mut rows = Rows::new(&[
        "database",
        "table",
        "name",
        "type",
        "position",
        "default_kind",
        "default_expression",
        "comment",
        "is_in_partition_key",
        "is_in_sorting_key",
        "is_in_primary_key",
        "is_in_sampling_key",
        "compression_codec",
        "column",
    ]);
    for (db, name, table) in catalog.list() {
        let info = catalog.info(&db, &name);
        let schema = datafusion::catalog::TableProvider::schema(table.data.as_ref());
        let recorded = !info.columns.is_empty();
        let sorting = key_columns(&table.sorting_key);
        let partition = key_columns(&info.partition_key);
        let primary = if info.primary_key.is_empty() {
            sorting.clone()
        } else {
            key_columns(&info.primary_key)
        };
        let sampling = key_columns(&info.sampling_key);
        for (i, c) in columns_of(&schema, &info.columns).into_iter().enumerate() {
            let flag = |set: &BTreeSet<String>, recorded_flag: bool| {
                Cell::from(if recorded {
                    recorded_flag
                } else {
                    set.contains(&c.name)
                })
            };
            rows.push(vec![
                db.as_str().into(),
                name.as_str().into(),
                c.name.as_str().into(),
                c.ty.as_str().into(),
                Cell::UInt(i as u64 + 1),
                c.default_kind.as_str().into(),
                c.default_expression.as_str().into(),
                c.comment.as_str().into(),
                flag(&partition, c.is_in_partition_key),
                flag(&sorting, c.is_in_sorting_key),
                flag(&primary, c.is_in_primary_key),
                flag(&sampling, c.is_in_sampling_key),
                if c.codec.is_empty() {
                    "".into()
                } else {
                    format!("CODEC({})", c.codec).into()
                },
                c.name.as_str().into(),
            ]);
        }
    }
    for (table, _, shape) in SYSTEM_TABLES {
        for (i, (name, ty)) in shape.iter().enumerate() {
            let alias = shapes::ALIASES
                .iter()
                .find(|(t, c, _)| t == table && c == name)
                .map(|(_, _, target)| *target);
            rows.push(vec![
                "system".into(),
                (*table).into(),
                (*name).into(),
                (*ty).into(),
                Cell::UInt(i as u64 + 1),
                if alias.is_some() { "ALIAS" } else { "" }.into(),
                alias.unwrap_or("").into(),
                "".into(),
                false.into(),
                false.into(),
                false.into(),
                false.into(),
                "".into(),
                (*name).into(),
            ]);
        }
    }
    rows
}

/// `system.parts`: one active part per `MergeTree`-family table with rows
/// (the dev server's tables are one batch set each; the House's are one part
/// per live data file, the front's `parts_view`).
pub fn parts(catalog: &MemCatalog) -> Rows {
    let mut rows = Rows::new(&[
        "partition",
        "partition_id",
        "name",
        "active",
        "rows",
        "bytes_on_disk",
        "data_compressed_bytes",
        "database",
        "table",
        "engine",
        "disk_name",
        "min_block_number",
        "max_block_number",
        "level",
        "visible",
        "part_name",
        "bytes",
    ]);
    for (db, name, table) in catalog.list() {
        let engine = engine_name(&table.engine);
        let (count, bytes) = stats(&table.data);
        if !has_parts(&engine) || count == 0 {
            continue;
        }
        rows.push(vec![
            "tuple()".into(),
            "all".into(),
            "all_1_1_0".into(),
            true.into(),
            Cell::UInt(count),
            Cell::UInt(bytes),
            Cell::UInt(bytes),
            db.as_str().into(),
            name.as_str().into(),
            engine.as_str().into(),
            "default".into(),
            Cell::Int(1),
            Cell::Int(1),
            Cell::UInt(0),
            true.into(),
            "all_1_1_0".into(),
            Cell::UInt(bytes),
        ]);
    }
    rows
}

/// `system.functions`: the functions the engine registers, under every name
/// ClickHouse gives them (aliases with `alias_to`), so an editor is offered
/// only what the engine runs (Task 15's expected gap).
pub fn functions(registry: &Registry) -> Rows {
    let registered: BTreeSet<&str> = registry
        .scalars
        .keys()
        .chain(registry.aggregates.keys())
        .chain(registry.windows.keys())
        .chain(registry.higher_order.keys())
        .map(|k| canonical_of_internal(k))
        .collect();
    let mut rows = Rows::new(&["name", "is_aggregate", "case_insensitive", "alias_to"]);
    for (name, aggregate, insensitive, alias_to) in all() {
        let canonical = if alias_to.is_empty() { name } else { alias_to };
        if registered.contains(canonical) {
            rows.push(vec![
                name.into(),
                aggregate.into(),
                insensitive.into(),
                alias_to.into(),
            ]);
        }
    }
    rows
}

/// The cluster an `ON CLUSTER` clause names in `ddl`, if any.
fn on_cluster(ddl: &str) -> Option<String> {
    let upper = ddl.to_ascii_uppercase();
    let at = upper.find(" ON CLUSTER ")? + " ON CLUSTER ".len();
    let rest = ddl[at..].trim_start();
    let name: String = if let Some(q) = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '\'' | '`' | '"'))
    {
        rest[1..].chars().take_while(|c| *c != q).collect()
    } else {
        rest.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '{' || *c == '}')
            .collect()
    };
    (!name.is_empty()).then_some(name)
}

/// `system.clusters` (HE1 R9.62): one row, this node, for each cluster an
/// `ON CLUSTER` clause of the catalog's stored `CREATE` statements names.
pub fn clusters(catalog: &MemCatalog) -> Rows {
    let names: BTreeSet<String> = catalog
        .list()
        .iter()
        .filter_map(|(db, name, _)| {
            let info = catalog.info(db, name);
            on_cluster(&info.show_create).or_else(|| on_cluster(&info.create_table_query))
        })
        .collect();
    let mut rows = Rows::new(&[
        "cluster",
        "shard_num",
        "shard_weight",
        "replica_num",
        "host_name",
        "host_address",
        "port",
        "is_local",
        "user",
        "name",
    ]);
    for cluster in names {
        rows.push(vec![
            cluster.as_str().into(),
            Cell::UInt(1),
            Cell::UInt(1),
            Cell::UInt(1),
            "localhost".into(),
            "127.0.0.1".into(),
            Cell::UInt(9000),
            true.into(),
            "default".into(),
            cluster.as_str().into(),
        ]);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_name_their_columns() {
        let k = key_columns("tuple(a, toStartOfHour(ts), `b c`)");
        assert_eq!(
            k.into_iter().collect::<Vec<_>>(),
            vec!["a".to_string(), "b c".to_string(), "ts".to_string()]
        );
        assert!(key_columns("tuple()").is_empty());
        assert_eq!(
            on_cluster("CREATE TABLE a.b ON CLUSTER 'c1' (x UInt8)").as_deref(),
            Some("c1")
        );
        assert_eq!(on_cluster("CREATE TABLE a.b (x UInt8)"), None);
    }
}
