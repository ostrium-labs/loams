//! The oracle's corpus: `corpora.toml`, each corpus's setup and its `.sql`
//! cases.
//!
//! ```toml
//! [[corpus]]
//! name = "select"                       # case ids are "select/<file stem>"
//! setup = "corpus/select/setup.sql"     # run on every target first
//! cases = "../corpus/select"            # a directory of .sql files, one query each
//! ```
//!
//! Paths are relative to `corpora.toml`. A case may declare, in comment lines:
//!
//! - `-- oracle: precision = 6`: significant digits for float columns;
//! - `-- oracle: order = unordered` (or `ordered`): overrides the detection,
//!   which treats a case as ordered when its outermost query has `ORDER BY`;
//! - `-- oracle: format = JSON`: the answer is read in that output format and
//!   compared as bytes, after removing `JSON*`'s `statistics` (plan HE1 Task
//!   36), instead of as `JSONCompactEachRowWithNamesAndTypes` values.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::Deserialize;

use super::normalise::{Order, Precision};

/// `corpora.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corpora {
    #[serde(rename = "corpus", default)]
    pub corpora: Vec<CorpusEntry>,
}

/// One `[[corpus]]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusEntry {
    pub name: String,
    /// Statements run on each target before the cases, `;`-separated.
    #[serde(default)]
    pub setup: Option<PathBuf>,
    /// The directory of `.sql` cases.
    pub cases: PathBuf,
}

/// A corpus read from disk.
#[derive(Debug)]
pub struct Corpus {
    pub name: String,
    pub setup: Vec<String>,
    pub cases: Vec<Case>,
}

/// One case.
#[derive(Debug, Clone)]
pub struct Case {
    /// `<corpus>/<file stem>`.
    pub id: String,
    /// The query, without a trailing `;`.
    pub sql: String,
    pub order: Order,
    pub precision: Precision,
    /// The output format compared as bytes, when the case names one.
    pub format: Option<String>,
}

/// Reads `corpora.toml` and every corpus it names.
pub fn load(path: &Path) -> anyhow::Result<Vec<Corpus>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let corpora: Corpora =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    corpora
        .corpora
        .into_iter()
        .map(|entry| {
            let setup = match &entry.setup {
                Some(file) => {
                    let file = base.join(file);
                    let text = std::fs::read_to_string(&file)
                        .with_context(|| format!("reading {}", file.display()))?;
                    split_statements(&text)
                }
                None => Vec::new(),
            };
            let dir = base.join(&entry.cases);
            let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                .with_context(|| format!("reading {}", dir.display()))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "sql"))
                .collect();
            files.sort();
            let cases = files
                .iter()
                .map(|file| {
                    let stem = file
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .context("a case file name")?;
                    let text = std::fs::read_to_string(file)
                        .with_context(|| format!("reading {}", file.display()))?;
                    parse_case(&format!("{}/{stem}", entry.name), &text)
                        .with_context(|| format!("in {}", file.display()))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            Ok(Corpus {
                name: entry.name,
                setup,
                cases,
            })
        })
        .collect()
}

/// Reads one case's text: its directives, its query and its order.
pub fn parse_case(id: &str, text: &str) -> anyhow::Result<Case> {
    let mut precision = Precision::default();
    let mut order = None;
    let mut format = None;
    for line in text.lines() {
        let Some(directive) = line.trim().strip_prefix("-- oracle:") else {
            continue;
        };
        let Some((key, value)) = directive.split_once('=') else {
            bail!("a directive is `-- oracle: key = value`: {line:?}");
        };
        match (key.trim(), value.trim()) {
            ("precision", digits) => {
                let digits: usize = digits
                    .parse()
                    .with_context(|| format!("precision {digits:?}"))?;
                if !(1..=17).contains(&digits) {
                    bail!("precision is 1 to 17 significant digits, not {digits}");
                }
                precision = Precision::uniform(digits);
            }
            ("order", "ordered") => order = Some(Order::Ordered),
            ("order", "unordered") => order = Some(Order::Unordered),
            ("format", name)
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                format = Some(name.to_string());
            }
            (key, value) => bail!("unknown directive {key} = {value}"),
        }
    }
    let statements = split_statements(text);
    let [sql] = statements.as_slice() else {
        bail!("a case is one statement, not {}", statements.len());
    };
    let tokens = top_level_words(sql);
    if tokens
        .iter()
        .any(|(w, next_is_paren)| w.eq_ignore_ascii_case("FORMAT") && !next_is_paren)
    {
        bail!("a case has no FORMAT clause: the oracle sets the format");
    }
    let order = order.unwrap_or_else(|| detect_order(sql));
    Ok(Case {
        id: id.to_string(),
        sql: sql.clone(),
        order,
        precision,
        format,
    })
}

