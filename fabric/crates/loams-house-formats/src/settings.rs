//! [`FormatSettings`]: the settings that change how a result is written, read
//! from a request's settings (HTTP parameters, `SETTINGS` clause, native
//! `Query` settings).
//!
//! The writers own these settings, so [`FormatSettings::split`] takes them out
//! of a request's list and leaves the rest for the engine (Task 37, D1805):
//! a setting is either honoured here or refused there, never ignored.

use loams_house_types::ChError;
use loams_house_types::text::{DateTimeOutputFormat, TextSettings};

/// How a result is written.
#[derive(Debug, Clone)]
pub struct FormatSettings {
    /// Value text (`loams_house_types::text`): time zone, JSON quoting,
    /// decimal zeros, `date_time_output_format`.
    pub text: TextSettings,
    /// `output_format_native_write_json_as_string` (default off): `JSON`
    /// columns in `Native` as strings; off, a `JSON` column is `48`.
    pub native_write_json_as_string: bool,
    /// `output_format_binary_write_json_as_string` (default off): the same
    /// for `RowBinary`.
    pub binary_write_json_as_string: bool,
    /// The native protocol revision of the client reading `Native` blocks
    /// (R9.41): before 54405 a `LowCardinality(T)` column is sent as `T`.
    /// `None` for the `Native` format over HTTP, which has no revision.
    pub client_revision: Option<u64>,
    /// `output_format_write_statistics` (default on): `JSON*`'s `statistics`.
    pub write_statistics: bool,
    /// `output_format_pretty_max_rows` (default 10000).
    pub pretty_max_rows: u64,
    /// `output_format_pretty_max_column_pad_width` (default 250).
    pub pretty_max_column_pad_width: usize,
    /// `output_format_pretty_max_value_width` (default 10000).
    pub pretty_max_value_width: usize,
    /// `output_format_pretty_row_numbers` (default on since 24.x).
    pub pretty_row_numbers: bool,
    /// `output_format_pretty_single_large_number_tip_threshold` (default
    /// 1000000; 0 turns the tip off).
    pub pretty_single_large_number_tip_threshold: u64,
    /// `format_csv_delimiter` (default `,`).
    pub csv_delimiter: u8,
    /// `output_format_csv_crlf_end_of_line` (default off).
    pub csv_crlf_end_of_line: bool,
    /// `output_format_tsv_crlf_end_of_line` (default off).
    pub tsv_crlf_end_of_line: bool,
}

impl Default for FormatSettings {
    fn default() -> Self {
        Self {
            text: TextSettings::default(),
            native_write_json_as_string: false,
            binary_write_json_as_string: false,
            client_revision: None,
            write_statistics: true,
            pretty_max_rows: 10_000,
            pretty_max_column_pad_width: 250,
            pretty_max_value_width: 10_000,
            pretty_row_numbers: true,
            pretty_single_large_number_tip_threshold: 1_000_000,
            csv_delimiter: b',',
            csv_crlf_end_of_line: false,
            tsv_crlf_end_of_line: false,
        }
    }
}

/// A boolean setting's value as ClickHouse reads one.
fn boolean(name: &str, value: &str) -> Result<bool, ChError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        other => Err(ChError::cannot_parse_text(format!(
            "Cannot parse boolean value '{other}' of setting {name}"
        ))),
    }
}

fn unsigned(name: &str, value: &str) -> Result<u64, ChError> {
    value.trim().parse().map_err(|_| {
        ChError::cannot_parse_text(format!(
            "Cannot parse '{value}' as an unsigned integer for setting {name}"
        ))
    })
}

