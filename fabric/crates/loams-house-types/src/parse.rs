//! Parsing ClickHouse type text: every spelling the 26.9.2.8 oracle accepts
//! for the types the engine knows, and the aliases of
//! `system.data_type_families` (captured from the oracle into
//! `data_type_families.tsv`, with which family names are case-insensitive).

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::error::ChError;
use crate::ty::{AggregateSpec, ChType, JsonParams};

/// `system.data_type_families` of `clickhouse-server` 26.9.2.8: `name`,
/// `case_insensitive`, `alias_to`.
const FAMILIES: &str = include_str!("data_type_families.tsv");

struct Families {
    /// Exact names to their family (after aliases).
    exact: HashMap<&'static str, &'static str>,
    /// Lower-cased case-insensitive names to their family.
    folded: HashMap<String, &'static str>,
    /// The most words a case-insensitive name has (`NATIONAL CHARACTER LARGE OBJECT`).
    max_words: usize,
}

fn families() -> &'static Families {
    static FAMILIES_TABLE: OnceLock<Families> = OnceLock::new();
    FAMILIES_TABLE.get_or_init(|| {
        let mut rows = Vec::new();
        for line in FAMILIES.lines() {
            let mut cols = line.split('\t');
            let (Some(name), Some(ci), alias) = (cols.next(), cols.next(), cols.next()) else {
                continue;
            };
            rows.push((name, ci == "1", alias.unwrap_or("")));
        }
        let target: HashMap<&str, &str> = rows
            .iter()
            .map(|(n, _, a)| (*n, if a.is_empty() { *n } else { *a }))
            .collect();
        let resolve = |mut name: &'static str| {
            for _ in 0..8 {
                match target.get(name) {
                    Some(next) if *next != name => name = next,
                    _ => break,
                }
            }
            name
        };
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        let mut max_words = 1;
        for (name, ci, _) in &rows {
            let family = resolve(name);
            exact.insert(*name, family);
            if *ci {
                folded.insert(name.to_ascii_lowercase(), family);
                max_words = max_words.max(name.split(' ').count());
            }
        }
        Families {
            exact,
            folded,
            max_words,
        }
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    /// A backquoted or double-quoted identifier.
    Ident(String),
    Str(String),
    Num(String),
    Sym(char),
}

fn lex(text: &str) -> Result<Vec<Tok>, ChError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '.')
            {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect()));
        } else if c.is_ascii_digit()
            || ((c == '-' || c == '+') && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric()
                    || chars[i] == '.'
                    || ((chars[i] == '-' || chars[i] == '+') && matches!(chars[i - 1], 'e' | 'E')))
            {
                i += 1;
            }
            out.push(Tok::Num(chars[start..i].iter().collect()));
        } else if c == '\'' || c == '`' || c == '"' {
            let quote = c;
            let mut value = String::new();
            i += 1;
            loop {
                let Some(&ch) = chars.get(i) else {
                    return Err(syntax(text, "an unterminated quote"));
                };
                i += 1;
                if ch == '\\' {
                    let Some(&next) = chars.get(i) else {
                        return Err(syntax(text, "a backslash at the end"));
                    };
                    i += 1;
                    value.push(match next {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        '0' => '\0',
                        'b' => '\x08',
                        'f' => '\x0c',
                        other => other,
                    });
                } else if ch == quote {
                    if chars.get(i) == Some(&quote) {
                        value.push(quote);
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    value.push(ch);
                }
            }
            out.push(if quote == '\'' {
                Tok::Str(value)
            } else {
                Tok::Ident(value)
            });
        } else if "(),=".contains(c) {
            out.push(Tok::Sym(c));
            i += 1;
        } else {
            return Err(syntax(text, &format!("unexpected character {c:?}")));
        }
    }
    Ok(out)
}

fn syntax(text: &str, why: &str) -> ChError {
    ChError::syntax_error(format!("Cannot parse data type {text:?}: {why}"))
}

/// Parses ClickHouse type text into a [`ChType`].
pub fn parse(text: &str) -> Result<ChType, ChError> {
    let toks = lex(text)?;
    let mut p = Parser { text, toks, pos: 0 };
    let t = p.ty()?;
    if p.pos != p.toks.len() {
        return Err(syntax(text, "text after the type"));
    }
    Ok(t)
}

