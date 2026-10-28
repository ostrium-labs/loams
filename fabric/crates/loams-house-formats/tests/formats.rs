//! Golden output per format (plan HE1 Task 36).
//!
//! The expected bytes here are written from ClickHouse's output format code
//! and the value text `loams-house-types` already checks against the oracle;
//! the oracle corpus `he1/36` (`-- oracle: format = <F>`) is the authority,
//! compared byte for byte by `scripts/house/engine-oracle.sh`. The native
//! fixtures (`SELECT 1`, the `LowCardinality` granule) are oracle captures
//! from `loams-house-native`'s tests.

#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::types::{Int32Type, UInt8Type};
use arrow_array::{
    ArrayRef, Date32Array, DictionaryArray, Float64Array, ListArray, RecordBatch, StringArray,
    TimestampSecondArray, UInt8Array, UInt32Array,
};
use arrow_schema::{Field, Schema};
use loams_house_formats::{FormatSettings, Stats, columns, write_all};
use loams_house_native::block::{Block, Framing};
use loams_house_native::wire::Reader;
use loams_house_types::arrow::CH_TYPE;

/// Eight columns of two rows: `id UInt32, s String, ns Nullable(String),
/// f Float64, d Date, dt DateTime, arr Array(UInt8),
/// lc LowCardinality(Nullable(String))`.
fn fixture() -> RecordBatch {
    let arrays: Vec<(&str, &str, ArrayRef)> = vec![
        ("id", "UInt32", Arc::new(UInt32Array::from(vec![1, 2]))),
        (
            "s",
            "String",
            Arc::new(StringArray::from(vec!["a\tb", "it's"])),
        ),
        (
            "ns",
            "Nullable(String)",
            Arc::new(StringArray::from(vec![None, Some("q/")])),
        ),
        (
            "f",
            "Float64",
            Arc::new(Float64Array::from(vec![0.5, -1.5])),
        ),
        ("d", "Date", Arc::new(Date32Array::from(vec![20749, 0]))),
        (
            "dt",
            "DateTime",
            Arc::new(TimestampSecondArray::from(vec![1_792_717_323, 0])),
        ),
        (
            "arr",
            "Array(UInt8)",
            Arc::new(ListArray::from_iter_primitive::<UInt8Type, _, _>(vec![
                Some(vec![Some(1), Some(2)]),
                Some(vec![]),
            ])),
        ),
        (
            "lc",
            "LowCardinality(Nullable(String))",
            Arc::new(
                vec![Some("x"), None]
                    .into_iter()
                    .collect::<DictionaryArray<Int32Type>>(),
            ),
        ),
    ];
    let fields: Vec<Field> = arrays
        .iter()
        .map(|(name, ty, a)| {
            Field::new(*name, a.data_type().clone(), true)
                .with_metadata(HashMap::from([(CH_TYPE.to_string(), (*ty).to_string())]))
        })
        .collect();
    RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        arrays.into_iter().map(|(_, _, a)| a).collect(),
    )
    .unwrap()
}

fn render(format: &str, batch: &RecordBatch, settings: &FormatSettings) -> Vec<u8> {
    let cols = columns(&batch.schema()).unwrap();
    let mut out = Vec::new();
    write_all(
        format,
        &cols,
        settings,
        std::slice::from_ref(batch),
        &Stats::default(),
        &mut out,
    )
    .unwrap();
    out
}

fn text(format: &str) -> String {
    String::from_utf8(render(format, &fixture(), &FormatSettings::default())).unwrap()
}

const NAMES: &str = "id\ts\tns\tf\td\tdt\tarr\tlc\n";
const TYPES: &str = "UInt32\tString\tNullable(String)\tFloat64\tDate\tDateTime\tArray(UInt8)\tLowCardinality(Nullable(String))\n";
const TSV_ROWS: &str = "1\ta\\tb\t\\N\t0.5\t2026-10-23\t2026-10-23 01:02:03\t[1,2]\tx\n\
                        2\tit\\'s\tq/\t-1.5\t1970-01-01\t1970-01-01 00:00:00\t[]\t\\N\n";

