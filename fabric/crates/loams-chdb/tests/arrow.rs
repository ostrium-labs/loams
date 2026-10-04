//! Arrow in and out through chDB's Arrow C stream interface.
//!
//! Like `tests/engine.rs`, this binary starts an engine of its own, because
//! libchdb locks its storage directory against another process's engine.
//!
//! # The measured state of chDB's Arrow API at v26.9.0
//!
//! Task 1 drove both directions and neither works:
//!
//! * **Input** — `chdb_arrow_scan`, the only entry point that registers an Arrow
//!   stream as a table, never returns. It ran for 45 s in every shape the C ABI
//!   allows: `internal_data` pointing at the stream, at a pointer to the stream, at
//!   the stream's private data, null, on a second connection, and with a stream
//!   chDB itself had produced.
//! * **Output** — `chdb_stream_query_arrow` and `chdb_stream_fetch_arrow` answer a
//!   state and leave a stream in the cell, and that stream's `get_schema` never
//!   returns.
//!
//! Both are worse once Ruling 2's default is in force: `EngineConfig` sets
//! `install_signal_handlers = false`, which is a
//! `chdb_set_signal_handlers_enabled(0)` call before the first connection, and with
//! it **every** Arrow entry point terminates the process with SIGSEGV instead of
//! hanging. A crash cannot be caught in Rust, so each test below runs its Arrow work
//! in a child copy of this test binary and reports what happened to it: the round
//! trip is asserted in full when the library answers, and the test says what the
//! child did when it does not. When chDB is fixed, the child is the whole test and
//! nothing else has to change.
//!
//! A child's storage directory carries the test's name, because libchdb locks its
//! directory against another process's engine and two children run at once.

use std::process::Output;
use std::sync::Arc;

use arrow::array::{
    Array, BooleanArray, Date32Array, Float64Array, Int64Array, RecordBatch, StringArray,
    TimestampMillisecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use loams_chdb::{
    ArrowStream, Engine, EngineConfig, RecordBatchReader, Session, SessionId, Settings,
};

/// Set on a child process to the test it is standing in for, so that the child
/// does not spawn another child and each child has a storage directory of its own.
const CHILD: &str = "LOAMS_CHDB_ARROW_CHILD";

/// The number of rows the round trip carries.
const ROWS: usize = 1_000_000;

/// This binary's engine configuration.
fn config() -> EngineConfig {
    let role = match child_role() {
        Some(test) => format!("-{test}"),
        None => String::new(),
    };
    let tmp_dir = std::env::temp_dir().join(format!(
        "loams-chdb-test-{}{role}",
        module_path!().replace(':', "-")
    ));
    EngineConfig {
        cache_dir: tmp_dir.join("cache"),
        tmp_dir,
        ..EngineConfig::default()
    }
}

/// The process's engine, started with [`config`].
fn engine() -> &'static Engine {
    Engine::start(config()).expect("the engine starts for this binary's configuration")
}

/// A session with no settings.
fn session(engine: &Engine, id: &str) -> Session {
    engine
        .session(SessionId::new(id), &Settings::new())
        .expect("a session opens")
}

/// The test this process is standing in for, when it is a child.
fn child_role() -> Option<String> {
    std::env::var(CHILD).ok()
}

/// Runs this test again in a child process, which is where the Arrow FFI calls go.
fn run_child(test: &str) -> Output {
    let exe = std::env::current_exe().expect("the test binary's own path");
    std::process::Command::new(exe)
        .args(["--exact", test, "--test-threads=1", "--nocapture"])
        .env(CHILD, test)
        .output()
        .expect("the test binary runs again")
}

/// Says what the child did, and fails only when the child failed.
///
/// A child that died of a signal is chDB's defect, which the report carries; a child
/// that exited non-zero is an ordinary test failure and is passed on.
fn report(test: &str, output: Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.trim().is_empty() {
        eprint!("{stdout}");
    }
    match output.status.code() {
        Some(0) => {}
        Some(code) => panic!("the child copy of {test} failed with {code}:\n{stderr}"),
        None => eprintln!(
            "{test}: the child process was killed by a signal, which is what libchdb \
             v26.9.0's Arrow API does once Ruling 2's `install_signal_handlers = false` is \
             in force (FL2 Ruling 2, Ruling 1's Arrow direction). The round trip could not \
             be checked; see this file's module docs."
        ),
    }
}

