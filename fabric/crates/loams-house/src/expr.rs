//! Catalog expressions (HS1 ruling R11.11): expression text a lake table's
//! properties hold — a column's `DEFAULT`/`MATERIALIZED` expression, the sorting
//! key a `FINAL` merge view groups by — is never spliced into the SQL a worker's
//! control connection runs.
//!
//! The control connection is not read-only (it makes the read views of §49 §7),
//! and another writer of a shared REST catalog can set those properties. So each
//! fragment is parsed with the classifier's parser (sqlparser's
//! `ClickHouseDialect`) **as one expression**, the tree is walked, and the text
//! the view carries is printed again from the tree ([`reprint`]):
//!
//! * a fragment that is not exactly one expression (a `;`, a second statement, a
//!   `)` that closes the House's own parenthesis, anything after the expression)
//!   is refused;
//! * only a pure expression is taken: names of the table's columns, literals,
//!   operators, `CAST`, `CASE`, `INTERVAL`, `[…]`, tuples, `IN` over literals,
//!   and calls of [`CATALOG_FUNCTIONS`] — never a subquery, `EXISTS`, a table
//!   function, a lambda, a qualified name, a wildcard, a parametric or window
//!   call, or anything else;
//! * the printed text is canonical: every name back-quoted, every string
//!   re-escaped, every operator a function call (`plus(a, b)`), so ClickHouse
//!   reads exactly the tree that was checked, whatever the fragment's lexing.
//!
//! The deny list still reads the printed view afterwards ([`crate::read::plan`]);
//! this is the layer that stops text from becoming SQL at all.

use sqlparser::ast::{
    BinaryOperator, CastKind, CeilFloorKind, DateTimeField, Expr, Function, FunctionArg,
    FunctionArgExpr, FunctionArguments, ObjectNamePart, UnaryOperator, Value,
};
use sqlparser::dialect::ClickHouseDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

use crate::types::ChType;

