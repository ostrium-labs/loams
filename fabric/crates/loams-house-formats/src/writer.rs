//! The [`FormatWriter`] interface and [`writer`], which picks the writer for a
//! format name.
//!
//! Every writer streams: [`FormatWriter::write_batch`] writes a batch's bytes
//! to the output before the next batch is computed, except where the format
//! itself needs the whole result first (`Pretty*` measures its columns'
//! widths over every row it shows, as ClickHouse's squashed Pretty does;
//! `Parquet` and `Arrow` (file) write a footer that indexes the whole file).

use std::io::Write;
use std::time::Duration;

use arrow_array::RecordBatch;
use loams_house_types::ChError;

use crate::Column;
use crate::settings::FormatSettings;

/// A query's figures for the formats that print them (`JSON*`'s `rows`,
/// `rows_before_limit_at_least` and `statistics`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// The rows a `LIMIT` saw before it cut, when the query had one.
    pub rows_before_limit_at_least: Option<u64>,
    /// Time from the query's start to its end.
    pub elapsed: Duration,
    /// Rows read from tables.
    pub rows_read: u64,
    /// Bytes read from tables.
    pub bytes_read: u64,
}

/// A result's writer in one format.
pub trait FormatWriter {
    /// Writes one block of rows (its columns are the writer's columns).
    ///
    /// # Errors
    ///
    /// A batch whose columns do not match, a value the format cannot write,
    /// or an output error (`210`).
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError>;

    /// Writes `WITH TOTALS`' row (one row). Formats without totals ignore it,
    /// as ClickHouse's do.
    ///
    /// # Errors
    ///
    /// As [`FormatWriter::write_batch`].
    fn write_totals(&mut self, totals: &RecordBatch) -> Result<(), ChError>;

    /// Writes `extremes = 1`'s two rows, the minimums then the maximums.
    /// Formats without extremes ignore them.
    ///
    /// # Errors
    ///
    /// As [`FormatWriter::write_batch`].
    fn write_extremes(&mut self, extremes: &RecordBatch) -> Result<(), ChError>;

    /// Ends the result: footers, `rows`, `statistics`, and flushes.
    ///
    /// # Errors
    ///
    /// As [`FormatWriter::write_batch`].
    fn finish(self: Box<Self>, stats: &Stats) -> Result<(), ChError>;
}

/// The output formats the engine writes (plan HE1 Task 36).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `JSONEachRow` (`JSONLines`, `NDJSON`) and `JSONStringsEachRow`.
    JsonEachRow {
        /// Every value as a JSON string of its text (`JSONStrings*`).
        strings: bool,
    },
    /// `JSON`, `JSONStrings`, `JSONCompact` and `JSONCompactStrings`.
    Json {
        /// Rows as arrays, not objects.
        compact: bool,
        /// Every value as a JSON string of its text.
        strings: bool,
    },
    /// `JSONCompactEachRow[WithNames[AndTypes]]` and its `Strings` forms.
    JsonCompactEachRow {
        /// Every value as a JSON string of its text.
        strings: bool,
        /// A first line of names.
        names: bool,
        /// A line of types after the names.
        types: bool,
    },
    /// `TabSeparated[Raw][WithNames[AndTypes]]` and the `TSV*` aliases.
    TabSeparated {
        /// Strings unescaped (`Raw`).
        raw: bool,
        /// A first line of names.
        names: bool,
        /// A line of types after the names.
        types: bool,
    },
    /// `CSV[WithNames[AndTypes]]`.
    Csv {
        /// A first line of names.
        names: bool,
        /// A line of types after the names.
        types: bool,
    },
    /// `Values`.
    Values,
    /// `RowBinary[WithNames[AndTypes]]`.
    RowBinary {
        /// A header of the column count and names.
        names: bool,
        /// The types after the names.
        types: bool,
    },
    /// `Native`.
    Native,
    /// `Parquet`.
    Parquet,
    /// `Arrow` (the IPC file format).
    Arrow,
    /// `ArrowStream` (the IPC stream format).
    ArrowStream,
    /// `Pretty*` and `PrettyCompact*`.
    Pretty {
        /// `PrettyCompact`: no lines between rows, names in the top border.
        compact: bool,
    },
    /// `Null`: nothing.
    Null,
}

