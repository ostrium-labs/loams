//! `Pretty` and `PrettyCompact` (and their `NoEscapes`/`MonoBlock` names):
//! a table drawn with box characters, as ClickHouse's
//! `PrettyBlockOutputFormat` and `PrettyCompactBlockOutputFormat` draw one
//! over HTTP (Apache-2.0):
//!
//! * no colour: `output_format_pretty_color = auto` colours only a terminal,
//!   so `NoEscapes` writes the same bytes;
//! * one table for the whole result (`output_format_pretty_squash_consecutive_ms`
//!   joins the engine's blocks, which all arrive at once), up to
//!   `output_format_pretty_max_rows` rows, then `Showed first N.`;
//! * row numbers (`output_format_pretty_row_numbers`, on by default) in a
//!   left margin;
//! * values as `serializeText` writes them (`NULL` is `ᴺᵁᴸᴸ`), numbers
//!   right-aligned, cut at `output_format_pretty_max_value_width` with `⋯`;
//! * a single large number gets a readable tip (` -- 1.23 million`).
//!
//! Widths count characters, with a tab advancing to the next multiple of 8.
//! Not done yet: multi-line values (`output_format_pretty_multiline_fields`),
//! the footer of names for long tables, and East Asian wide characters; the
//! oracle corpus decides when they are needed.

use std::io::Write;

use arrow_array::RecordBatch;
use loams_house_types::text::{Style, write_value};
use loams_house_types::{ChError, ChType};

use crate::Column;
use crate::binary::is_null;
use crate::settings::FormatSettings;
use crate::writer::{FormatWriter, Stats, check_batch, io_error};

/// The `Pretty*` writer. It keeps the rows it shows until
/// [`FormatWriter::finish`], since the widths depend on all of them.
pub(crate) struct PrettyWriter<W: Write> {
    compact: bool,
    columns: Vec<Column>,
    settings: FormatSettings,
    out: W,
    /// The shown rows' cells.
    rows: Vec<Vec<String>>,
    /// Every row's count, shown or not.
    total: u64,
    /// The single cell's value as a number, for the readable tip.
    single_number: Option<f64>,
    totals: Option<Vec<String>>,
    extremes: Option<[Vec<String>; 2]>,
}

impl<W: Write> std::fmt::Debug for PrettyWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrettyWriter")
            .field("compact", &self.compact)
            .field("total", &self.total)
            .finish_non_exhaustive()
    }
}

/// Whether ClickHouse right-aligns a type's values
/// (`shouldAlignRightInPrettyFormats`): numbers and decimals.
fn align_right(t: &ChType) -> bool {
    matches!(
        t.base(),
        ChType::UInt8
            | ChType::UInt16
            | ChType::UInt32
            | ChType::UInt64
            | ChType::UInt128
            | ChType::UInt256
            | ChType::Int8
            | ChType::Int16
            | ChType::Int32
            | ChType::Int64
            | ChType::Int128
            | ChType::Int256
            | ChType::Float32
            | ChType::Float64
            | ChType::BFloat16
            | ChType::Decimal { .. }
    )
}

/// A cell's display width.
fn width(s: &str) -> usize {
    let mut w = 0;
    for c in s.chars() {
        if c == '\t' {
            w += 8 - w % 8;
        } else if !c.is_control() {
            w += 1;
        }
    }
    w
}

/// `formatReadableQuantity(value, 2)`.
fn readable_quantity(value: f64) -> String {
    const UNITS: [&str; 6] = [
        "",
        " thousand",
        " million",
        " billion",
        " trillion",
        " quadrillion",
    ];
    let mut size = value;
    let mut i = 0;
    while i + 1 < UNITS.len() && size.abs() >= 1000.0 {
        size /= 1000.0;
        i += 1;
    }
    format!("{size:.2}{}", UNITS[i])
}

impl<W: Write> PrettyWriter<W> {
    pub(crate) fn new(
        compact: bool,
        columns: Vec<Column>,
        settings: FormatSettings,
        out: W,
    ) -> Self {
        Self {
            compact,
            columns,
            settings,
            out,
            rows: Vec::new(),
            total: 0,
            single_number: None,
            totals: None,
            extremes: None,
        }
    }

    fn cells(&self, batch: &RecordBatch, row: usize) -> Result<Vec<String>, ChError> {
        let max = self.settings.pretty_max_value_width;
        self.columns
            .iter()
            .enumerate()
            .map(|(n, c)| {
                let mut bytes = Vec::new();
                write_value(
                    &c.ty,
                    batch.column(n).as_ref(),
                    row,
                    Style::Text,
                    &self.settings.text,
                    &mut bytes,
                )?;
                let text = String::from_utf8_lossy(&bytes).into_owned();
                Ok(if text.chars().count() > max {
                    let mut cut: String = text.chars().take(max).collect();
                    cut.push('⋯');
                    cut
                } else {
                    text
                })
            })
            .collect()
    }