/// The functions a catalog expression may call: scalar, deterministic or
/// generator functions that read nothing but their arguments (arithmetic,
/// comparison, conversion, dates, hashes, strings, arrays and tuples, UUIDs and
/// random numbers, JSON text). Compared without case. Every name is chDB's own
/// (`catalog_functions_are_chdb_functions`) and none is on the deny list or a
/// host function (`catalog_functions_are_not_denied`).
#[rustfmt::skip]
pub const CATALOG_FUNCTIONS: &[&str] = &[
    // Arithmetic and logic (the operators print as these).
    "plus", "minus", "multiply", "divide", "intDiv", "intDivOrZero", "modulo", "moduloOrZero",
    "negate", "abs", "gcd", "lcm", "equals", "notEquals", "less", "greater", "lessOrEquals",
    "greaterOrEquals", "and", "or", "not", "xor", "bitAnd", "bitOr", "bitXor", "bitNot",
    "bitShiftLeft", "bitShiftRight",
    // Conditionals and NULLs.
    "if", "multiIf", "ifNull", "nullIf", "coalesce", "assumeNotNull", "toNullable", "isNull",
    "isNotNull", "isZeroOrNull",
    // Maths.
    "least", "greatest", "round", "roundBankers", "floor", "ceil", "trunc", "sign", "sqrt",
    "cbrt", "exp", "exp2", "exp10", "log", "log2", "log10", "pow", "power",
    // Conversions.
    "CAST", "accurateCast", "accurateCastOrNull", "toInt8", "toInt16", "toInt32", "toInt64",
    "toInt128", "toInt256", "toUInt8", "toUInt16", "toUInt32", "toUInt64", "toUInt128",
    "toUInt256", "toFloat32", "toFloat64", "toDecimal32", "toDecimal64", "toDecimal128",
    "toDecimal256", "toString", "toFixedString", "toBool", "toInt8OrZero", "toInt16OrZero",
    "toInt32OrZero", "toInt64OrZero", "toUInt8OrZero", "toUInt16OrZero", "toUInt32OrZero",
    "toUInt64OrZero", "toFloat32OrZero", "toFloat64OrZero", "toInt8OrNull", "toInt16OrNull",
    "toInt32OrNull", "toInt64OrNull", "toUInt8OrNull", "toUInt16OrNull", "toUInt32OrNull",
    "toUInt64OrNull", "toFloat32OrNull", "toFloat64OrNull", "toDate", "toDate32", "toDateTime",
    "toDateTime64", "toDateOrZero", "toDateOrNull", "toDateTimeOrZero", "toDateTimeOrNull",
    "toUUID", "toUUIDOrZero", "toUUIDOrNull", "toIPv4", "toIPv6", "toIPv4OrNull",
    "toIPv6OrNull", "reinterpretAsString",
    // Dates and times.
    "now", "now64", "today", "yesterday", "toYear", "toQuarter", "toMonth", "toDayOfYear",
    "toDayOfMonth", "toDayOfWeek", "toHour", "toMinute", "toSecond", "toISOWeek", "toWeek",
    "toYYYYMM", "toYYYYMMDD", "toYYYYMMDDhhmmss", "toStartOfYear", "toStartOfQuarter",
    "toStartOfMonth", "toStartOfWeek", "toMonday", "toStartOfDay", "toStartOfHour",
    "toStartOfMinute", "toStartOfSecond", "toStartOfFiveMinutes", "toStartOfTenMinutes",
    "toStartOfFifteenMinutes", "toStartOfInterval", "toLastDayOfMonth", "toUnixTimestamp",
    "toUnixTimestamp64Milli", "toUnixTimestamp64Micro", "toUnixTimestamp64Nano",
    "fromUnixTimestamp", "fromUnixTimestamp64Milli", "fromUnixTimestamp64Micro",
    "fromUnixTimestamp64Nano", "toTimeZone", "toTimezone", "toRelativeYearNum",
    "toRelativeMonthNum", "toRelativeWeekNum", "toRelativeDayNum", "toRelativeHourNum",
    "toRelativeMinuteNum", "toRelativeSecondNum", "addYears", "addQuarters", "addMonths",
    "addWeeks", "addDays", "addHours", "addMinutes", "addSeconds", "subtractYears",
    "subtractQuarters", "subtractMonths", "subtractWeeks", "subtractDays", "subtractHours",
    "subtractMinutes", "subtractSeconds", "dateDiff", "dateTrunc", "formatDateTime",
    "parseDateTimeBestEffort", "parseDateTimeBestEffortOrNull", "parseDateTimeBestEffortOrZero",
    "parseDateTime64BestEffort", "toIntervalSecond", "toIntervalMinute", "toIntervalHour",
    "toIntervalDay", "toIntervalWeek", "toIntervalMonth", "toIntervalQuarter", "toIntervalYear",
    // Hashes.
    "cityHash64", "sipHash64", "sipHash128", "intHash32", "intHash64", "xxHash32", "xxHash64",
    "xxh3", "murmurHash2_32", "murmurHash2_64", "murmurHash3_32", "murmurHash3_64",
    "murmurHash3_128", "farmHash64", "farmFingerprint64", "javaHash", "hiveHash", "halfMD5",
    "MD5", "SHA1", "SHA224", "SHA256", "SHA512", "wyHash64", "metroHash64", "gccMurmurHash",
    // Strings.
    "lower", "upper", "lowerUTF8", "upperUTF8", "length", "lengthUTF8", "empty", "notEmpty",
    "concat", "concatWithSeparator", "substring", "substringUTF8", "left", "right", "leftUTF8",
    "rightUTF8", "trim", "trimLeft", "trimRight", "trimBoth", "leftPad", "rightPad",
    "leftPadUTF8", "rightPadUTF8", "lpad", "rpad", "reverse", "reverseUTF8", "repeat",
    "replaceAll", "replaceOne", "replaceRegexpAll", "replaceRegexpOne", "position",
    "positionCaseInsensitive", "startsWith", "endsWith", "match", "extract", "like", "notLike",
    "ilike", "notILike", "splitByChar", "splitByString", "toValidUTF8", "base64Encode",
    "base64Decode", "tryBase64Decode", "hex", "unhex",
    // Arrays and tuples.
    "tuple", "tupleElement", "array", "arrayElement", "has", "hasAll", "hasAny", "indexOf",
    "arrayConcat", "arraySort", "arrayReverse", "arrayDistinct", "arrayUniq", "arraySlice",
    "arrayStringConcat", "emptyArrayString",
    // Higher-order and more array functions, and maps (HE1 R9.12: Langfuse's
    // `MATERIALIZED` and `DEFAULT` columns). Their lambdas are pure expressions
    // too: [`print`] checks a lambda's body as it checks any other.
    "arraySum", "arrayProduct", "arrayMin", "arrayMax", "arrayAvg", "arrayMap", "arrayFilter",
    "arrayExists", "arrayAll", "arrayCount", "arrayFirst", "arrayLast", "arrayFirstIndex",
    "arraySplit", "arrayReverseSort", "arrayEnumerate", "arrayEnumerateUniq", "arrayZip",
    "arrayFlatten", "arrayCompact", "arrayPushBack", "arrayPushFront", "arrayPopBack",
    "arrayPopFront", "arrayResize", "arrayIntersect", "arrayElementOrNull", "countEqual",
    "emptyArrayUInt8", "emptyArrayUInt64", "emptyArrayInt64", "emptyArrayFloat64", "range",
    "map", "mapKeys", "mapValues", "mapFilter", "mapApply", "mapContains", "mapFromArrays",
    "mapUpdate", "mapConcat", "mapExists", "mapAll", "mapSort",
    "splitByNonAlpha", "splitByWhitespace", "splitByRegexp", "positionUTF8",
    "positionCaseInsensitiveUTF8", "multiSearchAny", "countSubstrings",
    // Generators.
    "generateUUIDv4", "generateUUIDv7", "generateSnowflakeID", "rand", "rand32", "rand64",
    "randCanonical",
    // JSON text.
    "JSONExtractString", "JSONExtractInt", "JSONExtractUInt", "JSONExtractFloat",
    "JSONExtractBool", "JSONExtractRaw", "JSONHas", "JSONLength",
    // Types.
    "defaultValueOfTypeName", "toTypeName", "materialize", "identity",
];