struct Parser<'a> {
    text: &'a str,
    toks: Vec<Tok>,
    pos: usize,
}

/// One argument inside a type's parentheses, before the family reads it.
#[derive(Debug, Clone)]
enum Arg {
    Type(ChType),
    Named(String, ChType),
    Str(String),
    Num(String),
    /// `'name' = value` (enums) or `name=value` (JSON settings).
    Assign(String, String),
    /// `name` alone where a family takes names (an enum's implicit value).
    SkipPath(String),
    SkipRegexp(String),
    /// A function with parameters, inside `AggregateFunction` (`quantiles(0.5)`).
    Function(String, Vec<String>),
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, n: usize) -> Option<&Tok> {
        self.toks.get(self.pos + n)
    }

    fn err(&self, why: &str) -> ChError {
        syntax(self.text, why)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Sym(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: char) -> Result<(), ChError> {
        if self.eat(c) {
            Ok(())
        } else {
            Err(self.err(&format!("expected {c:?}")))
        }
    }

    /// The family name at the cursor: the longest run of words that names a
    /// family, exactly or case-insensitively.
    fn family(&mut self) -> Result<(String, &'static str), ChError> {
        let fam = families();
        let mut words = Vec::new();
        for n in 0..fam.max_words {
            match self.peek_at(n) {
                Some(Tok::Word(w)) => words.push(w.clone()),
                _ => break,
            }
        }
        if words.is_empty() {
            return Err(self.err("expected a type name"));
        }
        for n in (1..=words.len()).rev() {
            let name = words[..n].join(" ");
            let found = if n == 1 {
                fam.exact.get(name.as_str()).copied()
            } else {
                None
            }
            .or_else(|| fam.folded.get(&name.to_ascii_lowercase()).copied());
            if let Some(family) = found {
                self.pos += n;
                return Ok((name, family));
            }
        }
        Err(ChError::unknown_type(format!(
            "Unknown data type family: {}",
            words[0]
        )))
    }

    fn ty(&mut self) -> Result<ChType, ChError> {
        let (written, family) = self.family()?;
        let args = if self.eat('(') {
            self.args(family)?
        } else {
            Vec::new()
        };
        build(self.text, &written, family, args)
    }

    fn args(&mut self, family: &str) -> Result<Vec<Arg>, ChError> {
        let mut args = Vec::new();
        if self.eat(')') {
            return Ok(args);
        }
        loop {
            args.push(self.arg(family, args.len())?);
            if self.eat(')') {
                return Ok(args);
            }
            self.expect(',')?;
        }
    }

    fn arg(&mut self, family: &str, index: usize) -> Result<Arg, ChError> {
        match self.peek().cloned() {
            Some(Tok::Str(s)) => {
                self.pos += 1;
                if self.eat('=') {
                    let v = self.number()?;
                    Ok(Arg::Assign(s, v))
                } else {
                    Ok(Arg::Str(s))
                }
            }
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(Arg::Num(n))
            }
            Some(Tok::Ident(name)) => {
                self.pos += 1;
                Ok(Arg::Named(name, self.ty()?))
            }
            Some(Tok::Word(w)) => {
                let aggregate = matches!(family, "AggregateFunction" | "SimpleAggregateFunction");
                if aggregate && (index == 0 || (index == 1 && self.first_was_version())) {
                    self.pos += 1;
                    let mut params = Vec::new();
                    if self.eat('(') && !self.eat(')') {
                        loop {
                            params.push(self.param()?);
                            if self.eat(')') {
                                break;
                            }
                            self.expect(',')?;
                        }
                    }
                    return Ok(Arg::Function(w, params));
                }
                // `name=value` (`JSON(max_dynamic_paths=0)`, `Dynamic(max_types=3)`).
                if self.peek_at(1) == Some(&Tok::Sym('=')) {
                    self.pos += 2;
                    let v = self.number()?;
                    return Ok(Arg::Assign(w, v));
                }
                if family == "JSON" {
                    if w.eq_ignore_ascii_case("SKIP") {
                        self.pos += 1;
                        if let Some(Tok::Word(r)) = self.peek()
                            && r.eq_ignore_ascii_case("REGEXP")
                        {
                            self.pos += 1;
                            return match self.peek().cloned() {
                                Some(Tok::Str(s)) => {
                                    self.pos += 1;
                                    Ok(Arg::SkipRegexp(s))
                                }
                                _ => {
                                    Err(self.err("expected a regular expression after SKIP REGEXP"))
                                }
                            };
                        }
                        return match self.peek().cloned() {
                            Some(Tok::Word(p) | Tok::Ident(p)) => {
                                self.pos += 1;
                                Ok(Arg::SkipPath(p))
                            }
                            _ => Err(self.err("expected a path after SKIP")),
                        };
                    }
                    if self.peek_at(1) == Some(&Tok::Sym('=')) {
                        self.pos += 2;
                        let v = self.number()?;
                        return Ok(Arg::Assign(w, v));
                    }
                    self.pos += 1;
                    return Ok(Arg::Named(w, self.ty()?));
                }
                // `name Type` (a named tuple or nested element), or a type.
                let named = matches!(family, "Tuple" | "Nested")
                    && matches!(self.peek_at(1), Some(Tok::Word(_) | Tok::Ident(_)))
                    && !self.words_form_family(2);
                if named {
                    self.pos += 1;
                    return Ok(Arg::Named(w, self.ty()?));
                }
                Ok(Arg::Type(self.ty()?))
            }
            _ => Err(self.err("expected a type argument")),
        }
    }

    /// Whether the first argument just read was an `AggregateFunction` version.
    fn first_was_version(&self) -> bool {
        // `(` `1` `,` <here>
        self.pos >= 2 && matches!(self.toks.get(self.pos - 2), Some(Tok::Num(_)))
    }

    /// Whether the next `n` or more words read as one multi-word family name
    /// (`DOUBLE PRECISION`), so the first is not an element name.
    fn words_form_family(&self, n: usize) -> bool {
        let fam = families();
        let mut words = Vec::new();
        for k in 0..fam.max_words {
            match self.peek_at(k) {
                Some(Tok::Word(w)) => words.push(w.as_str()),
                _ => break,
            }
        }
        (n..=words.len()).any(|k| {
            fam.folded
                .contains_key(&words[..k].join(" ").to_ascii_lowercase())
        })
    }

    fn number(&mut self) -> Result<String, ChError> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(n)
            }
            _ => Err(self.err("expected a number")),
        }
    }

    /// An aggregate function's parameter, as ClickHouse prints it.
    fn param(&mut self) -> Result<String, ChError> {
        match self.peek().cloned() {
            Some(Tok::Num(n)) => {
                self.pos += 1;
                Ok(canonical_number(&n))
            }
            Some(Tok::Str(s)) => {
                self.pos += 1;
                Ok(crate::ty::quote(&s))
            }
            _ => Err(self.err("expected a literal parameter")),
        }
    }
}