#[test]
fn tab_separated_family() {
    assert_eq!(text("TabSeparated"), TSV_ROWS);
    assert_eq!(text("TSV"), TSV_ROWS);
    assert_eq!(text("TSVWithNames"), format!("{NAMES}{TSV_ROWS}"));
    assert_eq!(
        text("TabSeparatedWithNamesAndTypes"),
        format!("{NAMES}{TYPES}{TSV_ROWS}")
    );
    assert_eq!(
        text("TabSeparatedRaw"),
        "1\ta\tb\t\\N\t0.5\t2026-10-23\t2026-10-23 01:02:03\t[1,2]\tx\n\
         2\tit's\tq/\t-1.5\t1970-01-01\t1970-01-01 00:00:00\t[]\t\\N\n"
    );
}

#[test]
fn csv_family() {
    let rows = "1,\"a\tb\",\\N,0.5,\"2026-10-23\",\"2026-10-23 01:02:03\",\"[1,2]\",\"x\"\n\
                2,\"it's\",\"q/\",-1.5,\"1970-01-01\",\"1970-01-01 00:00:00\",\"[]\",\\N\n";
    assert_eq!(text("CSV"), rows);
    assert_eq!(
        text("CSVWithNames"),
        format!("\"id\",\"s\",\"ns\",\"f\",\"d\",\"dt\",\"arr\",\"lc\"\n{rows}")
    );
}

#[test]
fn values() {
    assert_eq!(
        text("Values"),
        "(1,'a\\tb',NULL,0.5,'2026-10-23','2026-10-23 01:02:03',[1,2],'x'),\
         (2,'it\\'s','q/',-1.5,'1970-01-01','1970-01-01 00:00:00',[],NULL)"
    );
}

#[test]
fn json_each_row_family() {
    assert_eq!(
        text("JSONEachRow"),
        "{\"id\":1,\"s\":\"a\\tb\",\"ns\":null,\"f\":0.5,\"d\":\"2026-10-23\",\"dt\":\"2026-10-23 01:02:03\",\"arr\":[1,2],\"lc\":\"x\"}\n\
         {\"id\":2,\"s\":\"it's\",\"ns\":\"q\\/\",\"f\":-1.5,\"d\":\"1970-01-01\",\"dt\":\"1970-01-01 00:00:00\",\"arr\":[],\"lc\":null}\n"
    );
    assert_eq!(
        text("JSONCompactEachRow"),
        "[1,\"a\\tb\",null,0.5,\"2026-10-23\",\"2026-10-23 01:02:03\",[1,2],\"x\"]\n\
         [2,\"it's\",\"q\\/\",-1.5,\"1970-01-01\",\"1970-01-01 00:00:00\",[],null]\n"
    );
    assert_eq!(
        text("JSONStringsEachRow"),
        "{\"id\":\"1\",\"s\":\"a\\tb\",\"ns\":\"ᴺᵁᴸᴸ\",\"f\":\"0.5\",\"d\":\"2026-10-23\",\"dt\":\"2026-10-23 01:02:03\",\"arr\":\"[1,2]\",\"lc\":\"x\"}\n\
         {\"id\":\"2\",\"s\":\"it's\",\"ns\":\"q\\/\",\"f\":\"-1.5\",\"d\":\"1970-01-01\",\"dt\":\"1970-01-01 00:00:00\",\"arr\":\"[]\",\"lc\":\"ᴺᵁᴸᴸ\"}\n"
    );
    assert!(
        text("JSONCompactEachRowWithNames")
            .starts_with("[\"id\",\"s\",\"ns\",\"f\",\"d\",\"dt\",\"arr\",\"lc\"]\n[1,")
    );
}

fn two_columns() -> RecordBatch {
    let batch = fixture();
    batch.project(&[0, 2]).unwrap()
}

#[test]
fn json_document() {
    let out = render("JSON", &two_columns(), &FormatSettings::default());
    let expected = "{\n\
\t\"meta\":\n\
\t[\n\
\t\t{\n\
\t\t\t\"name\": \"id\",\n\
\t\t\t\"type\": \"UInt32\"\n\
\t\t},\n\
\t\t{\n\
\t\t\t\"name\": \"ns\",\n\
\t\t\t\"type\": \"Nullable(String)\"\n\
\t\t}\n\
\t],\n\
\n\
\t\"data\":\n\
\t[\n\
\t\t{\n\
\t\t\t\"id\": 1,\n\
\t\t\t\"ns\": null\n\
\t\t},\n\
\t\t{\n\
\t\t\t\"id\": 2,\n\
\t\t\t\"ns\": \"q\\/\"\n\
\t\t}\n\
\t],\n\
\n\
\t\"rows\": 2,\n\
\n\
\t\"statistics\":\n\
\t{\n\
\t\t\"elapsed\": 0,\n\
\t\t\"rows_read\": 0,\n\
\t\t\"bytes_read\": 0\n\
\t}\n\
}\n";
    assert_eq!(String::from_utf8(out.clone()).unwrap(), expected);
    let stripped = loams_house_formats::json::strip_statistics(&out);
    assert!(
        String::from_utf8(stripped)
            .unwrap()
            .ends_with("\t\"rows\": 2\n}\n")
    );
}