/// Whether `name` is one of [`CATALOG_FUNCTIONS`] (without case).
pub fn is_catalog_function(name: &str) -> bool {
    CATALOG_FUNCTIONS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
}

/// Parses `text` as one ClickHouse expression and prints it again from its
/// tree, or says why it is not a pure expression of [`CATALOG_FUNCTIONS`].
pub fn reprint(text: &str) -> Result<String, String> {
    print(&parse(&tuple_indexes(text))?)
}

/// ClickHouse's tuple index `t.1` (`tupleElement(t, 1)`), which sqlparser 0.63
/// reads as a name followed by the number `.1`: written out as the call before
/// parsing (HE1 R9.12). A name is a word that does not start with a digit, right
/// before a `.` and a number with nothing between them; string literals and
/// quoted names are left alone.
fn tuple_indexes(text: &str) -> String {
    let toks = crate::request::tokens(text.as_bytes());
    let raw = |i: usize| &text[toks[i].start..toks[i].end];
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < toks.len() {
        let is_name = toks[i].word && !raw(i).starts_with(|c: char| c.is_ascii_digit());
        let indexed = |j: usize| {
            j + 2 < toks.len()
                && raw(j + 1) == "."
                && toks[j + 1].start == toks[j].end
                && toks[j + 2].start == toks[j + 1].end
                && toks[j + 2].word
                && raw(j + 2).chars().all(|c| c.is_ascii_digit())
        };
        if is_name && indexed(i) {
            let mut expr = raw(i).to_string();
            let mut j = i;
            while indexed(j) {
                expr = format!("tupleElement({expr}, {})", raw(j + 2));
                j += 2;
            }
            out.push_str(&text[copied..toks[i].start]);
            out.push_str(&expr);
            copied = toks[j].end;
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

/// A sorting key's elements, each printed again from its tree, with the column
/// it is when it is a bare column (`ORDER BY (a, toDate(b))` → `` `a` `` (`a`),
/// `` toDate(`b`) ``); `tuple()` and the empty text have none.
pub fn key_elements(text: &str) -> Result<Vec<(String, Option<String>)>, String> {
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let expr = parse(text)?;
    let elements: Vec<&Expr> = match &expr {
        Expr::Tuple(items) => items.iter().collect(),
        Expr::Nested(inner) => vec![inner.as_ref()],
        Expr::Function(function) if function_name(function)?.eq_ignore_ascii_case("tuple") => {
            plain_arguments(function)?
        }
        other => vec![other],
    };
    elements
        .into_iter()
        .map(|element| {
            let column = match element {
                Expr::Identifier(ident) => Some(ident.value.clone()),
                _ => None,
            };
            Ok((print(element)?, column))
        })
        .collect()
}

/// sqlparser's own limit on nesting is its default (50); deeper text is refused.
fn parse(text: &str) -> Result<Expr, String> {
    let dialect = ClickHouseDialect {};
    let mut parser = Parser::new(&dialect)
        .try_with_sql(text)
        .map_err(|err| format!("cannot read {text:?}: {err}"))?;
    let expr = parser
        .parse_expr()
        .map_err(|err| format!("{text:?} is not an expression: {err}"))?;
    let next = parser.peek_token();
    if next.token != Token::EOF {
        return Err(format!(
            "{text:?} is more than one expression (it goes on at {})",
            next.token
        ));
    }
    Ok(expr)
}

/// A name, back-quoted always (a keyword never reads as one).
fn name(value: &str) -> String {
    format!("`{}`", value.replace('\\', "\\\\").replace('`', "\\`"))
}

/// A string literal.
fn string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn call(function: &str, args: &[String]) -> String {
    format!("{function}({})", args.join(", "))
}

fn refuse(what: &str) -> String {
    format!("{what} is not allowed in a catalog expression")
}

/// A function's single, unqualified name, as a plain word.
fn function_name(function: &Function) -> Result<&str, String> {
    let [ObjectNamePart::Identifier(ident)] = function.name.0.as_slice() else {
        return Err(refuse("a qualified function name"));
    };
    let word = !ident.value.is_empty()
        && ident
            .value
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && ident
            .value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !word {
        return Err(refuse(&format!("the function name {:?}", ident.value)));
    }
    Ok(&ident.value)
}

/// A call's arguments, when it is a plain call: no parameters, `DISTINCT`,
/// clauses, named arguments, wildcards, `FILTER`, `OVER` or `WITHIN GROUP`.
fn plain_arguments(function: &Function) -> Result<Vec<&Expr>, String> {
    if !matches!(function.parameters, FunctionArguments::None) {
        return Err(refuse("a parametric function call"));
    }
    if function.filter.is_some()
        || function.null_treatment.is_some()
        || function.over.is_some()
        || !function.within_group.is_empty()
        || function.uses_odbc_syntax
    {
        return Err(refuse("an aggregate or window clause"));
    }
    let FunctionArguments::List(list) = &function.args else {
        return Err(refuse("a function without an argument list"));
    };
    if list.duplicate_treatment.is_some() || !list.clauses.is_empty() {
        return Err(refuse("an aggregate clause"));
    }
    list.args
        .iter()
        .map(|arg| match arg {
            FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => Ok(expr),
            _ => Err(refuse("a named or wildcard argument")),
        })
        .collect()
}

/// A number as ClickHouse reads it: digits, a fraction, an exponent, or hex.
fn number(text: &str) -> Result<String, String> {
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    let hex = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()));
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => (m, Some(e.strip_prefix(['+', '-']).unwrap_or(e))),
        None => (text, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, "0"));
    let decimal = digits(whole) && digits(fraction) && exponent.is_none_or(digits);
    if hex || decimal {
        Ok(text.to_string())
    } else {
        Err(refuse(&format!("the number {text:?}")))
    }
}

