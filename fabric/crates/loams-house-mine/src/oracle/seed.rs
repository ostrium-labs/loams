//! Seeded targets (plan HE1 Task 23, D1804): the Rust engine's dev server
//! does not run a corpus's setup statements (it is read-only in HE1). After
//! the baseline has run them, every table they made is copied to it: the
//! columns' ClickHouse types from `system.columns`, the rows as `Parquet`.
//! So the data is the oracle's by construction, and a difference is the
//! engine's.
//!
//! The target takes `POST /loams/seed?database=…&table=…&engine=…&sorting_key=…`
//! with the Parquet body and the columns as JSON in `X-Loams-Columns`
//! (`[["name", "Type"], …]`).

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
         AND NOT is_temporary AND engine NOT IN ('View', 'MaterializedView', 'LiveView') ORDER BY name \
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
                Err(e) => bail!("the baseline refused to export {table}: {e}"),
            };
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
            &[("X-Loams-Columns", columns.to_string())],
            &parquet,
        )? {
            bail!("{} refused the seed of {table}: {e}", target.name);
        }
    }
    Ok(())
}