#[test]
fn json_compact_document_with_an_empty_result() {
    let batch = two_columns().slice(0, 0);
    let settings = FormatSettings {
        write_statistics: false,
        ..FormatSettings::default()
    };
    let out = render("JSONCompact", &batch, &settings);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .ends_with("\t\"data\":\n\t[\n\n\t],\n\n\t\"rows\": 0\n}\n")
    );
    let out = render("JSONCompact", &two_columns(), &settings);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains("\t[\n\t\t[1, null],\n\t\t[2, \"q\\/\"]\n\t],")
    );
}

#[test]
fn row_binary() {
    let mut expected = Vec::new();
    // Row 1.
    expected.extend(1u32.to_le_bytes());
    expected.extend([3, b'a', b'\t', b'b']);
    expected.push(1);
    expected.extend(0.5f64.to_le_bytes());
    expected.extend(20749u16.to_le_bytes());
    expected.extend(1_792_717_323u32.to_le_bytes());
    expected.extend([2, 1, 2]);
    expected.extend([0, 1, b'x']);
    // Row 2.
    expected.extend(2u32.to_le_bytes());
    expected.extend([4, b'i', b't', b'\'', b's']);
    expected.extend([0, 2, b'q', b'/']);
    expected.extend((-1.5f64).to_le_bytes());
    expected.extend(0u16.to_le_bytes());
    expected.extend(0u32.to_le_bytes());
    expected.push(0);
    expected.push(1);
    assert_eq!(
        render("RowBinary", &fixture(), &FormatSettings::default()),
        expected
    );
    let with_names = render(
        "RowBinaryWithNamesAndTypes",
        &fixture(),
        &FormatSettings::default(),
    );
    assert_eq!(&with_names[..4], &[8, 2, b'i', b'd']);
    assert!(with_names.ends_with(&expected));
}

#[test]
fn native_blocks_match_the_oracle() {
    // `SELECT 1 FORMAT Native` on clickhouse-server 26.9.2.8.
    let one = RecordBatch::try_new(
        Arc::new(Schema::new(vec![Field::new(
            "1",
            arrow_schema::DataType::UInt8,
            false,
        )])),
        vec![Arc::new(UInt8Array::from(vec![1]))],
    )
    .unwrap();
    assert_eq!(
        render("Native", &one, &FormatSettings::default()),
        [
            0x01, 0x01, 0x01, 0x31, 0x05, b'U', b'I', b'n', b't', b'8', 0x01
        ]
    );

    // The fixture decodes with the codec, column by column.
    let bytes = render("Native", &fixture(), &FormatSettings::default());
    let mut reader = Reader::new(&bytes);
    let block = Block::read(&mut reader, Framing::NATIVE_FORMAT).unwrap();
    assert_eq!(reader.remaining().len(), 0);
    assert_eq!(block.rows, 2);
    let names: Vec<&str> = block.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "s", "ns", "f", "d", "dt", "arr", "lc"]);
    let data = |name: &str| {
        block
            .columns
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .data
            .clone()
    };
    assert_eq!(data("ns"), [1, 0, 0, 2, b'q', b'/']);
    let mut arr = Vec::new();
    arr.extend(2u64.to_le_bytes());
    arr.extend(2u64.to_le_bytes());
    arr.extend([1, 2]);
    assert_eq!(data("arr"), arr);
    // LowCardinality(Nullable(String)) of ['x', NULL]: version 1, index type
    // UInt8 with additional keys, dictionary [NULL's placeholder, '', 'x'],
    // keys [2, 0] (the oracle-captured layout of `loams-house-native`).
    let mut lc = Vec::new();
    lc.extend(1u64.to_le_bytes());
    lc.extend(0x600u64.to_le_bytes());
    lc.extend(3u64.to_le_bytes());
    lc.extend([0, 0, 1, b'x']);
    lc.extend(2u64.to_le_bytes());
    lc.extend([2, 0]);
    assert_eq!(data("lc"), lc);
}