fn literal(value: &Value) -> Result<String, String> {
    match value {
        Value::Number(text, false) => number(&text.to_string()),
        Value::SingleQuotedString(text) => Ok(string(text)),
        Value::Boolean(true) => Ok("true".to_string()),
        Value::Boolean(false) => Ok("false".to_string()),
        Value::Null => Ok("NULL".to_string()),
        other => Err(refuse(&format!("the literal {other}"))),
    }
}

/// Whether an expression is a literal (or a negated number, or a tuple of
/// literals): what an `IN` list may hold, so that its right side never names a
/// table.
fn is_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Value(_) => true,
        Expr::UnaryOp {
            op: UnaryOperator::Minus | UnaryOperator::Plus,
            expr,
        } => matches!(expr.as_ref(), Expr::Value(_)),
        Expr::Nested(inner) => is_literal(inner),
        Expr::Tuple(items) => items.iter().all(is_literal),
        _ => false,
    }
}

fn interval_function(field: &DateTimeField) -> Option<&'static str> {
    Some(match field {
        DateTimeField::Year | DateTimeField::Years => "toIntervalYear",
        DateTimeField::Quarter => "toIntervalQuarter",
        DateTimeField::Month | DateTimeField::Months => "toIntervalMonth",
        DateTimeField::Week(None) | DateTimeField::Weeks => "toIntervalWeek",
        DateTimeField::Day | DateTimeField::Days => "toIntervalDay",
        DateTimeField::Hour | DateTimeField::Hours => "toIntervalHour",
        DateTimeField::Minute | DateTimeField::Minutes => "toIntervalMinute",
        DateTimeField::Second | DateTimeField::Seconds => "toIntervalSecond",
        _ => return None,
    })
}

