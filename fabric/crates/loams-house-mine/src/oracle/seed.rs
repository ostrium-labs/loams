//! Capture oracle metadata and binary Parquet before a seeded target reads it.
//! The oracle runs in a dedicated database server; all non-system tables are
//! snapshotted. Publication is atomic so a failed export never exposes half a seed.
use super::{
    normalise::Outcome,
    target::{Client, Target},
};
use anyhow::{Context as _, bail};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn identifier(s: &str) -> String {
    format!("`{}`", s.replace('\\', "\\\\").replace('`', "\\`"))
}
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}
fn rows(client: &Client, target: &Target, sql: &str) -> anyhow::Result<Vec<Vec<Value>>> {
    match client.query(target, sql)? {
        Outcome::Rows(r) => Ok(r.rows),
        Outcome::Error { code, message } => bail!("snapshot query failed: {code} {message}"),
    }
}
fn text(row: &[Value], at: usize) -> anyhow::Result<&str> {
    row.get(at)
        .and_then(Value::as_str)
        .context("snapshot metadata must be strings")
}
fn hex(s: &str) -> String {
    s.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
/// Snapshot every user table after baseline setup. Identifiers never become raw paths.
pub fn capture(client: &Client, baseline: &Target, root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root)?;
    let generation = format!(
        "seed-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    let tables = rows(
        client,
        baseline,
        "SELECT database, name, engine, sorting_key, partition_key FROM system.tables WHERE database NOT IN ('system', 'information_schema', 'INFORMATION_SCHEMA') AND is_temporary = 0 ORDER BY database, name",
    )?;
    let mut snapshot = Vec::new();
    for row in tables {
        let db = text(&row, 0)?;
        let name = text(&row, 1)?;
        let cols = rows(
            client,
            baseline,
            &format!(
                "SELECT name, type, default_kind, default_expression FROM system.columns WHERE database = {} AND table = {} ORDER BY position",
                literal(db),
                literal(name)
            ),
        )?;
        let mut columns = Vec::new();
        for col in cols {
            columns.push(json!({"name":text(&col,0)?,"type":text(&col,1)?,"default_kind":text(&col,2)?,"default_expression":text(&col,3)?}));
        }
        let file = format!("{generation}/{}/{}/rows.parquet", hex(db), hex(name));
        let path = root.join(&file);
        fs::create_dir_all(path.parent().context("snapshot parent")?)?;
        let projection = columns
            .iter()
            .map(|col| {
                let name = identifier(col["name"].as_str().expect("column name"));
                if col["type"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("AggregateFunction("))
                {
                    format!("toString({name}) AS {name}")
                } else {
                    name
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let (status, code, bytes) = client.raw(
            baseline,
            &format!(
                "SELECT {projection} FROM {}.{} FORMAT Parquet",
                identifier(db),
                identifier(name)
            ),
        )?;
        if status != 200 || code.is_some() {
            bail!("exporting {db}.{name}: {}", String::from_utf8_lossy(&bytes));
        }
        fs::write(path, bytes)?;
        snapshot.push(json!({"database":db,"name":name,"engine":text(&row,2)?,"sorting_key":text(&row,3)?,"partition_key":text(&row,4)?,"columns":columns,"parquet":file}));
    }
    let temp = root.join(format!("{generation}.json"));
    fs::write(&temp, serde_json::to_vec(&json!({"tables":snapshot}))?)?;
    fs::rename(temp, root.join("snapshot.json"))?;
    Ok(())
}