/// A numeric literal as ClickHouse prints a parameter: integers as they are,
/// floats in their shortest form.
fn canonical_number(n: &str) -> String {
    if let Ok(i) = n.parse::<i128>() {
        return i.to_string();
    }
    match n.parse::<f64>() {
        Ok(f) => crate::text::float64(f),
        Err(_) => n.to_string(),
    }
}

fn int_arg(text: &str, arg: &Arg, what: &str) -> Result<u64, ChError> {
    match arg {
        Arg::Num(n) => n.parse::<u64>().map_err(|_| {
            ChError::bad_arguments(format!("{what} of {text} must be a non-negative integer"))
        }),
        _ => Err(ChError::bad_arguments(format!(
            "{what} of {text} must be a number"
        ))),
    }
}

fn one_type(text: &str, family: &str, args: Vec<Arg>) -> Result<ChType, ChError> {
    match <[Arg; 1]>::try_from(args) {
        Ok([Arg::Type(t)]) => Ok(t),
        _ => Err(ChError::number_of_arguments_doesnt_match(format!(
            "{family} data type family must have exactly one argument - nested type ({text})"
        ))),
    }
}

fn build(
    text: &str,
    written: &str,
    family: &'static str,
    args: Vec<Arg>,
) -> Result<ChType, ChError> {
    let no_args = |t: ChType| -> Result<ChType, ChError> {
        // SQL aliases take and ignore arguments (`VARCHAR(255)`, `FLOAT(10)`).
        if args.is_empty() || !written.eq(family) {
            Ok(t)
        } else {
            Err(ChError::bad_arguments(format!(
                "Data type {family} cannot have arguments ({text})"
            )))
        }
    };
    let decimal = |precision: u64, scale: u64| -> Result<ChType, ChError> {
        if !(1..=76).contains(&precision) || scale > precision {
            return Err(ChError::argument_out_of_bound(format!(
                "Decimal precision {precision} and scale {scale} are out of range ({text})"
            )));
        }
        Ok(ChType::Decimal {
            precision: precision as u8,
            scale: scale as u8,
        })
    };
    match family {
        "UInt8" => no_args(ChType::UInt8),
        "UInt16" => no_args(ChType::UInt16),
        "UInt32" => no_args(ChType::UInt32),
        "UInt64" => no_args(ChType::UInt64),
        "UInt128" => no_args(ChType::UInt128),
        "UInt256" => no_args(ChType::UInt256),
        "Int8" => no_args(ChType::Int8),
        "Int16" => no_args(ChType::Int16),
        "Int32" => no_args(ChType::Int32),
        "Int64" => no_args(ChType::Int64),
        "Int128" => no_args(ChType::Int128),
        "Int256" => no_args(ChType::Int256),
        "Float32" => no_args(ChType::Float32),
        "Float64" => no_args(ChType::Float64),
        "BFloat16" => no_args(ChType::BFloat16),
        "Bool" => no_args(ChType::Bool),
        "String" => Ok(ChType::String),
        "UUID" => no_args(ChType::Uuid),
        "IPv4" => no_args(ChType::IPv4),
        "IPv6" => no_args(ChType::IPv6),
        "Date" => no_args(ChType::Date),
        "Date32" => no_args(ChType::Date32),
        "Nothing" => no_args(ChType::Nothing),
        "FixedString" => match args.as_slice() {
            [n] => {
                let n = int_arg(text, n, "The size")?;
                if n == 0 || n > u64::from(u32::MAX) {
                    return Err(ChError::argument_out_of_bound(format!(
                        "FixedString size {n} is out of range"
                    )));
                }
                Ok(ChType::FixedString(n as u32))
            }
            _ => Err(ChError::number_of_arguments_doesnt_match(
                "FixedString data type family must have exactly one argument - size in bytes",
            )),
        },
        "Decimal" => match args.as_slice() {
            [] => decimal(10, 0),
            [p] => decimal(int_arg(text, p, "The precision")?, 0),
            [p, s] => decimal(
                int_arg(text, p, "The precision")?,
                int_arg(text, s, "The scale")?,
            ),
            _ => Err(ChError::number_of_arguments_doesnt_match(format!(
                "Decimal data type family must have one or two arguments ({text})"
            ))),
        },
        "Decimal32" | "Decimal64" | "Decimal128" | "Decimal256" => {
            let precision = match family {
                "Decimal32" => 9,
                "Decimal64" => 18,
                "Decimal128" => 38,
                _ => 76,
            };
            match args.as_slice() {
                [s] => decimal(precision, int_arg(text, s, "The scale")?),
                _ => Err(ChError::number_of_arguments_doesnt_match(format!(
                    "{family} data type family must have exactly one argument - scale ({text})"
                ))),
            }
        }
        "DateTime" | "DateTime32" => match args.as_slice() {
            [] => Ok(ChType::DateTime(None)),
            [Arg::Str(tz)] => Ok(ChType::DateTime(Some(tz.clone()))),
            [p] if family == "DateTime" => datetime64(text, p, None),
            [p, Arg::Str(tz)] if family == "DateTime" => datetime64(text, p, Some(tz.clone())),
            _ => Err(ChError::bad_arguments(format!(
                "Wrong arguments for {family} ({text})"
            ))),
        },
        "DateTime64" => match args.as_slice() {
            [p] => datetime64(text, p, None),
            [p, Arg::Str(tz)] => datetime64(text, p, Some(tz.clone())),
            _ => Err(ChError::number_of_arguments_doesnt_match(format!(
                "DateTime64 data type family must have one or two arguments ({text})"
            ))),
        },
        "Enum" | "Enum8" | "Enum16" => enumeration(text, family, args),
        "Nullable" => {
            let inner = one_type(text, family, args)?;
            if matches!(
                inner,
                ChType::Nullable(_)
                    | ChType::LowCardinality(_)
                    | ChType::Array(_)
                    | ChType::Map(..)
                    | ChType::Tuple(_)
                    | ChType::AggregateFunction(_)
                    | ChType::Json(_)
            ) {
                return Err(ChError::illegal_type_of_argument(format!(
                    "Nested type {inner} cannot be inside Nullable type"
                )));
            }
            Ok(ChType::Nullable(Box::new(inner)))
        }
        "LowCardinality" => Ok(ChType::LowCardinality(Box::new(one_type(
            text, family, args,
        )?))),
        "Array" => Ok(ChType::Array(Box::new(one_type(text, family, args)?))),
        "Map" => match <[Arg; 2]>::try_from(args) {
            Ok([Arg::Type(k), Arg::Type(v)]) => Ok(ChType::Map(Box::new(k), Box::new(v))),
            _ => Err(ChError::number_of_arguments_doesnt_match(format!(
                "Map data type family must have two arguments: key and value types ({text})"
            ))),
        },
        "Tuple" => {
            let mut elements = Vec::new();
            for a in args {
                match a {
                    Arg::Type(t) => elements.push((None, t)),
                    Arg::Named(n, t) => elements.push((Some(n), t)),
                    _ => {
                        return Err(ChError::bad_arguments(format!(
                            "Tuple elements must be types ({text})"
                        )));
                    }
                }
            }
            let named = elements.iter().filter(|(n, _)| n.is_some()).count();
            if named != 0 && named != elements.len() {
                return Err(ChError::bad_arguments(
                    "Names are specified not for all elements of Tuple type",
                ));
            }
            if elements.is_empty() {
                return Err(ChError::not_implemented(
                    "An empty Tuple() is not supported by the Loams House engine",
                ));
            }
            Ok(ChType::Tuple(elements))
        }
        "Nested" => {
            let mut elements = Vec::new();
            for a in args {
                match a {
                    Arg::Named(n, t) => elements.push((n, t)),
                    _ => {
                        return Err(ChError::bad_arguments(format!(
                            "Nested elements must be named ({text})"
                        )));
                    }
                }
            }
            Ok(ChType::Nested(elements))
        }
        "JSON" => {
            let mut params = JsonParams::default();
            for a in args {
                match a {
                    Arg::Assign(name, value) => params.settings.push((
                        name,
                        value.parse().map_err(|_| {
                            ChError::bad_arguments(format!("Bad JSON setting value in {text}"))
                        })?,
                    )),
                    Arg::Named(path, t) => params.typed_paths.push((path, t)),
                    Arg::SkipPath(p) => params.skip_paths.push(p),
                    Arg::SkipRegexp(r) => params.skip_regexps.push(r),
                    _ => {
                        return Err(ChError::bad_arguments(format!(
                            "Bad JSON type parameter in {text}"
                        )));
                    }
                }
            }
            params.typed_paths.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(ChType::Json(params))
        }
        "AggregateFunction" | "SimpleAggregateFunction" => {
            let mut it = args.into_iter().peekable();
            let mut version = None;
            if let Some(Arg::Num(n)) = it.peek() {
                version = Some(
                    n.parse()
                        .map_err(|_| ChError::bad_arguments(format!("Bad version in {text}")))?,
                );
                it.next();
            }
            let (function, params) = match it.next() {
                Some(Arg::Function(f, p)) => (f, p),
                _ => {
                    return Err(ChError::bad_arguments(format!(
                        "{family} data type family must have a function name as its first argument ({text})"
                    )));
                }
            };
            let mut types = Vec::new();
            for a in it {
                match a {
                    Arg::Type(t) => types.push(t),
                    _ => {
                        return Err(ChError::bad_arguments(format!(
                            "Arguments of {family} must be types ({text})"
                        )));
                    }
                }
            }
            let spec = AggregateSpec {
                version,
                function,
                params,
                args: types,
            };
            if family == "SimpleAggregateFunction" {
                if spec.args.len() != 1 || spec.version.is_some() {
                    return Err(ChError::number_of_arguments_doesnt_match(format!(
                        "SimpleAggregateFunction takes a function and one type ({text})"
                    )));
                }
                Ok(ChType::SimpleAggregateFunction(spec))
            } else {
                Ok(ChType::AggregateFunction(spec))
            }
        }
        f if f.starts_with("Interval") => {
            no_args(ChType::Interval(f["Interval".len()..].to_string()))
        }
        other => {
            // Known to ClickHouse, not modelled by the engine: keep the canonical text.
            let mut out = other.to_string();
            if !args.is_empty() {
                let parts: Vec<String> = args
                    .iter()
                    .map(|a| match a {
                        Arg::Type(t) => t.to_compact_string(),
                        Arg::Named(n, t) => format!(
                            "{} {}",
                            crate::ty::quote_identifier(n),
                            t.to_compact_string()
                        ),
                        Arg::Str(s) => crate::ty::quote(s),
                        Arg::Num(n) => n.clone(),
                        Arg::Assign(n, v) => format!("{n}={v}"),
                        Arg::SkipPath(p) => format!("SKIP {p}"),
                        Arg::SkipRegexp(r) => format!("SKIP REGEXP {}", crate::ty::quote(r)),
                        Arg::Function(f, p) if p.is_empty() => f.clone(),
                        Arg::Function(f, p) => format!("{f}({})", p.join(", ")),
                    })
                    .collect();
                out = format!("{out}({})", parts.join(", "));
            }
            Ok(ChType::Other(out))
        }
    }
}