fn binary_function(op: &BinaryOperator) -> Option<&'static str> {
    Some(match op {
        BinaryOperator::Plus => "plus",
        BinaryOperator::Minus => "minus",
        BinaryOperator::Multiply => "multiply",
        BinaryOperator::Divide => "divide",
        BinaryOperator::Modulo => "modulo",
        BinaryOperator::StringConcat => "concat",
        BinaryOperator::Gt => "greater",
        BinaryOperator::Lt => "less",
        BinaryOperator::GtEq => "greaterOrEquals",
        BinaryOperator::LtEq => "lessOrEquals",
        BinaryOperator::Eq => "equals",
        BinaryOperator::NotEq => "notEquals",
        BinaryOperator::And => "and",
        BinaryOperator::Or => "or",
        BinaryOperator::Xor => "xor",
        _ => return None,
    })
}

/// `floor(x[, scale])` and `ceil(…)`, which sqlparser reads as their own nodes.
fn rounding(function: &str, expr: &Expr, field: &CeilFloorKind) -> Result<String, String> {
    match field {
        CeilFloorKind::DateTimeField(DateTimeField::NoDateTime) => {
            Ok(call(function, &[print(expr)?]))
        }
        CeilFloorKind::Scale(scale) => Ok(call(function, &[print(expr)?, literal(&scale.value)?])),
        CeilFloorKind::DateTimeField(_) => Err(refuse(&format!("{function}(… TO …)"))),
    }
}

/// A lambda's parameter: a plain name, no type.
fn lambda_param(param: &sqlparser::ast::LambdaFunctionParameter) -> Result<String, String> {
    if param.data_type.is_some() {
        return Err(refuse("a typed lambda parameter"));
    }
    Ok(name(&param.name.value))
}

/// `x -> body` or `(x, y) -> body`, its body checked as any expression.
fn print_lambda(lambda: &sqlparser::ast::LambdaFunction) -> Result<String, String> {
    use sqlparser::ast::OneOrManyWithParens;
    let params: Vec<String> = match &lambda.params {
        OneOrManyWithParens::One(p) => vec![lambda_param(p)?],
        OneOrManyWithParens::Many(ps) => ps.iter().map(lambda_param).collect::<Result<_, _>>()?,
    };
    let params = if params.len() == 1 {
        params[0].clone()
    } else {
        format!("({})", params.join(", "))
    };
    Ok(format!("({params} -> {})", print(&lambda.body)?))
}