impl FormatSettings {
    /// Applies one setting. `Ok(false)`: not a format setting, so the engine
    /// decides.
    ///
    /// # Errors
    ///
    /// A value the setting does not take, with ClickHouse's code.
    pub fn apply(&mut self, name: &str, value: &str) -> Result<bool, ChError> {
        match name {
            "output_format_json_quote_64bit_integers" => {
                self.text.json_quote_64bit_integers = boolean(name, value)?;
            }
            "output_format_json_quote_denormals" => {
                self.text.json_quote_denormals = boolean(name, value)?;
            }
            "output_format_json_escape_forward_slashes" => {
                self.text.json_escape_forward_slashes = boolean(name, value)?;
            }
            "output_format_json_named_tuples_as_objects" => {
                self.text.json_named_tuples_as_objects = boolean(name, value)?;
            }
            "output_format_decimal_trailing_zeros" => {
                self.text.decimal_trailing_zeros = boolean(name, value)?;
            }
            "date_time_output_format" => {
                self.text.date_time_output_format = DateTimeOutputFormat::parse(value)?;
            }
            "output_format_native_write_json_as_string" => {
                self.native_write_json_as_string = boolean(name, value)?;
            }
            "output_format_binary_write_json_as_string" => {
                self.binary_write_json_as_string = boolean(name, value)?;
            }
            "output_format_write_statistics" => self.write_statistics = boolean(name, value)?,
            "output_format_pretty_max_rows" => self.pretty_max_rows = unsigned(name, value)?,
            "output_format_pretty_max_column_pad_width" => {
                self.pretty_max_column_pad_width = unsigned(name, value)? as usize;
            }
            "output_format_pretty_max_value_width" => {
                self.pretty_max_value_width = unsigned(name, value)? as usize;
            }
            "output_format_pretty_row_numbers" => {
                self.pretty_row_numbers = boolean(name, value)?;
            }
            "output_format_pretty_single_large_number_tip_threshold" => {
                self.pretty_single_large_number_tip_threshold = unsigned(name, value)?;
            }
            "format_csv_delimiter" => {
                let bytes = value.as_bytes();
                if bytes.len() != 1 {
                    return Err(ChError::bad_arguments(format!(
                        "A setting's value string has to be an exactly one character long \
                         (format_csv_delimiter = '{value}')"
                    )));
                }
                self.csv_delimiter = bytes[0];
            }
            "output_format_csv_crlf_end_of_line" => {
                self.csv_crlf_end_of_line = boolean(name, value)?;
            }
            "output_format_tsv_crlf_end_of_line" => {
                self.tsv_crlf_end_of_line = boolean(name, value)?;
            }
            "session_timezone" if !value.is_empty() => {
                // The zone of `DateTime` columns without one; the engine reads
                // it too, so it is applied here and kept in the list.
                self.text.timezone = value.to_string();
                return Ok(false);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The format settings among `settings`, applied in order (a later value
    /// wins), and the settings left for the engine.
    ///
    /// # Errors
    ///
    /// As [`FormatSettings::apply`].
    pub fn split(
        settings: &[(String, String)],
    ) -> Result<(FormatSettings, Vec<(String, String)>), ChError> {
        let mut format = FormatSettings::default();
        let mut rest = Vec::new();
        for (name, value) in settings {
            if !format.apply(name, value)? {
                rest.push((name.clone(), value.clone()));
            }
        }
        Ok((format, rest))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn format_settings_are_taken_and_the_rest_left() {
        let settings = vec![
            (
                "output_format_json_quote_64bit_integers".to_string(),
                "1".to_string(),
            ),
            ("max_threads".to_string(), "4".to_string()),
            ("date_time_output_format".to_string(), "iso".to_string()),
            ("session_timezone".to_string(), "Asia/Kolkata".to_string()),
        ];
        let (format, rest) = FormatSettings::split(&settings).unwrap();
        assert!(format.text.json_quote_64bit_integers);
        assert_eq!(
            format.text.date_time_output_format,
            DateTimeOutputFormat::Iso
        );
        assert_eq!(format.text.timezone, "Asia/Kolkata");
        let names: Vec<&str> = rest.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["max_threads", "session_timezone"]);
    }

    #[test]
    fn bad_values_are_refused_with_clickhouse_codes() {
        let mut format = FormatSettings::default();
        assert_eq!(
            format
                .apply("output_format_json_quote_denormals", "maybe")
                .unwrap_err()
                .code,
            6
        );
        assert_eq!(
            format
                .apply("date_time_output_format", "rfc")
                .unwrap_err()
                .code,
            36
        );
        assert_eq!(
            format.apply("format_csv_delimiter", ";;").unwrap_err().code,
            36
        );
        assert!(format.apply("format_csv_delimiter", ";").unwrap());
        assert_eq!(format.csv_delimiter, b';');
    }
}