    /// One table of `rows`, with the row-number margin when `numbered`.
    fn table(&self, rows: &[Vec<String>], first_number: u64, numbered: bool, out: &mut String) {
        let n = self.columns.len();
        let pad_cap = self.settings.pretty_max_column_pad_width;
        let names: Vec<usize> = self.columns.iter().map(|c| width(&c.name)).collect();
        let mut widths = names.clone();
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(width(cell).min(pad_cap));
            }
        }
        let margin = if numbered {
            (first_number + rows.len() as u64).to_string().len() + 2
        } else {
            0
        };
        let blank = " ".repeat(margin);
        let line = |l: &str, fill: &str, mid: &str, r: &str, out: &mut String| {
            out.push_str(&blank);
            out.push_str(l);
            for (i, w) in widths.iter().enumerate() {
                if i > 0 {
                    out.push_str(mid);
                }
                out.push_str(&fill.repeat(w + 2));
            }
            out.push_str(r);
            out.push('\n');
        };
        let cell = |text: &str, i: usize, out: &mut String| {
            let pad = " ".repeat(widths[i].saturating_sub(width(text)));
            out.push(' ');
            if align_right(&self.columns[i].ty) {
                out.push_str(&pad);
                out.push_str(text);
            } else {
                out.push_str(text);
                out.push_str(&pad);
            }
            out.push(' ');
        };
        // Header.
        if self.compact {
            out.push_str(&blank);
            out.push_str("┌─");
            for (i, c) in self.columns.iter().enumerate() {
                if i > 0 {
                    out.push_str("─┬─");
                }
                let dashes = "─".repeat(widths[i].saturating_sub(names[i]));
                if align_right(&c.ty) {
                    out.push_str(&dashes);
                    out.push_str(&c.name);
                } else {
                    out.push_str(&c.name);
                    out.push_str(&dashes);
                }
            }
            out.push_str("─┐\n");
        } else {
            line("┏", "━", "┳", "┓", out);
            out.push_str(&blank);
            out.push('┃');
            for (i, c) in self.columns.iter().enumerate() {
                if i > 0 {
                    out.push('┃');
                }
                cell(&c.name, i, out);
            }
            out.push_str("┃\n");
            line("┡", "━", "╇", "┩", out);
        }
        // Rows.
        for (r, row) in rows.iter().enumerate() {
            if r > 0 && !self.compact {
                line("├", "─", "┼", "┤", out);
            }
            if numbered {
                let number = format!("{}. ", first_number + r as u64 + 1);
                out.push_str(&" ".repeat(margin.saturating_sub(number.len())));
                out.push_str(&number);
            }
            out.push('│');
            for (i, text) in row.iter().enumerate().take(n) {
                if i > 0 {
                    out.push('│');
                }
                cell(text, i, out);
            }
            out.push('│');
            if let Some(v) = self.single_number.filter(|_| rows.len() == 1 && n == 1) {
                let threshold = self.settings.pretty_single_large_number_tip_threshold;
                if threshold > 0 && v.is_finite() && v.abs() > threshold as f64 {
                    out.push_str(" -- ");
                    out.push_str(&readable_quantity(v));
                }
            }
            out.push('\n');
        }
        line("└", "─", "┴", "┘", out);
    }
}

impl<W: Write + Send> FormatWriter for PrettyWriter<W> {
    fn write_batch(&mut self, batch: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, batch)?;
        for row in 0..batch.num_rows() {
            if self.total < self.settings.pretty_max_rows {
                let cells = self.cells(batch, row)?;
                self.rows.push(cells);
                if self.total == 0 && self.columns.len() == 1 && align_right(&self.columns[0].ty) {
                    let array = batch.column(0);
                    self.single_number = (!is_null(array.as_ref(), row))
                        .then(|| self.rows[0][0].parse::<f64>().ok())
                        .flatten();
                }
            }
            self.total += 1;
        }
        Ok(())
    }

    fn write_totals(&mut self, totals: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, totals)?;
        if totals.num_rows() > 0 {
            self.totals = Some(self.cells(totals, 0)?);
        }
        Ok(())
    }

    fn write_extremes(&mut self, extremes: &RecordBatch) -> Result<(), ChError> {
        check_batch(&self.columns, extremes)?;
        if extremes.num_rows() >= 2 {
            self.extremes = Some([self.cells(extremes, 0)?, self.cells(extremes, 1)?]);
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>, _stats: &Stats) -> Result<(), ChError> {
        let mut text = String::new();
        let numbered = self.settings.pretty_row_numbers;
        if !self.rows.is_empty() {
            self.table(&self.rows, 0, numbered, &mut text);
        }
        if self.total > self.rows.len() as u64 {
            text.push_str(&format!("  Showed first {}.\n", self.rows.len()));
        }
        // The tip is for a lone result row only.
        self.single_number = None;
        if let Some(totals) = &self.totals {
            text.push_str("\nTotals:\n");
            self.table(std::slice::from_ref(totals), 0, false, &mut text);
        }
        if let Some(extremes) = &self.extremes {
            text.push_str("\nExtremes:\n");
            self.table(extremes, 0, false, &mut text);
        }
        self.out.write_all(text.as_bytes()).map_err(io_error)?;
        self.out.flush().map_err(io_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_quantities_match_clickhouse() {
        assert_eq!(readable_quantity(1_234_567.0), "1.23 million");
        assert_eq!(readable_quantity(12_345_678_901.0), "12.35 billion");
        assert_eq!(readable_quantity(999.0), "999.00");
    }

    #[test]
    fn tabs_advance_to_the_next_stop() {
        assert_eq!(width("a\tb"), 9);
        assert_eq!(width("ᴺᵁᴸᴸ"), 4);
    }
}
