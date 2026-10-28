//! RE2 patterns on the `regex` crate (plan HE1 Task 31, R31.3).
//!
//! ClickHouse compiles `match`, `replaceRegexp*`, `like` and friends with RE2
//! in UTF-8 mode with `.` matching `\n` (`OptimizedRegularExpression`,
//! `src/Common/OptimizedRegularExpression.cpp`). The `regex` crate reads most
//! of RE2's syntax the same way but not all of it, so [`translate`] parses the
//! pattern as RE2 does and writes it in `regex`'s syntax with RE2's meaning:
//!
//! * `\d`, `\w`, `\s`, `\b` are ASCII in RE2 and Unicode in `regex`: written
//!   as ASCII classes (`\s` is `[\t\n\f\r ]`, without `\v`, as in RE2);
//! * inside a class, `regex` reads `[`, `&&`, `--` and `~~` as nesting and set
//!   operations, RE2 as literals: each is escaped;
//! * `\<` and `\>` are word boundaries in `regex` and literals in RE2;
//! * `{` that does not start a repetition is a literal in RE2 (`a{,2}`);
//! * `\Q…\E` (RE2 only) is written as escaped literals.
//!
//! What RE2 rejects is `427 CANNOT_COMPILE_REGEXP` here too: repetitions above
//! 1000, a repetition of a repetition (`a**`), escapes RE2 does not know
//! (`\Z`, `\1`), flags other than `imsU`, and look-around. A pattern RE2 takes
//! but that would need more than a rewrite (`\C`, octal escapes, Unicode
//! property names other than general categories and scripts) is refused with
//! `36 BAD_ARGUMENTS`, never read differently.

use loams_house_types::ChError;

/// `427 CANNOT_COMPILE_REGEXP`, as RE2's rejection.
pub fn cannot_compile(pattern: &str, why: &str) -> ChError {
    ChError {
        code: 427,
        name: "CANNOT_COMPILE_REGEXP",
        message: format!("OptimizedRegularExpression: cannot compile re2: {pattern}, error: {why}"),
    }
}

/// `36`: a pattern the engine will not read differently from RE2.
fn refused(pattern: &str, what: &str) -> ChError {
    ChError::bad_arguments(format!(
        "The regular expression {pattern} uses {what}, which the Loams House engine does not support (RE2 and Rust's regex differ there; R31.3)"
    ))
}

/// The most RE2 allows in a counted repetition.
const MAX_REPEAT: u32 = 1000;

/// The scripts RE2 names in `\p{…}` that `regex` names the same way. Other
/// long names (`regex` also takes binary properties such as `Alphabetic`,
/// which RE2 rejects) are refused.
const SCRIPTS: &[&str] = &[
    "Any",
    "Arabic",
    "Armenian",
    "Bengali",
    "Cherokee",
    "Common",
    "Cyrillic",
    "Devanagari",
    "Ethiopic",
    "Georgian",
    "Greek",
    "Gujarati",
    "Gurmukhi",
    "Han",
    "Hangul",
    "Hebrew",
    "Hiragana",
    "Inherited",
    "Kannada",
    "Katakana",
    "Khmer",
    "Lao",
    "Latin",
    "Malayalam",
    "Mongolian",
    "Myanmar",
    "Oriya",
    "Sinhala",
    "Tamil",
    "Telugu",
    "Thaana",
    "Thai",
    "Tibetan",
];

/// The general-category letters RE2 takes (`\pL`, `\p{Lu}`).
const CATEGORIES: &[&str] = &[
    "C", "Cc", "Cf", "Co", "Cs", "L", "Ll", "Lm", "Lo", "Lt", "Lu", "M", "Mc", "Me", "Mn", "N",
    "Nd", "Nl", "No", "P", "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "S", "Sc", "Sk", "Sm", "So",
    "Z", "Zl", "Zp", "Zs",
];

const DIGIT: &str = "0-9";
const WORD: &str = "0-9A-Za-z_";
const SPACE: &str = r"\t\n\x0C\r ";