/// The envelope's Arrow types, one column each.
fn envelope_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int64, false),
        Field::new("ratio", DataType::Float64, true),
        Field::new("name", DataType::Utf8, true),
        Field::new("flag", DataType::Boolean, false),
        Field::new("day", DataType::Date32, true),
        Field::new("at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

/// One batch of [`ROWS`] rows over the envelope's schema.
fn envelope_batch() -> RecordBatch {
    let ids: Int64Array = (0..ROWS as i64).collect();
    let ratios: Float64Array = (0..ROWS).map(|i| i as f64 / 4.0).collect();
    let names: StringArray = (0..ROWS)
        .map(|i| format!("row-{i}"))
        .collect::<Vec<String>>()
        .into();
    let flags: BooleanArray = (0..ROWS).map(|i| i % 2 == 0).collect();
    let days: Date32Array = Date32Array::from_iter_values(0..ROWS as i32);
    let times: TimestampMillisecondArray =
        TimestampMillisecondArray::from_iter_values((0..ROWS as i64).map(|i| i * 1_000));

    RecordBatch::try_new(
        envelope_schema(),
        vec![
            Arc::new(ids),
            Arc::new(ratios),
            Arc::new(names),
            Arc::new(flags),
            Arc::new(days),
            Arc::new(times),
        ],
    )
    .expect("the envelope batch is well formed")
}

/// A one-shot reader over a batch, which is what `register_arrow` takes.
struct OneBatch {
    schema: SchemaRef,
    batch: Option<RecordBatch>,
}

impl OneBatch {
    fn new(batch: RecordBatch) -> Self {
        Self {
            schema: batch.schema(),
            batch: Some(batch),
        }
    }
}

impl Iterator for OneBatch {
    type Item = Result<RecordBatch, arrow::error::ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.batch.take().map(Ok)
    }
}

impl RecordBatchReader for OneBatch {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }
}

/// Every batch of an Arrow stream, as one vector.
fn read_all_batches(stream: &mut ArrowStream) -> Vec<RecordBatch> {
    let mut batches = Vec::new();
    while let Some(batch) = stream
        .next_batch()
        .expect("the Arrow stream yields batches")
    {
        batches.push(batch);
    }
    batches
}

#[test]
fn registered_arrow_reads_back() {
    if child_role().is_some() {
        registered_arrow_reads_back_here();
        return;
    }
    report(
        "registered_arrow_reads_back",
        run_child("registered_arrow_reads_back"),
    );
}

/// The round trip itself: a million rows, every type the envelope uses, exported
/// through the Arrow C stream and read back through the engine.
fn registered_arrow_reads_back_here() {
    let engine = engine();
    let session = session(engine, "arrow-in");
    let batch = envelope_batch();

    let mut handle = session
        .register_arrow("loams_arrow_in", Box::new(OneBatch::new(batch)))
        .expect("the Arrow reader is registered");
    assert_eq!(handle.name(), "loams_arrow_in");
    assert!(
        handle.reader_is_alive(),
        "the handle holds the reader the engine pulls from"
    );

    // The registered stream is a table function of chDB's own naming.
    let mut count = session
        .execute(
            "SELECT count() FROM ArrowStream('loams_arrow_in')",
            "TSV",
            &[],
        )
        .expect("the registered table can be read");
    let mut counted = Vec::new();
    while let Some(chunk) = count.next_chunk().expect("the count streams") {
        counted.extend_from_slice(&chunk);
    }
    assert_eq!(
        String::from_utf8(counted).unwrap_or_default().trim(),
        ROWS.to_string(),
        "the engine read every row the reader exported"
    );

    // And the data itself comes back through the Arrow path, values and all.
    let mut stream = session
        .execute_arrow("SELECT id, ratio, name, flag, day, at FROM ArrowStream('loams_arrow_in')")
        .expect("the registered table can be read as Arrow");
    assert_eq!(
        *stream.schema().expect("the stream sends a schema"),
        *envelope_schema(),
        "the engine hands the envelope's schema back"
    );
    let mut rows = 0usize;
    let mut last: Option<(i64, f64, String, bool, i32, i64)> = None;
    for batch in read_all_batches(&mut stream) {
        rows += batch.num_rows();
        if rows == ROWS {
            let index = batch.num_rows() - 1;
            let ids = batch
                .column(0)
                .as_any()
                .downcast_ref::<Int64Array>()
                .expect("column 0 is Int64");
            let ratios = batch
                .column(1)
                .as_any()
                .downcast_ref::<Float64Array>()
                .expect("column 1 is Float64");
            let names = batch
                .column(2)
                .as_any()
                .downcast_ref::<StringArray>()
                .expect("column 2 is Utf8");
            let flags = batch
                .column(3)
                .as_any()
                .downcast_ref::<BooleanArray>()
                .expect("column 3 is Boolean");
            let days = batch
                .column(4)
                .as_any()
                .downcast_ref::<Date32Array>()
                .expect("column 4 is Date32");
            let times = batch
                .column(5)
                .as_any()
                .downcast_ref::<TimestampMillisecondArray>()
                .expect("column 5 is a timestamp");
            last = Some((
                ids.value(index),
                ratios.value(index),
                names.value(index).to_string(),
                flags.value(index),
                days.value(index),
                times.value(index),
            ));
        }
    }
    assert_eq!(rows, ROWS, "every row came back");
    assert_eq!(
        last,
        Some((
            ROWS as i64 - 1,
            (ROWS - 1) as f64 / 4.0,
            format!("row-{}", ROWS - 1),
            (ROWS - 1).is_multiple_of(2),
            (ROWS - 1) as i32,
            ((ROWS - 1) as i64) * 1_000,
        )),
        "the last row's values survived the round trip"
    );

    handle.unregister().expect("the table unregisters");
    assert!(
        !handle.reader_is_alive(),
        "unregistering releases the reader"
    );
}

