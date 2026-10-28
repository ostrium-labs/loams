//! The `JSON*` formats: `JSON`, `JSONCompact`, `JSONEachRow`,
//! `JSONCompactEachRow[WithNames[AndTypes]]` and their `Strings` forms.
//!
//! Laid out as ClickHouse's `JSONRowOutputFormat`,
//! `JSONCompactRowOutputFormat`, `JSONEachRowRowOutputFormat`,
//! `JSONCompactEachRowRowOutputFormat` and `JSONUtils` write them
//! (`src/Processors/Formats/Impl/`, `src/Formats/JSONUtils.cpp`, Apache-2.0):
//! `JSON` and `JSONCompact` indent with tabs and end with `rows`,
//! `rows_before_limit_at_least` and `statistics`; the `EachRow` forms write
//! one line per row without spaces.
//!
//! Values are `loams_house_types::text`'s JSON style; the `Strings` forms
//! write each value's text (`serializeText`, so `NULL` is `ᴺᵁᴸᴸ`) as a JSON
//! string.

use std::io::Write;

use arrow_array::{Array, RecordBatch};
use loams_house_types::ChError;
use loams_house_types::text::{Style, float64, json_string, write_value};

use crate::Column;
use crate::settings::FormatSettings;
use crate::writer::{Format, FormatWriter, Stats, check_batch, io_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// `JSON`/`JSONStrings`: an object per row inside the document.
    Document,
    /// `JSONCompact`/`JSONCompactStrings`: an array per row inside the
    /// document.
    CompactDocument,
    /// `JSONEachRow`: an object per line.
    EachRow,
    /// `JSONCompactEachRow*`: an array per line.
    CompactEachRow,
}

/// A writer of the `JSON*` formats.
pub(crate) struct JsonWriter<W: Write> {
    layout: Layout,
    strings: bool,
    columns: Vec<Column>,
    settings: FormatSettings,
    out: W,
    rows: u64,
    buf: Vec<u8>,
    /// The document's `totals` and `extremes`, written after `data`.
    totals: Option<Vec<u8>>,
    extremes: Option<Vec<u8>>,
}

impl<W: Write> std::fmt::Debug for JsonWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonWriter")
            .field("layout", &self.layout)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

fn tabs(n: usize, out: &mut Vec<u8>) {
    out.extend(std::iter::repeat_n(b'\t', n));
}

/// `JSONUtils::writeTitle`: `<indent>"title":<after>`.
fn title(name: &str, indent: usize, after: &str, settings: &FormatSettings, out: &mut Vec<u8>) {
    tabs(indent, out);
    json_string(name.as_bytes(), &settings.text, out);
    out.push(b':');
    out.extend_from_slice(after.as_bytes());
}

impl<W: Write> JsonWriter<W> {
    pub(crate) fn new(
        format: Format,
        columns: Vec<Column>,
        settings: FormatSettings,
        out: W,
    ) -> Result<Self, ChError> {
        let (layout, strings, names, types) = match format {
            Format::Json {
                compact: false,
                strings,
            } => (Layout::Document, strings, false, false),
            Format::Json {
                compact: true,
                strings,
            } => (Layout::CompactDocument, strings, false, false),
            Format::JsonEachRow { strings } => (Layout::EachRow, strings, false, false),
            Format::JsonCompactEachRow {
                strings,
                names,
                types,
            } => (Layout::CompactEachRow, strings, names, types),
            other => {
                return Err(ChError::logical_error(format!(
                    "the JSON writer asked for {other:?}"
                )));
            }
        };
        let mut w = Self {
            layout,
            strings,
            columns,
            settings,
            out,
            rows: 0,
            buf: Vec::new(),
            totals: None,
            extremes: None,
        };
        match layout {
            Layout::Document | Layout::CompactDocument => w.prefix(),
            Layout::CompactEachRow => {
                if names {
                    let names: Vec<String> = w.columns.iter().map(|c| c.name.clone()).collect();
                    w.header_line(&names);
                }
                if types {
                    // Types on one line: ClickHouse writes `Tuple(a UInt8, b
                    // String)` here (measured), not `toTypeName`'s multi-line
                    // form.
                    let types: Vec<String> =
                        w.columns.iter().map(|c| c.ty.to_compact_string()).collect();
                    w.header_line(&types);
                }
            }
            Layout::EachRow => {}
        }
        w.flush_buf()?;
        Ok(w)
    }

