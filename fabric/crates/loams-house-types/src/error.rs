//! ClickHouse's error codes and names, as the Rust engine answers them.
//!
//! The engine never invents codes: each one here is ClickHouse's
//! (`src/Common/ErrorCodes.cpp`), so a client that branches on a code sees
//! the one `clickhouse-server` would send.

use std::fmt;

/// An error with ClickHouse's code, name and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChError {
    /// ClickHouse's numeric code (`48`, `50`, …).
    pub code: i32,
    /// ClickHouse's name for the code (`NOT_IMPLEMENTED`, …).
    pub name: &'static str,
    /// The message, in ClickHouse's words where the engine can match them.
    pub message: String,
}

impl fmt::Display for ChError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Code: {}. DB::Exception: {}. ({})",
            self.code, self.message, self.name
        )
    }
}

impl std::error::Error for ChError {}

macro_rules! codes {
    ($($fn:ident => $code:literal, $name:literal;)*) => {
        impl ChError {
            $(
                #[doc = concat!("`", stringify!($code), " ", $name, "`.")]
                pub fn $fn(message: impl Into<String>) -> Self {
                    Self { code: $code, name: $name, message: message.into() }
                }
            )*
        }
    };
}

codes! {
    logical_error => 1, "LOGICAL_ERROR";
    cannot_parse_text => 6, "CANNOT_PARSE_TEXT";
    unknown_identifier => 47, "UNKNOWN_IDENTIFIER";
    not_implemented => 48, "NOT_IMPLEMENTED";
    unknown_type => 50, "UNKNOWN_TYPE";
    bad_arguments => 36, "BAD_ARGUMENTS";
    illegal_type_of_argument => 43, "ILLEGAL_TYPE_OF_ARGUMENT";
    number_of_arguments_doesnt_match => 42, "NUMBER_OF_ARGUMENTS_DOESNT_MATCH";
    unknown_function => 46, "UNKNOWN_FUNCTION";
    syntax_error => 62, "SYNTAX_ERROR";
    cannot_convert_type => 70, "CANNOT_CONVERT_TYPE";
    argument_out_of_bound => 69, "ARGUMENT_OUT_OF_BOUND";
    unexpected_ast_structure => 223, "UNEXPECTED_AST_STRUCTURE";
    support_is_disabled => 344, "SUPPORT_IS_DISABLED";
    unknown_setting => 115, "UNKNOWN_SETTING";
    unknown_format => 73, "UNKNOWN_FORMAT";
    network_error => 210, "NETWORK_ERROR";
}

impl ChError {
    /// `48`, naming the feature the Rust engine does not have.
    pub fn unsupported(feature: impl fmt::Display) -> Self {
        Self::not_implemented(format!(
            "{feature} is not supported by the Loams House engine"
        ))
    }
}