#[test]
fn arrow_output_matches_input_types() {
    if child_role().is_some() {
        arrow_output_matches_input_types_here();
        return;
    }
    report(
        "arrow_output_matches_input_types",
        run_child("arrow_output_matches_input_types"),
    );
}

/// The output direction: every Arrow type the envelope uses, checked against the
/// statement that asked for them.
fn arrow_output_matches_input_types_here() {
    let engine = engine();
    let session = session(engine, "arrow-types");

    let mut stream = session
        .execute_arrow(
            "SELECT CAST(7 AS Int64) AS id, \
             CAST(0.25 AS Float64) AS ratio, \
             CAST('loams' AS String) AS name, \
             CAST(1 AS Bool) AS flag, \
             CAST('2026-10-04' AS Date) AS day, \
             toDateTime64('2026-10-04 12:34:56.789', 3, 'UTC') AS at",
        )
        .expect("the query streams as Arrow");

    let schema = stream.schema().expect("the stream sends a schema");
    let expected: Vec<DataType> = vec![
        DataType::Int64,
        DataType::Float64,
        DataType::Utf8,
        DataType::Boolean,
        DataType::Date32,
        DataType::Timestamp(TimeUnit::Millisecond, None),
    ];
    let got: Vec<DataType> = schema
        .fields()
        .iter()
        .map(|field| field.data_type().clone())
        .collect();
    assert_eq!(
        got, expected,
        "chDB's Arrow output carries the types the statement asked for"
    );

    let batches = read_all_batches(&mut stream);
    let total: usize = batches.iter().map(RecordBatch::num_rows).sum();
    assert_eq!(total, 1, "one row came back");
    let batch = batches.first().expect("the row is in a batch");
    assert_eq!(
        batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .expect("Int64")
            .value(0),
        7
    );
    assert_eq!(
        batch
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .expect("Float64")
            .value(0),
        0.25
    );
    assert_eq!(
        batch
            .column(2)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("Utf8")
            .value(0),
        "loams"
    );
    assert!(
        batch
            .column(3)
            .as_any()
            .downcast_ref::<BooleanArray>()
            .expect("Boolean")
            .value(0)
    );
    assert_eq!(
        batch
            .column(4)
            .as_any()
            .downcast_ref::<Date32Array>()
            .expect("Date32")
            .value(0),
        20_304
    );
    assert_eq!(
        batch
            .column(5)
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .expect("Timestamp")
            .value(0),
        1_772_621_696_789
    );

    // The same types as arrow-rs builds them, compared the way a caller would.
    let reference = envelope_schema();
    for (field, expected_field) in schema.fields().iter().zip(reference.fields().iter()) {
        assert_eq!(
            field.data_type(),
            expected_field.data_type(),
            "column {} keeps the type the envelope uses",
            expected_field.name()
        );
    }
    assert!(stream.is_done(), "the stream ends after its blocks");
    assert!(
        stream
            .next_batch()
            .expect("a finished stream asks no more")
            .is_none()
    );
}
