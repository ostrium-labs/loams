//! `spark_v3_tables_read_as_spark` (plan HE1 Task 26, R26.10): Iceberg v3
//! tables written by unmodified Spark 4.0.4 with Iceberg 1.12.0 (deletion
//! vectors, a partitioned table, row lineage, a v2 table with position
//! deletes, and a v2 table upgraded to v3) read as Spark reads them.
//!
//! The expected values are Spark's: the HE0 Task 11 spike's fingerprints
//! (`spikes/he0-iceberg-v3/results/spark-08.txt` on `he0-iceberg-v3-spike`)
//! and the counts that follow from `fixtures/spark-v3/write.sql`.
//!
//! The tables are written by `scripts/house/spark-v3-fixtures.sh` (Spark in a
//! pinned container, a Hadoop catalog on a local directory) and read from
//! `LOAMS_SPARK_V3_WAREHOUSE`; until they are committed as fixtures the test
//! is ignored.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use common::{context, local};
use datafusion::arrow::array::{Array, AsArray, RecordBatch};
use datafusion::arrow::compute::{cast, concat_batches};
use datafusion::arrow::datatypes::{DataType, Int64Type};
use loams_house_iceberg::{TablePin, provider};
use loams_house_sql::Registry;

/// The warehouse's namespace directory (`<warehouse>/v3`).
fn namespace() -> PathBuf {
    let warehouse = std::env::var("LOAMS_SPARK_V3_WAREHOUSE").expect(
        "LOAMS_SPARK_V3_WAREHOUSE: the warehouse scripts/house/spark-v3-fixtures.sh writes",
    );
    Path::new(&warehouse).join("v3")
}

/// The version of a metadata file name: Hadoop's `v3.metadata.json` or the
/// REST catalogs' `00003-<uuid>.metadata.json`.
fn version(name: &str) -> Option<u64> {
    let stem = name.strip_suffix(".metadata.json")?;
    let digits: String = stem
        .trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// The pin of `table`'s current metadata, row lineage read as columns.
fn pin(table: &str) -> TablePin {
    let dir = namespace().join(table);
    let newest = std::fs::read_dir(dir.join("metadata"))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            version(&name).map(|v| (v, e.path()))
        })
        .max_by_key(|(v, _)| *v)
        .unwrap_or_else(|| panic!("{table}: no metadata file"))
        .1;
    let bytes = std::fs::read(&newest).unwrap();
    let mut pin =
        TablePin::from_metadata_json("v3", table, &format!("file://{}", newest.display()), &bytes)
            .unwrap();
    assert!(
        pin.external,
        "{table}: Spark's table has no House properties"
    );
    pin.location = format!("file://{}", dir.display());
    pin.lineage = true;
    pin
}

async fn query(table: &str, sql: &str) -> RecordBatch {
    let ctx = context();
    let p = provider(&pin(table), local(), &Registry::default())
        .await
        .unwrap();
    ctx.register_table("t", p).unwrap();
    let df = ctx.sql(sql).await.unwrap();
    let schema = Arc::new(df.schema().as_arrow().clone());
    let batches = df.collect().await.unwrap();
    concat_batches(&schema, &batches).unwrap()
}

/// One row of numbers, each cast to `Int64`.
async fn numbers(table: &str, sql: &str) -> Vec<i64> {
    let batch = query(table, sql).await;
    assert_eq!(batch.num_rows(), 1, "{table}: {sql}");
    batch
        .columns()
        .iter()
        .map(|c| {
            let c = cast(c, &DataType::Int64).unwrap();
            assert!(!c.is_null(0), "{table}: {sql}");
            c.as_primitive::<Int64Type>().value(0)
        })
        .collect()
}

/// Spark's lineage fingerprint: `count(*)`, `sum(_row_id)`,
/// `sum(_row_id * id)`, `sum(_last_updated_sequence_number)`.
async fn fingerprint(table: &str, filter: &str) -> Vec<i64> {
    numbers(
        table,
        &format!(
            "SELECT count(*), sum(_row_id), sum(_row_id * id), \
             sum(_last_updated_sequence_number) FROM t {filter}"
        ),
    )
    .await
}

#[tokio::test]
#[ignore = "needs LOAMS_SPARK_V3_WAREHOUSE (scripts/house/spark-v3-fixtures.sh)"]
async fn spark_v3_tables_read_as_spark() {
    // Rows, the sum of ids, and the rows an update marked.
    let counts = "SELECT count(*), sum(id), sum(CASE WHEN v = '{m}' THEN 1 ELSE 0 END) FROM t";
    let marked = |m: &str| counts.replace("{m}", m);
    // dv: 0..1000 less id % 10 in (0, 1); id % 10 = 2 updated to 'u'.
    assert_eq!(numbers("dv", &marked("u")).await, vec![800, 400_400, 100]);
    // dv_part: 0..3000 less multiples of 7; multiples of 5 left marked 'm'.
    assert_eq!(
        numbers("dv_part", &marked("m")).await,
        vec![2571, 3_855_858, 514]
    );
    // lineage: 0..100, ids ending in 3 updated to 'c'.
    assert_eq!(numbers("lineage", &marked("c")).await, vec![100, 4950, 10]);
    // posdel_v2 (position delete files): 0..1000 less id % 10 = 0.
    assert_eq!(
        numbers("posdel_v2", &marked("u")).await,
        vec![900, 450_000, 100]
    );

    // Row lineage as Spark reads it (spark-08.txt).
    assert_eq!(
        fingerprint("lineage", "WHERE id >= 40").await,
        vec![60, 4170, 307_810, 117]
    );
    assert_eq!(
        fingerprint("dv", "WHERE id >= 500").await,
        vec![400, 300_200, 233_632_200, 550]
    );
    assert_eq!(
        fingerprint("dv_part", "WHERE id >= 1000").await,
        vec![1714, 3_106_464, 6_837_533_572, 3428]
    );
    assert_eq!(
        fingerprint("dv_part", "").await,
        vec![2571, 3_855_750, 7_230_336_108, 4798]
    );
    assert_eq!(
        fingerprint("upgraded", "WHERE id >= 500").await,
        vec![490, 344_690, 268_441_140, 670]
    );
    assert_eq!(
        fingerprint("upgraded", "").await,
        vec![890, 484_890, 311_893_340, 1070]
    );
}

#[test]
fn metadata_versions_read_both_namings() {
    assert_eq!(version("v12.metadata.json"), Some(12));
    assert_eq!(
        version("00003-6c1a8f0e-0000-0000-0000-000000000000.metadata.json"),
        Some(3)
    );
    assert_eq!(version("version-hint.text"), None);
}
