//! `uint64_key_filter_reads_one_row_group` (BQ1-house, 2026-10-28): a point
//! lookup on a `UInt64` key reads one Parquet row group of sixteen through a
//! read view that carries the House's pushed-down key filter
//! (`loams_house::pushdown`), and all sixteen without it.
//!
//! The data is laid out as the House writes a `UInt64` column: Iceberg
//! `decimal(20, 0)`, read back with `CAST(…, 'UInt64')`, in row groups of
//! `loams_house_lake::write::ROW_GROUP_ROWS` rows, sorted by the key. The
//! count is chDB's own `ParquetReadRowGroups` event, read from `system.events`
//! before and after; this binary runs this one test, so nothing else moves it.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

use loams_chdb::{Engine, EngineConfig, SessionId, Settings};
use loams_house::pushdown::{SOURCE_ALIAS, key_filters};

const ROWS: u64 = 1_048_576;

fn row_groups_read(session: &loams_chdb::Session) -> u64 {
    let out = session
        .query(
            "SELECT value FROM system.events WHERE event = 'ParquetReadRowGroups'",
            "TSVRaw",
            &[],
        )
        .unwrap();
    String::from_utf8(out).unwrap().trim().parse().unwrap_or(0)
}

#[test]
fn uint64_key_filter_reads_one_row_group() {
    let dir = std::env::temp_dir().join(format!("loams-pushdown-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let engine = Engine::start(EngineConfig {
        tmp_dir: dir.join("engine"),
        cache_dir: dir.join("cache"),
        filesystem_cache: false,
        ..EngineConfig::default()
    })
    .unwrap();
    let session = engine
        .session(SessionId::new("pushdown"), &Settings::new())
        .unwrap();
    let file = dir.join("kv.parquet");
    let group = loams_house_lake::write::ROW_GROUP_ROWS;
    session
        .query(
            &format!(
                "INSERT INTO FUNCTION file('{}', Parquet, 'id Decimal(20, 0), v Int64') \
                 SELECT toDecimal128(number, 0), toInt64(number) FROM numbers({ROWS}) \
                 SETTINGS output_format_parquet_row_group_size = {group}, engine_file_truncate_on_insert = 1",
                file.display()
            ),
            "TSV",
            &[],
        )
        .unwrap();
    let source = format!("file('{}', Parquet)", file.display());
    let sql = "SELECT v FROM kv WHERE id = 424242";
    let filters = key_filters(sql, "default", "kv", "default", &["id"]);
    assert_eq!(
        filters,
        vec![format!("{SOURCE_ALIAS}.`id` = toDecimal128('424242', 0)")]
    );
    let view = |where_: &str| {
        format!(
            "CREATE OR REPLACE VIEW default.kv (id UInt64, v Int64) AS \
             SELECT CAST(id, 'UInt64') AS id, CAST(v, 'Int64') AS v FROM {source} AS {SOURCE_ALIAS}{where_}"
        )
    };
    for (where_, expected) in [
        (format!(" WHERE {}", filters.join(" AND ")), 1),
        (String::new(), ROWS.div_ceil(group as u64)),
    ] {
        session.query(&view(&where_), "TSV", &[]).unwrap();
        let before = row_groups_read(&session);
        let answer = session.query(sql, "TSVRaw", &[]).unwrap();
        assert_eq!(String::from_utf8(answer).unwrap().trim(), "424242");
        let read = row_groups_read(&session) - before;
        assert_eq!(read, expected, "row groups read with view `{where_}`");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
