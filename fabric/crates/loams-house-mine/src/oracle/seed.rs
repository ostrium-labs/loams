//! Seeded targets (plan HE1 Task 23, D1804): the Rust engine's dev server
//! does not run a corpus's setup statements (it is read-only in HE1). After
//! the baseline has run them, every table they made is copied to it: the
//! columns' ClickHouse types from `system.columns`, the rows as `Parquet`.
//! So the data is the oracle's by construction, and a difference is the
//! engine's.
//!
//! The target takes `POST /loams/seed?database=…&table=…&engine=…&sorting_key=…`
//! with the Parquet body and the columns as JSON in `X-Loams-Columns`
//! (`[["name", "Type"], …]`; the columns `SELECT *` returns), and what
//! `system.tables`, `system.columns`, `DESCRIBE` and `SHOW CREATE` print about
//! the table as JSON in `X-Loams-Table` (plan HE1 Task 38). Plain views are
//! copied as their rows, so `DESCRIBE` and `SELECT` of a view answer on the
//! engine (R9.13's gap); a view whose rows cannot be exported is skipped.

use anyhow::{Context as _, bail};
use serde_json::Value;

use super::target::{Client, Target};

fn text(client: &Client, baseline: &Target, sql: &str) -> anyhow::Result<String> {
    match client.fetch(baseline, sql)? {
        Ok(bytes) => Ok(String::from_utf8(bytes).context("the baseline's answer is not UTF-8")?),
        Err(e) => bail!("the baseline refused {sql:?}: {e}"),
    }
}

/// A string literal for the baseline.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Copies every table of the baseline's current database to `target`.
///
/// # Errors
///
/// A refused statement on either side, or an unreachable target.
pub fn copy_tables(client: &Client, baseline: &Target, target: &Target) -> anyhow::Result<()> {
    let db = text(client, baseline, "SELECT currentDatabase() FORMAT TSVRaw")?
        .trim()
        .to_string();
    let tables = text(
        client,
        baseline,
        "SELECT name, engine, sorting_key FROM system.tables WHERE database = currentDatabase() \
         AND NOT is_temporary AND engine NOT IN ('MaterializedView', 'LiveView') ORDER BY name \
         FORMAT JSONCompactEachRow",
    )?;
    for line in tables.lines().filter(|l| !l.trim().is_empty()) {
        let row: Vec<String> =
            serde_json::from_str(line).with_context(|| format!("system.tables row {line}"))?;
        let [table, engine, sorting_key] =
            <[String; 3]>::try_from(row).map_err(|r| anyhow::anyhow!("system.tables row {r:?}"))?;
        let columns = text(
            client,
            baseline,
            &format!(
                "SELECT name, type FROM system.columns WHERE database = currentDatabase() AND table = {} \
                 AND default_kind NOT IN ('ALIAS', 'MATERIALIZED', 'EPHEMERAL') \
                 ORDER BY position FORMAT JSONCompact",
                literal(&table)
            ),
        )?;
        let columns: Value =
            serde_json::from_str(&columns).context("system.columns as JSONCompact")?;
        let columns = columns
            .get("data")
            .cloned()
            .context("JSONCompact without data")?;
        let quoted = format!("`{}`", table.replace('`', "\\`"));
        let parquet =
            match client.fetch(baseline, &format!("SELECT * FROM {quoted} FORMAT Parquet"))? {
                Ok(bytes) => bytes,
                Err(e) if engine == "View" => {
                    eprintln!("oracle: not seeding the view {table}: {e}");
                    continue;
                }
                Err(e) => bail!("the baseline refused to export {table}: {e}"),
            };
        let info = table_info(client, baseline, &table, &quoted)?;
        let params = [
            ("database".to_string(), db.clone()),
            ("table".to_string(), table.clone()),
            ("engine".to_string(), engine),
            ("sorting_key".to_string(), sorting_key),
        ];
        if let Err(e) = client.post_to(
            target,
            "loams/seed",
            &params,
            &[
                ("X-Loams-Columns", columns.to_string()),
                ("X-Loams-Table", info.to_string()),
            ],
            &parquet,
        )? {
            bail!("{} refused the seed of {table}: {e}", target.name);
        }
    }
    Ok(())
}

/// One `JSONCompactEachRow` row of the baseline's answer to `sql`.
fn rows(client: &Client, baseline: &Target, sql: &str) -> anyhow::Result<Vec<Vec<Value>>> {
    text(client, baseline, sql)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("a row of {sql:?}: {l}")))
        .collect()
}

/// What the baseline prints about `table` (`X-Loams-Table`, plan HE1 Task
/// 38): `system.tables`' keys and statements, `SHOW CREATE TABLE`, and each
/// column from `DESCRIBE` with `system.columns`' key flags.
fn table_info(
    client: &Client,
    baseline: &Target,
    table: &str,
    quoted: &str,
) -> anyhow::Result<Value> {
    let str_at = |row: &[Value], i: usize| {
        row.get(i)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let t = rows(
        client,
        baseline,
        &format!(
            "SELECT engine_full, partition_key, primary_key, sampling_key, create_table_query, \
             as_select, comment FROM system.tables WHERE database = currentDatabase() AND name = {} \
             FORMAT JSONCompactEachRow",
            literal(table)
        ),
    )?;
    let t = t.first().cloned().unwrap_or_default();
    let show = rows(
        client,
        baseline,
        &format!("SHOW CREATE TABLE {quoted} FORMAT JSONCompactEachRow"),
    )?;
    let show_create = show.first().map(|r| str_at(r, 0)).unwrap_or_default();
    let flags = rows(
        client,
        baseline,
        &format!(
            "SELECT name, is_in_partition_key, is_in_sorting_key, is_in_primary_key, \
             is_in_sampling_key FROM system.columns WHERE database = currentDatabase() \
             AND table = {} FORMAT JSONCompactEachRow",
            literal(table)
        ),
    )?;
    let describe = rows(
        client,
        baseline,
        &format!("DESCRIBE TABLE {quoted} FORMAT JSONCompactEachRow"),
    )?;
    let columns: Vec<Value> = describe
        .iter()
        .map(|d| {
            let name = str_at(d, 0);
            let f = flags
                .iter()
                .find(|f| str_at(f, 0) == name)
                .cloned()
                .unwrap_or_default();
            let flag = |i: usize| f.get(i).and_then(Value::as_u64).unwrap_or(0) != 0;
            serde_json::json!({
                "name": name,
                "type": str_at(d, 1),
                "default_kind": str_at(d, 2),
                "default_expression": str_at(d, 3),
                "comment": str_at(d, 4),
                "codec": str_at(d, 5),
                "ttl": str_at(d, 6),
                "is_in_partition_key": flag(1),
                "is_in_sorting_key": flag(2),
                "is_in_primary_key": flag(3),
                "is_in_sampling_key": flag(4),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "engine_full": str_at(&t, 0),
        "partition_key": str_at(&t, 1),
        "primary_key": str_at(&t, 2),
        "sampling_key": str_at(&t, 3),
        "create_table_query": str_at(&t, 4),
        "as_select": str_at(&t, 5),
        "comment": str_at(&t, 6),
        "show_create": show_create,
        "columns": columns,
    }))
}