/// `c` as a literal outside a class.
fn literal(c: char, out: &mut String) {
    if r"\.+*?()|[]{}^$#&-~".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

/// `c` as a literal inside a class.
fn class_literal(c: char, out: &mut String) {
    if r"\[]^&~-".contains(c) {
        out.push('\\');
    }
    out.push(c);
}

struct Parser<'a> {
    pattern: &'a str,
    chars: Vec<char>,
    i: usize,
    out: String,
}

/// What the last thing written was, for RE2's repetition rules.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Last {
    /// Nothing a repetition can apply to (the start, `(`, `|`).
    Nothing,
    /// An atom.
    Atom,
    /// A repetition (`*`, `+`, `?`, `{n,m}`).
    Repeat,
    /// A lazy repetition (`*?`).
    Lazy,
}

impl Parser<'_> {
    fn peek(&self, k: usize) -> Option<char> {
        self.chars.get(self.i + k).copied()
    }

    fn fail(&self, why: &str) -> ChError {
        cannot_compile(self.pattern, why)
    }

    fn run(&mut self) -> Result<(), ChError> {
        let mut last = Last::Nothing;
        while let Some(c) = self.peek(0) {
            self.i += 1;
            match c {
                '\\' => {
                    last = if self.escape()? {
                        Last::Atom
                    } else {
                        Last::Nothing
                    };
                }
                '[' => {
                    self.class()?;
                    last = Last::Atom;
                }
                '(' => {
                    self.group()?;
                    last = Last::Nothing;
                }
                ')' => {
                    self.out.push(')');
                    last = Last::Atom;
                }
                '|' => {
                    self.out.push('|');
                    last = Last::Nothing;
                }
                '*' | '+' | '?' => {
                    match last {
                        Last::Repeat if c == '?' => {
                            self.out.push('?');
                            last = Last::Lazy;
                            continue;
                        }
                        Last::Repeat | Last::Lazy => {
                            return Err(self.fail("bad repetition operator"));
                        }
                        Last::Nothing => {
                            return Err(self.fail("missing argument to repetition operator"));
                        }
                        Last::Atom => {}
                    }
                    self.out.push(c);
                    last = Last::Repeat;
                }
                '{' => match self.repetition()? {
                    Some(text) => {
                        match last {
                            Last::Repeat | Last::Lazy => {
                                return Err(self.fail("bad repetition operator"));
                            }
                            Last::Nothing => {
                                return Err(self.fail("missing argument to repetition operator"));
                            }
                            Last::Atom => {}
                        }
                        self.out.push_str(&text);
                        last = Last::Repeat;
                    }
                    None => {
                        self.out.push_str(r"\{");
                        last = Last::Atom;
                    }
                },
                '.' => {
                    self.out.push('.');
                    last = Last::Atom;
                }
                '^' | '$' => {
                    self.out.push(c);
                    last = Last::Nothing;
                }
                other => {
                    literal(other, &mut self.out);
                    last = Last::Atom;
                }
            }
        }
        Ok(())
    }

    /// `{n}`, `{n,}` or `{n,m}` at the cursor (after `{`), as text; `None` when
    /// the `{` is a literal.
    fn repetition(&mut self) -> Result<Option<String>, ChError> {
        let rest: String = self.chars[self.i..].iter().collect();
        let Some(end) = rest.find('}') else {
            return Ok(None);
        };
        let body = &rest[..end];
        let (lo, hi) = match body.split_once(',') {
            Some((lo, hi)) => (lo, Some(hi)),
            None => (body, None),
        };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(lo) || hi.is_some_and(|h| !h.is_empty() && !digits(h)) {
            return Ok(None);
        }
        let n: u32 = lo.parse().unwrap_or(u32::MAX);
        let m: Option<u32> = match hi {
            Some("") | None => None,
            Some(h) => Some(h.parse().unwrap_or(u32::MAX)),
        };
        if n > MAX_REPEAT || m.is_some_and(|m| m > MAX_REPEAT) || m.is_some_and(|m| m < n) {
            return Err(self.fail(&format!("invalid repetition size: {{{body}}}")));
        }
        self.i += body.chars().count() + 1;
        Ok(Some(format!("{{{body}}}")))
    }

    /// An escape outside a class (after `\`); whether it is an atom.
    fn escape(&mut self) -> Result<bool, ChError> {
        let Some(c) = self.peek(0) else {
            return Err(self.fail("trailing \\"));
        };
        self.i += 1;
        match c {
            'd' => self.out.push_str(&format!("[{DIGIT}]")),
            'D' => self.out.push_str(&format!("[^{DIGIT}]")),
            'w' => self.out.push_str(&format!("[{WORD}]")),
            'W' => self.out.push_str(&format!("[^{WORD}]")),
            's' => self.out.push_str(&format!("[{SPACE}]")),
            'S' => self.out.push_str(&format!("[^{SPACE}]")),
            'b' => {
                self.out.push_str(r"(?-u:\b)");
                return Ok(false);
            }
            'B' => {
                self.out.push_str(r"(?-u:\B)");
                return Ok(false);
            }
            'A' | 'z' => {
                self.out.push('\\');
                self.out.push(c);
                return Ok(false);
            }
            'p' | 'P' => self.property(c)?,
            'x' => {
                let c = self.hex()?;
                self.out.push_str(&format!(r"\x{{{:X}}}", c as u32));
            }
            'a' | 'f' | 't' | 'n' | 'r' | 'v' => {
                self.out.push('\\');
                self.out.push(c);
            }
            'Q' => {
                // Literal text up to `\E` or the end.
                while let Some(q) = self.peek(0) {
                    if q == '\\' && self.peek(1) == Some('E') {
                        self.i += 2;
                        break;
                    }
                    literal(q, &mut self.out);
                    self.i += 1;
                }
            }
            // `\0` and `\1`–`\7` before another digit are octal in RE2;
            // a lone `\1`–`\9` is a backreference, which RE2 rejects.
            '0' => return Err(refused(self.pattern, "an octal escape")),
            '1'..='7' if self.peek(0).is_some_and(|d| d.is_ascii_digit()) => {
                return Err(refused(self.pattern, "an octal escape"));
            }
            'C' => return Err(refused(self.pattern, "\\C (any byte)")),
            c if c.is_ascii_punctuation() => literal(c, &mut self.out),
            c => return Err(self.fail(&format!("invalid escape sequence: \\{c}"))),
        }
        Ok(true)
    }

    /// `\p…`/`\P…` (after the letter).
    fn property(&mut self, letter: char) -> Result<(), ChError> {
        let name: String = match self.peek(0) {
            Some('{') => {
                let rest: String = self.chars[self.i + 1..].iter().collect();
                let Some(end) = rest.find('}') else {
                    return Err(self.fail("invalid character class range"));
                };
                self.i += rest[..end].chars().count() + 2;
                rest[..end].to_string()
            }
            Some(c) => {
                self.i += 1;
                c.to_string()
            }
            None => return Err(self.fail("invalid character class range")),
        };
        let bare = name.strip_prefix('^').unwrap_or(&name);
        if !CATEGORIES.contains(&bare) && !SCRIPTS.contains(&bare) {
            return Err(refused(self.pattern, &format!("the Unicode class {name}")));
        }
        self.out.push('\\');
        self.out.push(letter);
        self.out.push('{');
        self.out.push_str(&name);
        self.out.push('}');
        Ok(())
    }

    /// `\xHH` or `\x{H…}` (after the `x`): the character.
    fn hex(&mut self) -> Result<char, ChError> {
        let text: String = if self.peek(0) == Some('{') {
            let rest: String = self.chars[self.i + 1..].iter().collect();
            let Some(end) = rest.find('}') else {
                return Err(self.fail("invalid escape sequence: \\x"));
            };
            self.i += rest[..end].chars().count() + 2;
            rest[..end].to_string()
        } else {
            let h: String = self.chars[self.i..].iter().take(2).collect();
            self.i += h.chars().count();
            h
        };
        let ok = !text.is_empty() && text.chars().all(|c| c.is_ascii_hexdigit());
        let Some(c) = u32::from_str_radix(&text, 16)
            .ok()
            .filter(|_| ok)
            .and_then(char::from_u32)
        else {
            return Err(self.fail("invalid escape sequence: \\x"));
        };
        Ok(c)
    }

    /// A group's opening (after `(`).
    fn group(&mut self) -> Result<(), ChError> {
        if self.peek(0) != Some('?') {
            self.out.push('(');
            return Ok(());
        }
        // `(?:`, `(?P<name>`, `(?<name>`, or flags.
        let rest: String = self.chars[self.i + 1..].iter().collect();
        if rest.starts_with(':') {
            self.i += 2;
            self.out.push_str("(?:");
            return Ok(());
        }
        let named = rest
            .strip_prefix("P<")
            .map(|r| (r, 2))
            .or_else(|| rest.strip_prefix('<').map(|r| (r, 1)));
        if let Some((r, skip)) = named
            && !r.starts_with('=')
            && !r.starts_with('!')
        {
            let Some(end) = r.find('>') else {
                return Err(self.fail("invalid named capture group"));
            };
            let name = &r[..end];
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(self.fail("invalid named capture group"));
            }
            self.i += 1 + skip + name.chars().count() + 1;
            self.out.push_str(&format!("(?P<{name}>"));
            return Ok(());
        }
        // Flags: `(?i)`, `(?i-s:`.
        let mut body = String::new();
        let mut negated = false;
        let mut seen = false;
        for c in rest.chars() {
            match c {
                'i' | 'm' | 's' | 'U' => {
                    seen = true;
                    body.push(c);
                }
                '-' if !negated => {
                    negated = true;
                    body.push(c);
                }
                // `(?i)` sets flags for the rest of the group; `(?i:` opens one.
                ':' | ')' if seen && !body.ends_with('-') => {
                    self.i += 1 + body.chars().count() + 1;
                    self.out.push_str("(?");
                    self.out.push_str(&body);
                    self.out.push(c);
                    return Ok(());
                }
                _ => break,
            }
        }
        Err(self.fail("invalid perl operator: (?"))
    }

    /// A class (after `[`).
    fn class(&mut self) -> Result<(), ChError> {
        self.out.push('[');
        if self.peek(0) == Some('^') {
            self.out.push('^');
            self.i += 1;
        }
        let mut first = true;
        loop {
            let Some(c) = self.peek(0) else {
                return Err(self.fail("missing ]"));
            };
            if c == ']' && !first {
                self.i += 1;
                self.out.push(']');
                return Ok(());
            }
            first = false;
            // `[:alpha:]`
            if c == '['
                && self.peek(1) == Some(':')
                && let Some(len) = self.posix_len()
            {
                let text: String = self.chars[self.i..self.i + len].iter().collect();
                self.out.push_str(&text);
                self.i += len;
                continue;
            }
            let lo = self.class_item()?;
            // A range `lo-hi`, unless the `-` is the last thing in the class.
            if self.peek(0) == Some('-')
                && self.peek(1).is_some_and(|n| n != ']')
                && let ClassItem::Char(lo) = lo
            {
                self.i += 1;
                match self.class_item()? {
                    ClassItem::Char(hi) => {
                        if hi < lo {
                            return Err(self.fail("invalid character class range"));
                        }
                        class_literal(lo, &mut self.out);
                        self.out.push('-');
                        class_literal(hi, &mut self.out);
                    }
                    ClassItem::Set(_) => {
                        return Err(self.fail("invalid character class range"));
                    }
                }
                continue;
            }
            match lo {
                ClassItem::Char(c) => class_literal(c, &mut self.out),
                ClassItem::Set(s) => self.out.push_str(&s),
            }
        }
    }

    /// The length of a `[:name:]` at the cursor, if it is one.
    fn posix_len(&self) -> Option<usize> {
        let rest: String = self.chars[self.i..].iter().collect();
        let end = rest[2..].find(":]")?;
        let name = rest[2..2 + end].trim_start_matches('^');
        const NAMES: &[&str] = &[
            "alnum", "alpha", "ascii", "blank", "cntrl", "digit", "graph", "lower", "print",
            "punct", "space", "upper", "word", "xdigit",
        ];
        NAMES
            .contains(&name)
            .then(|| rest[..2 + end + 2].chars().count())
    }

    /// One item of a class: a character, or a set (`\d`, `\pL`).
    fn class_item(&mut self) -> Result<ClassItem, ChError> {
        let Some(c) = self.peek(0) else {
            return Err(self.fail("missing ]"));
        };
        self.i += 1;
        if c != '\\' {
            return Ok(ClassItem::Char(c));
        }
        let Some(e) = self.peek(0) else {
            return Err(self.fail("trailing \\"));
        };
        self.i += 1;
        Ok(match e {
            'd' => ClassItem::Set(DIGIT.to_string()),
            'D' => ClassItem::Set(format!("[^{DIGIT}]")),
            'w' => ClassItem::Set(WORD.to_string()),
            'W' => ClassItem::Set(format!("[^{WORD}]")),
            's' => ClassItem::Set(SPACE.to_string()),
            'S' => ClassItem::Set(format!("[^{SPACE}]")),
            'p' | 'P' => {
                let mark = self.out.len();
                self.property(e)?;
                let text = self.out[mark..].to_string();
                self.out.truncate(mark);
                ClassItem::Set(text)
            }
            'x' => ClassItem::Char(self.hex()?),
            'a' => ClassItem::Char('\x07'),
            'f' => ClassItem::Char('\x0C'),
            't' => ClassItem::Char('\t'),
            'n' => ClassItem::Char('\n'),
            'r' => ClassItem::Char('\r'),
            'v' => ClassItem::Char('\x0B'),
            '0' => return Err(refused(self.pattern, "an octal escape")),
            '1'..='7' if self.peek(0).is_some_and(|d| d.is_ascii_digit()) => {
                return Err(refused(self.pattern, "an octal escape"));
            }
            c if c.is_ascii_punctuation() => ClassItem::Char(c),
            c => return Err(self.fail(&format!("invalid escape sequence: \\{c}"))),
        })
    }
}