/// `Ordered` when the outermost query has `ORDER BY` (outside parentheses: a
/// window's or a subquery's `ORDER BY` does not order the result).
pub fn detect_order(sql: &str) -> Order {
    let words = top_level_words(sql);
    let ordered = words
        .windows(2)
        .any(|w| w[0].0.eq_ignore_ascii_case("ORDER") && w[1].0.eq_ignore_ascii_case("BY"));
    if ordered {
        Order::Ordered
    } else {
        Order::Unordered
    }
}

/// The words at parenthesis depth 0, outside comments, strings and quoted
/// identifiers, each with whether a `(` follows it directly.
fn top_level_words(sql: &str) -> Vec<(String, bool)> {
    let chars: Vec<char> = sql.chars().collect();
    let mut words = Vec::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '\'' | '"' | '`' => i = skip_quoted(&chars, i),
            '(' => {
                depth += 1;
                i += 1;
            }
            ')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                if depth == 0 {
                    let mut j = i;
                    while j < chars.len() && chars[j].is_whitespace() {
                        j += 1;
                    }
                    words.push((chars[start..i].iter().collect(), chars.get(j) == Some(&'(')));
                }
            }
            _ => i += 1,
        }
    }
    words
}

/// The index just past the quoted run that opens at `start` (`''` and a
/// backslash escape a quote inside it).
fn skip_quoted(chars: &[char], start: usize) -> usize {
    let quote = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 2;
            continue;
        }
        if chars[i] == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    i
}

/// Splits a script at the `;` outside comments and strings; drops empty
/// statements and statements that are only comments.
pub fn split_statements(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut statements = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut has_code = false;
    while i < chars.len() {
        match chars[i] {
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '\'' | '"' | '`' => {
                has_code = true;
                i = skip_quoted(&chars, i);
            }
            ';' => {
                if has_code {
                    statements.push(
                        chars[start..i]
                            .iter()
                            .collect::<String>()
                            .trim()
                            .to_string(),
                    );
                }
                has_code = false;
                i += 1;
                start = i;
            }
            c => {
                if !c.is_whitespace() {
                    has_code = true;
                }
                i += 1;
            }
        }
    }
    if has_code {
        statements.push(chars[start..].iter().collect::<String>().trim().to_string());
    }
    statements
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_detected_on_the_outermost_query_only() {
        assert_eq!(detect_order("SELECT g FROM t ORDER BY g"), Order::Ordered);
        assert_eq!(detect_order("SELECT g FROM t"), Order::Unordered);
        assert_eq!(
            detect_order("SELECT sum(k) OVER (PARTITION BY g ORDER BY k) FROM t"),
            Order::Unordered
        );
        assert_eq!(
            detect_order("SELECT * FROM (SELECT k FROM t ORDER BY k LIMIT 3)"),
            Order::Unordered
        );
        assert_eq!(
            detect_order("SELECT 'ORDER BY' FROM t -- ORDER BY"),
            Order::Unordered
        );
        assert_eq!(
            detect_order("WITH x AS (SELECT 1 ORDER BY 1) SELECT * FROM x ORDER BY 1"),
            Order::Ordered
        );
    }

    #[test]
    fn a_case_is_one_statement_without_format() {
        let case =
            parse_case("c/a", "-- a comment\n-- oracle: precision = 4\nSELECT 1;\n").expect("case");
        assert_eq!(case.sql, "-- a comment\n-- oracle: precision = 4\nSELECT 1");
        assert_eq!(case.precision, Precision::uniform(4));
        assert!(parse_case("c/b", "SELECT 1; SELECT 2").is_err());
        assert!(parse_case("c/c", "SELECT 1 FORMAT JSON").is_err());
        assert!(parse_case("c/d", "SELECT format('{}', 1)").is_ok());
        assert!(parse_case("c/e", "-- oracle: colour = blue\nSELECT 1").is_err());
        let unordered =
            parse_case("c/f", "-- oracle: order = unordered\nSELECT 1 ORDER BY 1").expect("case");
        assert_eq!(unordered.order, Order::Unordered);
        let json = parse_case("c/g", "-- oracle: format = JSON\nSELECT 1").expect("case");
        assert_eq!(json.format.as_deref(), Some("JSON"));
        assert!(parse_case("c/h", "-- oracle: format = JSON; DROP\nSELECT 1").is_err());
        assert_eq!(parse_case("c/i", "SELECT 1").expect("case").format, None);
    }

    #[test]
    fn setup_scripts_split_outside_strings() {
        let s = split_statements(
            "CREATE TABLE t (s String) ENGINE = Memory;\n-- only a comment;\nINSERT INTO t VALUES ('a;b');\n",
        );
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[1].ends_with("('a;b')"));
    }
}