#[test]
fn native_for_old_clients_has_no_low_cardinality() {
    let settings = FormatSettings {
        client_revision: Some(54213),
        ..FormatSettings::default()
    };
    let batch = fixture().project(&[7]).unwrap();
    let block = loams_house_formats::native::encode_block(
        &columns(&batch.schema()).unwrap(),
        &batch,
        &settings,
    )
    .unwrap();
    assert_eq!(block.columns[0].type_name, "Nullable(String)");
    assert_eq!(block.columns[0].data, [0, 1, 1, b'x', 0]);
}

#[test]
fn parquet_reads_back_identically() {
    let batch = fixture();
    let bytes = render("Parquet", &batch, &FormatSettings::default());
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
        bytes::Bytes::from(bytes),
    )
    .unwrap()
    .build()
    .unwrap();
    let back: Vec<RecordBatch> = reader.map(Result::unwrap).collect();
    assert_eq!(back.len(), 1);
    // Same columns, same ClickHouse types, same values in every format.
    assert_eq!(
        columns(&back[0].schema()).unwrap(),
        columns(&batch.schema()).unwrap()
    );
    for format in ["TSV", "JSONEachRow", "RowBinary"] {
        assert_eq!(
            render(format, &back[0], &FormatSettings::default()),
            render(format, &batch, &FormatSettings::default()),
            "{format}"
        );
    }
}

#[test]
fn arrow_stream_reads_back_identically() {
    let batch = fixture();
    for format in ["ArrowStream", "Arrow"] {
        let bytes = render(format, &batch, &FormatSettings::default());
        let back: Vec<RecordBatch> = if format == "Arrow" {
            arrow_ipc::reader::FileReader::try_new(std::io::Cursor::new(bytes), None)
                .unwrap()
                .map(Result::unwrap)
                .collect()
        } else {
            arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None)
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(back.len(), 1, "{format}");
        assert_eq!(
            render("TSV", &back[0], &FormatSettings::default()),
            render("TSV", &batch, &FormatSettings::default()),
            "{format}"
        );
    }
}

#[test]
fn pretty_compact_and_pretty() {
    let batch = fixture().project(&[0, 2]).unwrap();
    let compact =
        String::from_utf8(render("PrettyCompact", &batch, &FormatSettings::default())).unwrap();
    assert_eq!(
        compact,
        "   ┌─id─┬─ns───┐\n\
         1. │  1 │ ᴺᵁᴸᴸ │\n\
         2. │  2 │ q/   │\n   \
            └────┴──────┘\n"
    );
    let pretty = String::from_utf8(render("Pretty", &batch, &FormatSettings::default())).unwrap();
    assert_eq!(
        pretty,
        "   ┏━━━━┳━━━━━━┓\n   \
            ┃ id ┃ ns   ┃\n   \
            ┡━━━━╇━━━━━━┩\n\
         1. │  1 │ ᴺᵁᴸᴸ │\n   \
            ├────┼──────┤\n\
         2. │  2 │ q/   │\n   \
            └────┴──────┘\n"
    );
}

#[test]
fn settings_change_the_text() {
    use loams_house_types::text::DateTimeOutput;
    let batch = fixture().project(&[5]).unwrap();
    let mut settings = FormatSettings::default();
    settings.text.date_time_output = DateTimeOutput::UnixTimestamp;
    assert_eq!(
        String::from_utf8(render("TSV", &batch, &settings)).unwrap(),
        "1792717323\n0\n"
    );
    settings.text.date_time_output = DateTimeOutput::Iso;
    assert_eq!(
        String::from_utf8(render("JSONEachRow", &batch, &settings)).unwrap(),
        "{\"dt\":\"2026-10-23T01:02:03Z\"}\n{\"dt\":\"1970-01-01T00:00:00Z\"}\n"
    );
}

#[test]
fn null_format_writes_nothing() {
    assert!(render("Null", &fixture(), &FormatSettings::default()).is_empty());
}