enum ClassItem {
    Char(char),
    Set(String),
}

/// `pattern` (RE2) in `regex`'s syntax, with ClickHouse's flags: `.` matches
/// `\n`, and `(?i)` when `case_insensitive`.
pub fn translate(pattern: &str, case_insensitive: bool) -> Result<String, ChError> {
    let mut p = Parser {
        pattern,
        chars: pattern.chars().collect(),
        i: 0,
        out: String::from(if case_insensitive { "(?is)" } else { "(?s)" }),
    };
    p.run()?;
    Ok(p.out)
}

/// A compiled RE2 pattern.
pub fn compile(pattern: &str, case_insensitive: bool) -> Result<regex::bytes::Regex, ChError> {
    let text = translate(pattern, case_insensitive)?;
    regex::bytes::RegexBuilder::new(&text)
        .build()
        .map_err(|e| cannot_compile(pattern, &e.to_string().replace('\n', " ")))
}

/// A `LIKE` pattern as a regular expression in `regex`'s syntax, as
/// measured on the oracle: `%` is any text, `_` any one character, `\%`,
/// `\_` and `\\` are literals, a backslash before anything else is a literal
/// backslash followed by that character, and a backslash at the end is
/// `25 CANNOT_PARSE_ESCAPE_SEQUENCE`. `ILIKE` folds case as RE2 does.
pub fn like_regex(pattern: &str, case_insensitive: bool) -> Result<String, ChError> {
    let mut out = String::from(if case_insensitive {
        r"(?is)\A(?:"
    } else {
        r"(?s)\A(?:"
    });
    let mut chars = pattern.chars();
    let mut buf = [0u8; 4];
    while let Some(c) = chars.next() {
        match c {
            '%' => out.push_str(".*"),
            '_' => out.push('.'),
            '\\' => match chars.next() {
                None => {
                    return Err(ChError {
                        code: 25,
                        name: "CANNOT_PARSE_ESCAPE_SEQUENCE",
                        message: format!(
                            "Invalid escape sequence at the end of LIKE pattern '{pattern}'"
                        ),
                    });
                }
                Some(e @ ('%' | '_' | '\\')) => {
                    out.push_str(&regex::escape(e.encode_utf8(&mut buf)))
                }
                Some(e) => {
                    out.push_str(r"\\");
                    out.push_str(&regex::escape(e.encode_utf8(&mut buf)));
                }
            },
            other => out.push_str(&regex::escape(other.encode_utf8(&mut buf))),
        }
    }
    out.push_str(r")\z");
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn m(pattern: &str, text: &str) -> bool {
        compile(pattern, false).unwrap().is_match(text.as_bytes())
    }

    #[test]
    fn perl_classes_are_ascii_as_in_re2() {
        assert!(!m(r"^\d$", "٣"));
        assert!(!m(r"^\w$", "é"));
        assert!(m(r"^.$", "é"));
        assert!(!m(r"^\s$", "\x0B"));
        assert!(m(r"\bx", "éx"));
        assert!(m("a.b", "a\nb"));
        assert!(!m("^b", "a\nb"));
        assert!(m(r"^[\d]+$", "123"));
        assert!(!m(r"^[\w]$", "é"));
    }

    #[test]
    fn re2_literals_stay_literals() {
        assert!(m(r"\<a", "<a"));
        assert!(m("a{,2}", "a{,2}"));
        assert!(m(r"\Qa.b\E", "a.b"));
        assert!(!m(r"\Qa.b\E", "axb"));
        assert!(m("[a[]", "["));
        assert!(m("[a&&b]", "&"));
        assert!(m("[a-]", "-"));
        assert!(m("[]a]", "]"));
        assert!(m("(?i)ǆ", "ǅ"));
        assert!(m(r"[[:^alpha:]]", "."));
    }

    #[test]
    fn re2_rejections_are_427_and_differences_are_36() {
        for p in [
            "a**", "a{2}{3}", r"\Z", r"\8", r"(a)\1", "x{1001}", "(?x)a b", "(?=a)", "[a",
        ] {
            assert_eq!(compile(p, false).unwrap_err().code, 427, "{p}");
        }
        for p in [r"\C", r"\141", r"\p{Alphabetic}"] {
            assert_eq!(compile(p, false).unwrap_err().code, 36, "{p}");
        }
        for p in [
            "a*?", "a{2,3}?", "(?i)a", "(?i:a)", "(?P<n>a)", "(?<n>a)", r"\pL", "a??",
        ] {
            assert!(compile(p, false).is_ok(), "{p}");
        }
    }

    #[test]
    fn like_patterns_as_measured() {
        let like = |p: &str, s: &str| {
            regex::bytes::Regex::new(&like_regex(p, false).unwrap())
                .unwrap()
                .is_match(s.as_bytes())
        };
        assert!(like(r"a\%b", "a%b"));
        assert!(!like(r"a\%b", "axb"));
        assert!(like(r"a\bc", r"a\bc"));
        assert!(!like(r"a\bc", "abc"));
        assert!(like("_", "ä"));
        assert!(like("a_b", "a\nb"));
        assert!(like("a.b", "a.b"));
        assert!(!like("a.b", "axb"));
        assert_eq!(like_regex("ab\\", false).unwrap_err().code, 25);
    }
}