/// The text ClickHouse runs for a checked tree.
fn print(expr: &Expr) -> Result<String, String> {
    let all = |items: &[Expr]| items.iter().map(print).collect::<Result<Vec<_>, _>>();
    Ok(match expr {
        Expr::Identifier(ident) => name(&ident.value),
        Expr::Value(value) => literal(&value.value)?,
        Expr::Nested(inner) => format!("({})", print(inner)?),
        Expr::BinaryOp { left, op, right } => {
            let function = binary_function(op).ok_or_else(|| refuse(&format!("{op}")))?;
            call(function, &[print(left)?, print(right)?])
        }
        Expr::UnaryOp { op, expr } => match op {
            UnaryOperator::Plus => print(expr)?,
            UnaryOperator::Minus => call("negate", &[print(expr)?]),
            UnaryOperator::Not => call("not", &[print(expr)?]),
            UnaryOperator::BitwiseNot => call("bitNot", &[print(expr)?]),
            other => return Err(refuse(&format!("the operator {other}"))),
        },
        Expr::IsNull(inner) => call("isNull", &[print(inner)?]),
        Expr::IsNotNull(inner) => call("isNotNull", &[print(inner)?]),
        Expr::Between {
            expr,
            negated,
            low,
            high,
        } => {
            // As ClickHouse's parser expands it.
            let value = print(expr)?;
            let (low, high) = (print(low)?, print(high)?);
            if *negated {
                call(
                    "or",
                    &[
                        call("less", &[value.clone(), low]),
                        call("greater", &[value, high]),
                    ],
                )
            } else {
                call(
                    "and",
                    &[
                        call("greaterOrEquals", &[value.clone(), low]),
                        call("lessOrEquals", &[value, high]),
                    ],
                )
            }
        }
        Expr::InList {
            expr,
            list,
            negated,
        } => {
            if list.is_empty() || !list.iter().all(is_literal) {
                return Err(refuse("an IN list of anything but literals"));
            }
            format!(
                "({} {}IN ({}))",
                print(expr)?,
                if *negated { "NOT " } else { "" },
                all(list)?.join(", ")
            )
        }
        Expr::Like {
            negated,
            any: false,
            expr,
            pattern,
            escape_char: None,
        } => call(
            if *negated { "notLike" } else { "like" },
            &[print(expr)?, print(pattern)?],
        ),
        Expr::ILike {
            negated,
            any: false,
            expr,
            pattern,
            escape_char: None,
        } => call(
            if *negated { "notILike" } else { "ilike" },
            &[print(expr)?, print(pattern)?],
        ),
        Expr::Cast {
            kind: CastKind::Cast | CastKind::DoubleColon,
            expr,
            data_type,
            format: None,
        } => {
            let ty = data_type.to_string();
            ChType::parse(&ty).map_err(|_| refuse(&format!("the type {ty:?}")))?;
            call("CAST", &[print(expr)?, string(&ty)])
        }
        Expr::Ceil { expr, field } => rounding("ceil", expr, field)?,
        Expr::Floor { expr, field } => rounding("floor", expr, field)?,
        Expr::Function(function) => {
            let function_name = function_name(function)?;
            if !is_catalog_function(function_name) {
                return Err(refuse(&format!("the function {function_name}")));
            }
            // A lambda is only ever a higher-order function's argument.
            let args = plain_arguments(function)?
                .into_iter()
                .map(|arg| match arg {
                    Expr::Lambda(lambda) => print_lambda(lambda),
                    other => print(other),
                })
                .collect::<Result<Vec<_>, _>>()?;
            call(function_name, &args)
        }
        Expr::Case {
            operand,
            conditions,
            else_result,
            ..
        } => {
            let mut args = Vec::new();
            for when in conditions {
                let condition = print(&when.condition)?;
                args.push(match operand {
                    Some(operand) => call("equals", &[print(operand)?, condition]),
                    None => condition,
                });
                args.push(print(&when.result)?);
            }
            args.push(match else_result {
                Some(other) => print(other)?,
                None => "NULL".to_string(),
            });
            call("multiIf", &args)
        }
        Expr::Tuple(items) => call("tuple", &all(items)?),
        Expr::Array(array) => format!("[{}]", all(&array.elem)?.join(", ")),
        Expr::Interval(interval) => {
            let function = interval
                .leading_field
                .as_ref()
                .filter(|_| {
                    interval.last_field.is_none()
                        && interval.leading_precision.is_none()
                        && interval.fractional_seconds_precision.is_none()
                })
                .and_then(interval_function)
                .ok_or_else(|| refuse("an INTERVAL without one unit"))?;
            call(function, &[print(&interval.value)?])
        }
        Expr::CompoundFieldAccess { root, access_chain } => {
            use sqlparser::ast::{AccessExpr, Subscript};
            let mut out = print(root)?;
            for access in access_chain {
                out = match access {
                    AccessExpr::Subscript(Subscript::Index { index }) => {
                        call("arrayElement", &[out, print(index)?])
                    }
                    _ => return Err(refuse("a slice or a field access")),
                };
            }
            out
        }
        Expr::Subquery(_) | Expr::Exists { .. } | Expr::InSubquery { .. } => {
            return Err(refuse("a subquery"));
        }
        other => return Err(refuse(&format!("{other}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(text: &str) -> String {
        reprint(text).unwrap_or_else(|why| panic!("{text}: {why}"))
    }

    fn refused(text: &str) -> String {
        match reprint(text) {
            Ok(printed) => panic!("{text} was taken as {printed}"),
            Err(why) => why,
        }
    }

    /// HE1 R9.12: Langfuse's column expressions.
    #[test]
    fn lambdas_subscripts_and_tuple_indexes() {
        assert_eq!(
            ok(
                "arraySum(mapValues(mapFilter(x -> positionCaseInsensitive(x.1, 'input') > 0, cost_details)))"
            ),
            "arraySum(mapValues(mapFilter((`x` -> greater(positionCaseInsensitive(tupleElement(`x`, 1), 'input'), 0)), `cost_details`)))"
        );
        assert_eq!(
            ok("cost_details['total']"),
            "arrayElement(`cost_details`, 'total')"
        );
        assert_eq!(
            ok("arrayMap(v -> leftUTF8(v, 200), metadata_values)"),
            "arrayMap((`v` -> leftUTF8(`v`, 200)), `metadata_values`)"
        );
        assert_eq!(
            ok("arrayMap((k, v) -> k, a, b)"),
            "arrayMap(((`k`, `v`) -> `k`), `a`, `b`)"
        );
        assert_eq!(ok("t.1.2"), "tupleElement(tupleElement(`t`, 1), 2)");
        assert_eq!(ok("'x.1'"), "'x.1'");
        assert_eq!(ok("1.5"), "1.5");
        assert_eq!(
            ok("arrayElement(metadata_values, indexOf(metadata_names, 'evaluator_id'))"),
            "arrayElement(`metadata_values`, indexOf(`metadata_names`, 'evaluator_id'))"
        );
        // A lambda's body is held to the same rules.
        refused("arrayMap(x -> file('/etc/passwd'), a)");
        refused("arrayMap(x -> (SELECT 1), a)");
    }

    #[test]
    fn expressions_print_from_their_tree() {
        assert_eq!(ok("plus(1, 1)"), "plus(1, 1)");
        assert_eq!(ok("k + 1"), "plus(`k`, 1)");
        assert_eq!(ok("-k * 2"), "multiply(negate(`k`), 2)");
        assert_eq!(ok("toDate(d)"), "toDate(`d`)");
        assert_eq!(ok("now()"), "now()");
        assert_eq!(ok("'it''s'"), "'it\\'s'");
        assert_eq!(ok("'a\\'b'"), "'a\\'b'");
        assert_eq!(ok("CAST(k AS UInt32)"), "CAST(`k`, 'UInt32')");
        // sqlparser prints `String` as `STRING`, ClickHouse's alias of it.
        assert_eq!(ok("k::Nullable(String)"), "CAST(`k`, 'Nullable(STRING)')");
        assert_eq!(ok("`select`"), "`select`");
        assert_eq!(ok("\"we`ird\""), "`we\\`ird`");
        assert_eq!(
            ok("CASE WHEN k > 1 THEN 'a' ELSE 'b' END"),
            "multiIf(greater(`k`, 1), 'a', 'b')"
        );
        assert_eq!(ok("k IN (1, 2)"), "(`k` IN (1, 2))");
        assert_eq!(ok("[1, 2]"), "[1, 2]");
        assert_eq!(ok("(1, 'a')"), "tuple(1, 'a')");
        assert_eq!(
            ok("now() - INTERVAL 1 DAY"),
            "minus(now(), toIntervalDay(1))"
        );
        assert_eq!(ok("s LIKE 'a%'"), "like(`s`, 'a%')");
        assert_eq!(ok("k IS NULL"), "isNull(`k`)");
        assert_eq!(
            ok("k BETWEEN 1 AND 2"),
            "and(greaterOrEquals(`k`, 1), lessOrEquals(`k`, 2))"
        );
        assert_eq!(
            ok("k NOT BETWEEN 1 AND 2"),
            "or(less(`k`, 1), greater(`k`, 2))"
        );
        assert_eq!(ok("floor(x)"), "floor(`x`)");
        assert_eq!(ok("1 -- a comment"), "1");
        assert_eq!(ok("LOWER(s)"), "LOWER(`s`)");
    }

    #[test]
    fn statements_and_subqueries_are_refused() {
        for text in [
            "1); DROP TABLE x; --",
            "1; DROP TABLE x",
            "1, 2",
            "1) AS n FROM numbers(1) --",
            "(SELECT 1)",
            "(SELECT count() FROM system.one)",
            "EXISTS (SELECT 1)",
            "k IN (SELECT 1)",
            "k IN t",
            "k IN (t)",
            "numbers(1)",
            "file('/etc/hostname')",
            "url('http://127.0.0.1:1/x', 'CSV', 'a String')",
            "hostName()",
            "dictGet('d', 'a', 1)",
            "joinGet('j', 'a', 1)",
            "x -> x + 1",
            "db.t.c",
            "*",
            "quantile(0.5)(k)",
            "count() OVER ()",
            "sum(DISTINCT k)",
            "f(a => 1)",
            "`file`('/etc/passwd')",
            "TRY_CAST(k AS UInt8)",
            "CAST(k AS UInt8) FORMAT JSON",
        ] {
            refused(text);
        }
    }

    #[test]
    fn sorting_keys_split_into_elements() {
        assert_eq!(key_elements("").expect("empty"), Vec::new());
        assert_eq!(key_elements("tuple()").expect("tuple()"), Vec::new());
        assert_eq!(
            key_elements("k").expect("one"),
            vec![("`k`".to_string(), Some("k".to_string()))]
        );
        assert_eq!(
            key_elements("(k, toDate(d))").expect("two"),
            vec![
                ("`k`".to_string(), Some("k".to_string())),
                ("toDate(`d`)".to_string(), None)
            ]
        );
        assert_eq!(
            key_elements("tuple(a, b)").expect("tuple"),
            vec![
                ("`a`".to_string(), Some("a".to_string())),
                ("`b`".to_string(), Some("b".to_string()))
            ]
        );
        for text in [
            "k, (SELECT 1)",
            "(k, (SELECT 1))",
            "`k) FROM numbers(1) --`(k)",
            "k); DROP DATABASE default; --",
        ] {
            assert!(key_elements(text).is_err(), "{text}");
        }
    }

    #[test]
    fn catalog_functions_are_not_denied() {
        for denied in crate::deny::DENIED_FUNCTIONS
            .iter()
            .chain(crate::deny::HOST_FUNCTIONS.iter().map(|h| &h.name))
            .chain(crate::deny::DENIED_TABLE_FUNCTIONS)
            .chain(crate::deny::ALLOWED_TABLE_FUNCTIONS)
        {
            assert!(!is_catalog_function(denied), "{denied}");
        }
    }
}