    fn header_line(&mut self, items: &[String]) {
        self.buf.push(b'[');
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                self.buf.push(b',');
            }
            json_string(item.as_bytes(), &self.settings.text, &mut self.buf);
        }
        self.buf.extend_from_slice(b"]\n");
    }

    /// `{`, `meta`, and the start of `data`.
    fn prefix(&mut self) {
        let s = &self.settings;
        let out = &mut self.buf;
        out.extend_from_slice(b"{\n");
        title("meta", 1, "\n", s, out);
        tabs(1, out);
        out.extend_from_slice(b"[\n");
        for (i, c) in self.columns.iter().enumerate() {
            tabs(2, out);
            out.extend_from_slice(b"{\n");
            title("name", 3, " ", s, out);
            json_string(c.name.as_bytes(), &s.text, out);
            out.extend_from_slice(b",\n");
            title("type", 3, " ", s, out);
            json_string(c.ty.to_compact_string().as_bytes(), &s.text, out);
            out.push(b'\n');
            tabs(2, out);
            out.push(b'}');
            if i + 1 < self.columns.len() {
                out.extend_from_slice(b",\n");
            }
        }
        out.push(b'\n');
        tabs(1, out);
        out.push(b']');
        out.extend_from_slice(b",\n\n");
        title("data", 1, "\n", s, out);
        tabs(1, out);
        out.extend_from_slice(b"[\n");
    }

    fn flush_buf(&mut self) -> Result<(), ChError> {
        if !self.buf.is_empty() {
            self.out.write_all(&self.buf).map_err(io_error)?;
            self.buf.clear();
        }
        Ok(())
    }

    fn value(
        &self,
        c: &Column,
        array: &dyn Array,
        row: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ChError> {
        if self.strings {
            let mut text = Vec::new();
            write_value(
                &c.ty,
                array,
                row,
                Style::Text,
                &self.settings.text,
                &mut text,
            )?;
            json_string(&text, &self.settings.text, out);
            Ok(())
        } else {
            write_value(&c.ty, array, row, Style::Json, &self.settings.text, out)
        }
    }

    /// One row's fields in `layout`, keyed at `indent` for the document's
    /// objects.
    fn row(
        &self,
        batch: &RecordBatch,
        row: usize,
        layout: Layout,
        indent: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ChError> {
        for (n, c) in self.columns.iter().enumerate() {
            let array = batch.column(n).as_ref();
            match layout {
                Layout::Document => {
                    if n > 0 {
                        out.extend_from_slice(b",\n");
                    }
                    title(&c.name, indent, " ", &self.settings, out);
                }
                Layout::CompactDocument => {
                    if n > 0 {
                        out.extend_from_slice(b", ");
                    }
                }
                Layout::EachRow => {
                    if n > 0 {
                        out.push(b',');
                    }
                    json_string(c.name.as_bytes(), &self.settings.text, out);
                    out.push(b':');
                }
                Layout::CompactEachRow => {
                    if n > 0 {
                        out.push(b',');
                    }
                }
            }
            self.value(c, array, row, out)?;
        }
        Ok(())
    }

    /// A whole row as a line or a `data` element.
    fn data_row(&self, batch: &RecordBatch, row: usize, out: &mut Vec<u8>) -> Result<(), ChError> {
        match self.layout {
            Layout::Document => {
                tabs(2, out);
                out.extend_from_slice(b"{\n");
                self.row(batch, row, Layout::Document, 3, out)?;
                out.push(b'\n');
                tabs(2, out);
                out.push(b'}');
            }
            Layout::CompactDocument => {
                tabs(2, out);
                out.push(b'[');
                self.row(batch, row, Layout::CompactDocument, 0, out)?;
                out.push(b']');
            }
            Layout::EachRow => {
                out.push(b'{');
                self.row(batch, row, Layout::EachRow, 0, out)?;
                out.extend_from_slice(b"}\n");
            }
            Layout::CompactEachRow => {
                out.push(b'[');
                self.row(batch, row, Layout::CompactEachRow, 0, out)?;
                out.extend_from_slice(b"]\n");
            }
        }
        Ok(())
    }

    /// `totals` or one of `extremes`' `min`/`max` in the document: an object
    /// (`JSON`) or an array (`JSONCompact`) titled `name` at `indent`.
    fn titled_row(
        &self,
        name: &str,
        indent: usize,
        batch: &RecordBatch,
        row: usize,
        out: &mut Vec<u8>,
    ) -> Result<(), ChError> {
        match self.layout {
            Layout::Document => {
                title(name, indent, "\n", &self.settings, out);
                tabs(indent, out);
                out.extend_from_slice(b"{\n");
                self.row(batch, row, Layout::Document, indent + 1, out)?;
                out.push(b'\n');
                tabs(indent, out);
                out.push(b'}');
            }
            _ => {
                title(name, indent, " ", &self.settings, out);
                out.push(b'[');
                self.row(batch, row, Layout::CompactDocument, 0, out)?;
                out.push(b']');
            }
        }
        Ok(())
    }
}

impl<W: Write + Send> FormatWriter for JsonWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, batch)?;
        let mut out = std::mem::take(&mut self.buf);
        for row in 0..batch.num_rows() {
            if self.rows > 0 && matches!(self.layout, Layout::Document | Layout::CompactDocument) {
                out.extend_from_slice(b",\n");
            }
            self.data_row(batch, row, &mut out)?;
            self.rows += 1;
        }
        self.buf = out;
        self.flush_buf()
    }

    fn write_totals(&mut self, totals: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, totals)?;
        if totals.num_rows() == 0 {
            return Ok(());
        }
        let mut out = Vec::new();
        match self.layout {
            Layout::Document | Layout::CompactDocument => {
                self.titled_row("totals", 1, totals, 0, &mut out)?;
                self.totals = Some(out);
            }
            // `JSONCompactEachRow`: a blank line, then the row (as the TSV
            // family writes totals). `JSONEachRow` has none.
            Layout::CompactEachRow => {
                out.push(b'\n');
                self.data_row(totals, 0, &mut out)?;
                self.out.write_all(&out).map_err(io_error)?;
            }
            Layout::EachRow => {}
        }
        Ok(())
    }

    fn write_extremes(&mut self, extremes: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, extremes)?;
        if extremes.num_rows() < 2 {
            return Ok(());
        }
        let mut out = Vec::new();
        match self.layout {
            Layout::Document | Layout::CompactDocument => {
                title("extremes", 1, "\n", &self.settings, &mut out);
                tabs(1, &mut out);
                out.extend_from_slice(b"{\n");
                self.titled_row("min", 2, extremes, 0, &mut out)?;
                out.extend_from_slice(b",\n");
                self.titled_row("max", 2, extremes, 1, &mut out)?;
                out.push(b'\n');
                tabs(1, &mut out);
                out.push(b'}');
                self.extremes = Some(out);
            }
            Layout::CompactEachRow => {
                out.push(b'\n');
                self.data_row(extremes, 0, &mut out)?;
                self.data_row(extremes, 1, &mut out)?;
                self.out.write_all(&out).map_err(io_error)?;
            }
            Layout::EachRow => {}
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>, stats: &Stats) -> Result<(), ChError> {
        if matches!(self.layout, Layout::Document | Layout::CompactDocument) {
            let mut out = std::mem::take(&mut self.buf);
            out.push(b'\n');
            tabs(1, &mut out);
            out.push(b']');
            for part in [self.totals.take(), self.extremes.take()]
                .into_iter()
                .flatten()
            {
                out.extend_from_slice(b",\n\n");
                out.extend_from_slice(&part);
            }
            out.extend_from_slice(b",\n\n");
            title("rows", 1, " ", &self.settings, &mut out);
            out.extend_from_slice(self.rows.to_string().as_bytes());
            if let Some(n) = stats.rows_before_limit_at_least {
                out.extend_from_slice(b",\n\n");
                title(
                    "rows_before_limit_at_least",
                    1,
                    " ",
                    &self.settings,
                    &mut out,
                );
                out.extend_from_slice(n.to_string().as_bytes());
            }
            if self.settings.write_statistics {
                out.extend_from_slice(b",\n\n");
                title("statistics", 1, "\n", &self.settings, &mut out);
                tabs(1, &mut out);
                out.extend_from_slice(b"{\n");
                title("elapsed", 2, " ", &self.settings, &mut out);
                out.extend_from_slice(float64(stats.elapsed.as_secs_f64()).as_bytes());
                out.extend_from_slice(b",\n");
                title("rows_read", 2, " ", &self.settings, &mut out);
                out.extend_from_slice(stats.rows_read.to_string().as_bytes());
                out.extend_from_slice(b",\n");
                title("bytes_read", 2, " ", &self.settings, &mut out);
                out.extend_from_slice(stats.bytes_read.to_string().as_bytes());
                out.push(b'\n');
                tabs(1, &mut out);
                out.push(b'}');
            }
            out.extend_from_slice(b"\n}\n");
            self.buf = out;
            self.flush_buf()?;
        }
        self.out.flush().map_err(io_error)
    }
}