impl Format {
    /// The format a `FORMAT` clause or `default_format` names. Names are
    /// case-insensitive, as in ClickHouse's format factory.
    ///
    /// # Errors
    ///
    /// `73 UNKNOWN_FORMAT` for a name ClickHouse does not have, `48` for one
    /// it has and the engine does not write yet.
    pub fn parse(name: &str) -> Result<Format, ChError> {
        let lower = name.trim().to_ascii_lowercase();
        let tsv = |raw, names, types| Format::TabSeparated { raw, names, types };
        Ok(match lower.as_str() {
            "jsoneachrow" | "jsonlines" | "ndjson" => Format::JsonEachRow { strings: false },
            "jsonstringseachrow" => Format::JsonEachRow { strings: true },
            "json" => Format::Json {
                compact: false,
                strings: false,
            },
            "jsonstrings" => Format::Json {
                compact: false,
                strings: true,
            },
            "jsoncompact" => Format::Json {
                compact: true,
                strings: false,
            },
            "jsoncompactstrings" => Format::Json {
                compact: true,
                strings: true,
            },
            "jsoncompacteachrow" => compact_each_row(false, false, false),
            "jsoncompacteachrowwithnames" => compact_each_row(false, true, false),
            "jsoncompacteachrowwithnamesandtypes" => compact_each_row(false, true, true),
            "jsoncompactstringseachrow" => compact_each_row(true, false, false),
            "jsoncompactstringseachrowwithnames" => compact_each_row(true, true, false),
            "jsoncompactstringseachrowwithnamesandtypes" => compact_each_row(true, true, true),
            "tabseparated" | "tsv" => tsv(false, false, false),
            "tabseparatedwithnames" | "tsvwithnames" => tsv(false, true, false),
            "tabseparatedwithnamesandtypes" | "tsvwithnamesandtypes" => tsv(false, true, true),
            "tabseparatedraw" | "tsvraw" | "raw" => tsv(true, false, false),
            "tabseparatedrawwithnames" | "tsvrawwithnames" | "rawwithnames" => {
                tsv(true, true, false)
            }
            "tabseparatedrawwithnamesandtypes"
            | "tsvrawwithnamesandtypes"
            | "rawwithnamesandtypes" => tsv(true, true, true),
            "csv" => Format::Csv {
                names: false,
                types: false,
            },
            "csvwithnames" => Format::Csv {
                names: true,
                types: false,
            },
            "csvwithnamesandtypes" => Format::Csv {
                names: true,
                types: true,
            },
            "values" => Format::Values,
            "rowbinary" => Format::RowBinary {
                names: false,
                types: false,
            },
            "rowbinarywithnames" => Format::RowBinary {
                names: true,
                types: false,
            },
            "rowbinarywithnamesandtypes" => Format::RowBinary {
                names: true,
                types: true,
            },
            "native" => Format::Native,
            "parquet" => Format::Parquet,
            "arrow" => Format::Arrow,
            "arrowstream" => Format::ArrowStream,
            "pretty" | "prettynoescapes" | "prettymonoblock" | "prettynoescapesmonoblock" => {
                Format::Pretty { compact: false }
            }
            "prettycompact"
            | "prettycompactnoescapes"
            | "prettycompactmonoblock"
            | "prettycompactnoescapesmonoblock" => Format::Pretty { compact: true },
            "null" => Format::Null,
            _ if KNOWN_ELSEWHERE.contains(&lower.as_str()) => {
                return Err(ChError::unsupported(format!("Output format {name}")));
            }
            _ => {
                return Err(ChError::unknown_format(format!(
                    "Unknown output format {name}"
                )));
            }
        })
    }

    /// The HTTP `Content-Type` ClickHouse answers with for the format.
    pub fn content_type(self) -> &'static str {
        match self {
            Format::JsonEachRow { .. } | Format::JsonCompactEachRow { .. } => {
                "application/x-ndjson; charset=UTF-8"
            }
            Format::Json { .. } => "application/json; charset=UTF-8",
            Format::TabSeparated { .. } => "text/tab-separated-values; charset=UTF-8",
            Format::Csv { names: false, .. } => "text/csv; charset=UTF-8; header=absent",
            Format::Csv { names: true, .. } => "text/csv; charset=UTF-8; header=present",
            Format::Values | Format::Pretty { .. } | Format::Null => "text/plain; charset=UTF-8",
            Format::RowBinary { .. }
            | Format::Native
            | Format::Parquet
            | Format::Arrow
            | Format::ArrowStream => "application/octet-stream",
        }
    }
}

fn compact_each_row(strings: bool, names: bool, types: bool) -> Format {
    Format::JsonCompactEachRow {
        strings,
        names,
        types,
    }
}

/// Output formats ClickHouse has that the engine does not write yet: `48`,
/// not `73`, so a client sees "not supported" rather than "unknown".
const KNOWN_ELSEWHERE: &[&str] = &[
    "vertical",
    "prettyspace",
    "prettyspacenoescapes",
    "prettyspacemonoblock",
    "markdown",
    "md",
    "xml",
    "tskv",
    "orc",
    "avro",
    "protobuf",
    "protobufsingle",
    "capnproto",
    "msgpack",
    "bsoneachrow",
    "jsonobjecteachrow",
    "jsoncolumns",
    "jsoncompactcolumns",
    "jsoncolumnswithmetadata",
    "jsoneachrowwithprogress",
    "jsonstringseachrowwithprogress",
    "customseparated",
    "customseparatedwithnames",
    "customseparatedwithnamesandtypes",
    "template",
    "sqlinsert",
    "npy",
    "linesasstring",
    "rawblob",
    "prometheus",
    "one",
    "form",
];

