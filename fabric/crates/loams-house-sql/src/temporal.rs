//! The date and time syntax of ClickHouse's dialect (plan HE1 Task 30),
//! rewritten into the function calls ClickHouse itself makes of it:
//!
//! * `INTERVAL n UNIT` is `toIntervalUnit(n)`, and `INTERVAL 'n unit'` is
//!   `toIntervalUnit(n)` too;
//! * `EXTRACT(YEAR FROM x)` is `toYear(x)` (and `toQuarter`, `toMonth`,
//!   `toDayOfMonth`, `toHour`, `toMinute`, `toSecond`).
//!
//! The column names were taken from the text before these rewrites
//! ([`crate::names`]), and they are ClickHouse's.
//!
//! Also here: [`fold_constant_args`]. The date and time functions read some
//! arguments as constants when they type their result (a `DateTime64`'s
//! precision, a time zone, `dateDiff`'s unit). A bound parameter arrives as
//! `_CAST(3, 'UInt8')`, which DataFusion does not see as a constant while it
//! plans, so a cast of a plain literal to an unsigned integer or `String`
//! type is folded into the literal in those functions' arguments (R30.4).

use datafusion::sql::sqlparser::ast::{
    DataType, DateTimeField, Expr, Function, FunctionArg, FunctionArgExpr, FunctionArgumentList,
    FunctionArguments, Ident, Interval, ObjectName, Value, ValueWithSpan,
};
use loams_house_types::ChError;

/// The ClickHouse unit name (`Day`) of an interval's field.
fn unit(field: &DateTimeField) -> Option<&'static str> {
    Some(match field {
        DateTimeField::Year | DateTimeField::Years => "Year",
        DateTimeField::Quarter => "Quarter",
        DateTimeField::Month | DateTimeField::Months => "Month",
        DateTimeField::Week(None) | DateTimeField::Weeks => "Week",
        DateTimeField::Day | DateTimeField::Days => "Day",
        DateTimeField::Hour | DateTimeField::Hours => "Hour",
        DateTimeField::Minute | DateTimeField::Minutes => "Minute",
        DateTimeField::Second | DateTimeField::Seconds => "Second",
        DateTimeField::Millisecond | DateTimeField::Milliseconds => "Millisecond",
        DateTimeField::Microsecond | DateTimeField::Microseconds => "Microsecond",
        DateTimeField::Nanosecond | DateTimeField::Nanoseconds => "Nanosecond",
        _ => return None,
    })
}

/// The unit of a written unit name (`day`, `DAYS`), as `INTERVAL '1 day'`
/// spells it.
fn unit_of_word(word: &str) -> Option<&'static str> {
    let w = word.to_ascii_lowercase();
    let w = w.strip_suffix('s').unwrap_or(&w);
    Some(match w {
        "year" => "Year",
        "quarter" => "Quarter",
        "month" => "Month",
        "week" => "Week",
        "day" => "Day",
        "hour" => "Hour",
        "minute" => "Minute",
        "second" => "Second",
        "millisecond" => "Millisecond",
        "microsecond" => "Microsecond",
        "nanosecond" => "Nanosecond",
        _ => return None,
    })
}

/// A call of `name` (quoted, as the guard leaves every function name) on
/// `args`.
fn call(name: &str, args: Vec<Expr>) -> Expr {
    Expr::Function(Function {
        name: ObjectName::from(vec![Ident::with_quote('"', name)]),
        uses_odbc_syntax: false,
        parameters: FunctionArguments::None,
        args: FunctionArguments::List(FunctionArgumentList {
            duplicate_treatment: None,
            args: args
                .into_iter()
                .map(|a| FunctionArg::Unnamed(FunctionArgExpr::Expr(a)))
                .collect(),
            clauses: vec![],
        }),
        filter: None,
        null_treatment: None,
        over: None,
        within_group: vec![],
    })
}

/// The function name and value an `INTERVAL` literal stands for
/// (`INTERVAL 1 DAY`: `toIntervalDay`, `1`).
///
/// # Errors
///
/// `48` for the SQL-standard forms ClickHouse does not have
/// (`INTERVAL '1-2' YEAR TO MONTH`, `SECOND(3)`).
pub fn interval(iv: &Interval) -> Result<(String, Expr), ChError> {
    let Interval {
        value,
        leading_field,
        leading_precision,
        last_field,
        fractional_seconds_precision,
    } = iv;
    if leading_precision.is_some() || last_field.is_some() || fractional_seconds_precision.is_some()
    {
        return Err(ChError::unsupported(format!("The interval literal {iv}")));
    }
    match leading_field {
        Some(f) => {
            let u =
                unit(f).ok_or_else(|| ChError::unsupported(format!("The interval unit {f}")))?;
            Ok((format!("toInterval{u}"), (**value).clone()))
        }
        // `INTERVAL '1 day'`: ClickHouse reads the number and the unit from
        // the string.
        None => {
            if let Expr::Value(ValueWithSpan {
                value: Value::SingleQuotedString(s),
                span,
            }) = value.as_ref()
            {
                let mut words = s.split_whitespace();
                if let (Some(n), Some(u), None) = (words.next(), words.next(), words.next())
                    && let Some(u) = unit_of_word(u)
                    && n.parse::<i64>().is_ok()
                {
                    return Ok((
                        format!("toInterval{u}"),
                        Expr::Value(ValueWithSpan {
                            value: Value::Number(n.to_string(), false),
                            span: *span,
                        }),
                    ));
                }
            }
            Err(ChError::syntax_error(format!(
                "Cannot read the interval literal {iv}"
            )))
        }
    }
}

