//! The row-per-line text formats: `TabSeparated[Raw][WithNames[AndTypes]]`,
//! `CSV[WithNames[AndTypes]]`, and `Values`.
//!
//! As ClickHouse's `TabSeparatedRowOutputFormat`, `CSVRowOutputFormat` and
//! `ValuesRowOutputFormat` write them (Apache-2.0): fields split by a tab or
//! the CSV delimiter, a line feed after each row (`\r\n` with the
//! `*_crlf_end_of_line` settings), header lines of names and types escaped as
//! the format's strings are; `WITH TOTALS`' row and `extremes` after a blank
//! line; `Values` as `(…),(…)` with no line feed at all.

use std::io::Write;

use arrow_array::RecordBatch;
use loams_house_types::ChError;
use loams_house_types::text::{Style, csv_string, escape, write_value};

use crate::Column;
use crate::settings::FormatSettings;
use crate::writer::{Format, FormatWriter, Stats, check_batch, io_error};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Tsv { raw: bool },
    Csv,
    Values,
}

/// A writer of the TSV, CSV and `Values` formats.
pub(crate) struct TextWriter<W: Write> {
    kind: Kind,
    columns: Vec<Column>,
    settings: FormatSettings,
    out: W,
    rows: u64,
}

impl<W: Write> std::fmt::Debug for TextWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextWriter")
            .field("kind", &self.kind)
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl<W: Write> TextWriter<W> {
    pub(crate) fn new(
        format: Format,
        columns: Vec<Column>,
        settings: FormatSettings,
        out: W,
    ) -> Result<Self, ChError> {
        let (kind, names, types) = match format {
            Format::TabSeparated { raw, names, types } => (Kind::Tsv { raw }, names, types),
            Format::Csv { names, types } => (Kind::Csv, names, types),
            Format::Values => (Kind::Values, false, false),
            other => {
                return Err(ChError::logical_error(format!(
                    "the text writer asked for {other:?}"
                )));
            }
        };
        let mut w = Self {
            kind,
            columns,
            settings,
            out,
            rows: 0,
        };
        let mut head = Vec::new();
        if names {
            let names: Vec<String> = w.columns.iter().map(|c| c.name.clone()).collect();
            w.header_line(&names, &mut head);
        }
        if types {
            let types: Vec<String> = w.columns.iter().map(|c| c.ty.to_compact_string()).collect();
            w.header_line(&types, &mut head);
        }
        if !head.is_empty() {
            w.out.write_all(&head).map_err(io_error)?;
        }
        Ok(w)
    }

    fn delimiter(&self) -> u8 {
        match self.kind {
            Kind::Tsv { .. } => b'\t',
            Kind::Csv => self.settings.csv_delimiter,
            Kind::Values => b',',
        }
    }

    fn end_of_line(&self, out: &mut Vec<u8>) {
        let crlf = match self.kind {
            Kind::Tsv { .. } => self.settings.tsv_crlf_end_of_line,
            Kind::Csv => self.settings.csv_crlf_end_of_line,
            Kind::Values => false,
        };
        if crlf {
            out.push(b'\r');
        }
        out.push(b'\n');
    }

    fn header_line(&self, items: &[String], out: &mut Vec<u8>) {
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push(self.delimiter());
            }
            match self.kind {
                Kind::Tsv { raw: false } => escape(item.as_bytes(), out),
                Kind::Tsv { raw: true } => out.extend_from_slice(item.as_bytes()),
                Kind::Csv => csv_string(item.as_bytes(), out),
                Kind::Values => {}
            }
        }
        self.end_of_line(out);
    }

    fn style(&self) -> Style {
        match self.kind {
            Kind::Tsv { raw: false } => Style::Tsv,
            Kind::Tsv { raw: true } => Style::Raw,
            Kind::Csv => Style::Csv,
            Kind::Values => Style::Quoted,
        }
    }

    fn row(&self, batch: &RecordBatch, row: usize, out: &mut Vec<u8>) -> Result<(), ChError> {
        let style = self.style();
        if self.kind == Kind::Values {
            out.push(b'(');
        }
        for (n, c) in self.columns.iter().enumerate() {
            if n > 0 {
                out.push(self.delimiter());
            }
            write_value(
                &c.ty,
                batch.column(n).as_ref(),
                row,
                style,
                &self.settings.text,
                out,
            )?;
        }
        if self.kind == Kind::Values {
            out.push(b')');
        } else {
            self.end_of_line(out);
        }
        Ok(())
    }
}

impl<W: Write + Send> FormatWriter for TextWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, batch)?;
        let mut out = Vec::new();
        for row in 0..batch.num_rows() {
            if self.kind == Kind::Values && self.rows > 0 {
                out.push(b',');
            }
            self.row(batch, row, &mut out)?;
            self.rows += 1;
        }
        self.out.write_all(&out).map_err(io_error)
    }

    fn write_totals(&mut self, totals: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, totals)?;
        if self.kind == Kind::Values || totals.num_rows() == 0 {
            return Ok(());
        }
        let mut out = Vec::new();
        self.end_of_line(&mut out);
        self.row(totals, 0, &mut out)?;
        self.out.write_all(&out).map_err(io_error)
    }

    fn write_extremes(&mut self, extremes: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, extremes)?;
        if self.kind == Kind::Values || extremes.num_rows() < 2 {
            return Ok(());
        }
        let mut out = Vec::new();
        self.end_of_line(&mut out);
        self.row(extremes, 0, &mut out)?;
        self.row(extremes, 1, &mut out)?;
        self.out.write_all(&out).map_err(io_error)
    }

    fn finish(mut self: Box<Self>, _stats: &Stats) -> Result<(), ChError> {
        self.out.flush().map_err(io_error)
    }
}