/// The writer of `format` over `columns`, writing to `out`.
///
/// # Errors
///
/// An unknown format (`73`), or one the engine does not write (`48`).
pub fn writer<'a, W: Write + Send + 'a>(
    format: &str,
    columns: &[Column],
    settings: &FormatSettings,
    out: W,
) -> Result<Box<dyn FormatWriter + Send + 'a>, ChError> {
    writer_for(Format::parse(format)?, columns, settings, out)
}

/// The writer of a parsed [`Format`].
///
/// # Errors
///
/// A column the format cannot carry, found before any byte is written.
pub fn writer_for<'a, W: Write + Send + 'a>(
    format: Format,
    columns: &[Column],
    settings: &FormatSettings,
    out: W,
) -> Result<Box<dyn FormatWriter + Send + 'a>, ChError> {
    let columns = columns.to_vec();
    let settings = settings.clone();
    Ok(match format {
        Format::JsonEachRow { .. } | Format::Json { .. } | Format::JsonCompactEachRow { .. } => {
            Box::new(crate::json::JsonWriter::new(
                format, columns, settings, out,
            )?)
        }
        Format::TabSeparated { .. } | Format::Csv { .. } | Format::Values => Box::new(
            crate::text::TextWriter::new(format, columns, settings, out)?,
        ),
        Format::RowBinary { names, types } => Box::new(crate::binary::RowBinaryWriter::new(
            names, types, columns, settings, out,
        )?),
        Format::Native => Box::new(crate::native::NativeWriter::new(columns, settings, out)?),
        Format::Parquet => Box::new(crate::parquet::ParquetWriter::new(columns, out)?),
        Format::Arrow => Box::new(crate::arrow::ArrowWriter::new(columns, out, true)?),
        Format::ArrowStream => Box::new(crate::arrow::ArrowWriter::new(columns, out, false)?),
        Format::Pretty { compact } => Box::new(crate::pretty::PrettyWriter::new(
            compact, columns, settings, out,
        )),
        Format::Null => Box::new(NullWriter),
    })
}

/// Writes all of a result: `batches`, then `finish`.
///
/// # Errors
///
/// As [`writer`] and [`FormatWriter`].
pub fn write_all<W: Write + Send>(
    format: &str,
    columns: &[Column],
    settings: &FormatSettings,
    batches: &[RecordBatch],
    stats: &Stats,
    out: W,
) -> Result<(), ChError> {
    let mut w = writer(format, columns, settings, out)?;
    for batch in batches {
        w.write_batch(batch)?;
    }
    w.finish(stats)
}

/// `Null`: reads the result and writes nothing.
#[derive(Debug)]
struct NullWriter;

impl FormatWriter for NullWriter {
    fn write_batch(&mut self, _batch: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn write_totals(&mut self, _totals: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn write_extremes(&mut self, _extremes: &RecordBatch) -> Result<(), ChError> {
        Ok(())
    }

    fn finish(self: Box<Self>, _stats: &Stats) -> Result<(), ChError> {
        Ok(())
    }
}

/// An output error as ClickHouse reports a failed write.
pub(crate) fn io_error(e: std::io::Error) -> ChError {
    ChError::network_error(format!("Cannot write the result: {e}"))
}

/// Checks that `batch` has the writer's columns.
pub(crate) fn check_batch(columns: &[Column], batch: &RecordBatch) -> Result<(), ChError> {
    if batch.num_columns() != columns.len() {
        return Err(ChError::logical_error(format!(
            "a batch of {} columns for {} output columns",
            batch.num_columns(),
            columns.len()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn format_names_are_case_insensitive_with_aliases() {
        assert_eq!(
            Format::parse("tsv").unwrap(),
            Format::TabSeparated {
                raw: false,
                names: false,
                types: false
            }
        );
        assert_eq!(
            Format::parse("NDJSON").unwrap(),
            Format::JsonEachRow { strings: false }
        );
        assert_eq!(
            Format::parse("JSONCompactEachRowWithNamesAndTypes").unwrap(),
            compact_each_row(false, true, true)
        );
        assert_eq!(
            Format::parse("TSVRawWithNames").unwrap(),
            Format::TabSeparated {
                raw: true,
                names: true,
                types: false
            }
        );
    }

    #[test]
    fn unknown_and_unwritten_formats_have_their_codes() {
        assert_eq!(Format::parse("Nope").unwrap_err().code, 73);
        assert_eq!(Format::parse("Vertical").unwrap_err().code, 48);
        assert_eq!(Format::parse("ORC").unwrap_err().code, 48);
    }
}