/// `INTERVAL …` as the call ClickHouse makes of it.
///
/// # Errors
///
/// As [`interval`].
pub fn interval_call(iv: &Interval) -> Result<Expr, ChError> {
    let (name, value) = interval(iv)?;
    Ok(call(&name, vec![value]))
}

/// The function `EXTRACT(field FROM …)` calls.
pub fn extract_function(field: &DateTimeField) -> Option<&'static str> {
    Some(match field {
        DateTimeField::Year | DateTimeField::Years => "toYear",
        DateTimeField::Quarter => "toQuarter",
        DateTimeField::Month | DateTimeField::Months => "toMonth",
        DateTimeField::Day | DateTimeField::Days => "toDayOfMonth",
        DateTimeField::Hour | DateTimeField::Hours => "toHour",
        DateTimeField::Minute | DateTimeField::Minutes => "toMinute",
        DateTimeField::Second | DateTimeField::Seconds => "toSecond",
        _ => return None,
    })
}

/// `EXTRACT(field FROM x)` as the call ClickHouse makes of it.
///
/// # Errors
///
/// `48` for a field ClickHouse's `EXTRACT` does not take.
pub fn extract_call(field: &DateTimeField, x: Expr) -> Result<Expr, ChError> {
    let f = extract_function(field)
        .ok_or_else(|| ChError::unsupported(format!("EXTRACT({field} FROM …)")))?;
    Ok(call(f, vec![x]))
}

/// Whether `canonical` is a date and time function whose arguments may be
/// read as constants while planning (R30.4).
fn reads_constants(canonical: &str) -> bool {
    canonical.starts_with("toInterval")
        || canonical.starts_with("toStartOf")
        || canonical.starts_with("toUnixTimestamp")
        || canonical.starts_with("fromUnixTimestamp")
        || canonical.starts_with("parseDateTime")
        || matches!(
            canonical,
            "toDate"
                | "toDate32"
                | "toDateTime"
                | "toDateTime64"
                | "toTimeZone"
                | "toTimezone"
                | "now"
                | "now64"
                | "dateDiff"
                | "timeDiff"
                | "dateTrunc"
                | "formatDateTime"
                | "toYYYYMM"
                | "toYYYYMMDD"
                | "toYYYYMMDDhhmmss"
                | "toYear"
                | "toQuarter"
                | "toMonth"
                | "toDayOfMonth"
                | "toDayOfWeek"
                | "toDayOfYear"
                | "toHour"
                | "toMinute"
                | "toSecond"
                | "toMonday"
        )
}

/// Whether ClickHouse type text names an unsigned integer type or `String`.
/// A cast to a signed type stays: ClickHouse refuses a signed `DateTime64`
/// scale (`43`), and so does the engine (R30.4).
fn plain_type(t: &str) -> bool {
    matches!(t, "String" | "UInt8" | "UInt16" | "UInt32" | "UInt64")
}

/// Folds `CAST(<literal> AS <unsigned integer or String type>)` into the literal in
/// the arguments of the date and time functions that read constants
/// (R30.4). Runs after the guard has walked the arguments, so the casts are
/// already in their `CAST(x AS T)` form.
pub fn fold_constant_args(canonical: &str, args: &mut FunctionArguments) {
    if !reads_constants(canonical) {
        return;
    }
    let FunctionArguments::List(list) = args else {
        return;
    };
    for a in &mut list.args {
        if let FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) = a
            && let Expr::Cast {
                expr,
                data_type: DataType::Custom(name, type_args),
                ..
            } = e
            && type_args.is_empty()
            && plain_type(&name.to_string())
            && let Expr::Value(ValueWithSpan {
                value: Value::Number(..) | Value::SingleQuotedString(_),
                ..
            }) = expr.as_ref()
        {
            *e = (**expr).clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
    use datafusion::sql::sqlparser::parser::Parser;

    fn expr(sql: &str) -> Expr {
        Parser::new(&ClickHouseDialect {})
            .try_with_sql(sql)
            .unwrap()
            .parse_expr()
            .unwrap()
    }

    #[test]
    fn interval_literals_are_calls() {
        let Expr::Interval(iv) = expr("INTERVAL 1 DAY") else {
            panic!()
        };
        assert_eq!(
            interval_call(&iv).unwrap().to_string(),
            "\"toIntervalDay\"(1)"
        );
        // sqlparser's ClickHouse dialect wants a unit after the value, so
        // `INTERVAL '2 hours'` is a syntax error (62) before it gets here.
        assert!(
            Parser::new(&ClickHouseDialect {})
                .try_with_sql("INTERVAL '2 hours'")
                .unwrap()
                .parse_expr()
                .is_err()
        );
        let Expr::Interval(iv) = expr("INTERVAL x MINUTE") else {
            panic!()
        };
        assert_eq!(
            interval_call(&iv).unwrap().to_string(),
            "\"toIntervalMinute\"(x)"
        );
    }

    #[test]
    fn extract_is_a_call() {
        let Expr::Extract { field, expr: x, .. } = expr("EXTRACT(YEAR FROM d)") else {
            panic!()
        };
        assert_eq!(
            extract_call(&field, *x).unwrap().to_string(),
            "\"toYear\"(d)"
        );
    }
}