fn datetime64(text: &str, precision: &Arg, tz: Option<String>) -> Result<ChType, ChError> {
    let p = int_arg(text, precision, "The precision")?;
    if p > 9 {
        return Err(ChError::argument_out_of_bound(format!(
            "Precision {p} is out of bounds for DateTime64, it must be from 0 to 9"
        )));
    }
    Ok(ChType::DateTime64(p as u8, tz))
}

fn enumeration(text: &str, family: &str, args: Vec<Arg>) -> Result<ChType, ChError> {
    let mut values: Vec<(String, i64)> = Vec::new();
    let mut next = 1i64;
    let explicit = args.iter().any(|a| matches!(a, Arg::Assign(..)));
    for a in args {
        match a {
            Arg::Assign(name, value) => {
                let v: i64 = value.parse().map_err(|_| {
                    ChError::bad_arguments(format!("Bad value of {name} in {text}"))
                })?;
                values.push((name, v));
                next = v + 1;
            }
            Arg::Str(name) if !explicit => {
                values.push((name, next));
                next += 1;
            }
            _ => {
                return Err(ChError::bad_arguments(format!(
                    "Elements of Enum data type must be of form: 'name' = number or 'name' ({text})"
                )));
            }
        }
    }
    if values.is_empty() {
        return Err(ChError::number_of_arguments_doesnt_match(
            "Enum data type cannot be empty",
        ));
    }
    let fits8 = values.iter().all(|(_, v)| i8::try_from(*v).is_ok());
    let wide = match family {
        "Enum8" => false,
        "Enum16" => true,
        _ => !fits8,
    };
    if wide {
        let mut out = Vec::new();
        for (n, v) in values {
            out.push((
                n,
                i16::try_from(v).map_err(|_| {
                    ChError::argument_out_of_bound(format!("Value {v} is out of range for Enum16"))
                })?,
            ));
        }
        Ok(ChType::Enum16(out))
    } else {
        let mut out = Vec::new();
        for (n, v) in values {
            out.push((
                n,
                i8::try_from(v).map_err(|_| {
                    ChError::argument_out_of_bound(format!("Value {v} is out of range for Enum8"))
                })?,
            ));
        }
        Ok(ChType::Enum8(out))
    }
}