/// Removes the `statistics` object from a `JSON*` document (and the
/// delimiter before it), so two answers compare as bytes: elapsed times and
/// read counts differ between any two runs. Other bytes are untouched;
/// a document without statistics comes back as it is.
pub fn strip_statistics(document: &[u8]) -> Vec<u8> {
    const KEY: &[u8] = b",\n\n\t\"statistics\":\n\t{\n";
    let Some(start) = find(document, KEY) else {
        return document.to_vec();
    };
    let Some(end) =
        find(&document[start + KEY.len()..], b"\n\t}").map(|e| start + KEY.len() + e + 3)
    else {
        return document.to_vec();
    };
    let mut out = document[..start].to_vec();
    out.extend_from_slice(&document[end..]);
    out
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_are_stripped_and_nothing_else() {
        let doc = b"{\n\t\"rows\": 1,\n\n\t\"statistics\":\n\t{\n\t\t\"elapsed\": 0.001,\n\t\t\"rows_read\": 1,\n\t\t\"bytes_read\": 1\n\t}\n}\n";
        assert_eq!(strip_statistics(doc), b"{\n\t\"rows\": 1\n}\n".to_vec());
        let plain = b"{\"a\":1}\n";
        assert_eq!(strip_statistics(plain), plain.to_vec());
    }
}
