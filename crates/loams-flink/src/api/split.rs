//! Splitting Flink SQL text into statements at top-level `;`.
//!
//! Quotes (`'…'`, `` `…` ``, `"…"`), line comments (`-- …`) and block
//! comments (`/* … */`) are skipped, so a `;` inside them does not end a
//! statement. `EXECUTE STATEMENT SET BEGIN … END;` holds `;`s between its
//! `BEGIN` and `END`, so the splitter tracks that block too.

/// The complete statements in `text` (trimmed, without their `;`, empty
/// ones dropped) and the unfinished rest.
pub fn split_statements(text: &str) -> (Vec<String>, String) {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut in_set_block = false;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'\'' | b'`' | b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b {
                        // A doubled quote is an escaped quote.
                        if i + 1 < bytes.len() && bytes[i + 1] == b {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b';' => {
                let stmt = text[start..i].trim();
                if in_set_block {
                    // Inside BEGIN … END the `;` ends an INSERT, not the
                    // statement set; `END;` ends the set.
                    if ends_with_word(stmt, "END") {
                        in_set_block = false;
                        push(&mut out, stmt);
                        start = i + 1;
                    }
                } else if starts_statement_set(stmt) && !ends_with_word(stmt, "END") {
                    in_set_block = true;
                } else {
                    push(&mut out, stmt);
                    start = i + 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    let rest = if start < text.len() {
        text[start..].to_string()
    } else {
        String::new()
    };
    (out, rest)
}

fn push(out: &mut Vec<String>, stmt: &str) {
    if !strip_leading_comments(stmt).is_empty() {
        out.push(stmt.to_string());
    }
}

/// The text with leading comments removed (what a statement starts with).
pub fn strip_leading_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        if let Some(rest) = s.strip_prefix("--") {
            s = rest.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(rest) = s.strip_prefix("/*") {
            s = rest.split_once("*/").map_or("", |(_, r)| r);
        } else {
            return s;
        }
    }
}

fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    strip_leading_comments(s)
        .split_whitespace()
        .map(|w| w.to_ascii_uppercase())
}

fn starts_statement_set(stmt: &str) -> bool {
    let w: Vec<String> = words(stmt).take(3).collect();
    (w.len() == 3 && w[0] == "EXECUTE" && w[1] == "STATEMENT" && w[2] == "SET")
        || (w.first().is_some_and(|f| f == "BEGIN") && w.get(1).is_some_and(|s| s == "STATEMENT"))
}

fn ends_with_word(stmt: &str, word: &str) -> bool {
    stmt.trim_end()
        .rsplit(|c: char| c.is_whitespace())
        .next()
        .is_some_and(|last| last.eq_ignore_ascii_case(word))
}

/// The first keyword of a statement, uppercased (`INSERT`, `SELECT`, …).
pub fn first_keyword(stmt: &str) -> Option<String> {
    words(stmt).next().map(|w| {
        w.trim_matches(|c: char| !c.is_ascii_alphanumeric())
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_at_top_level_semicolons() {
        let (s, rest) =
            split_statements("SELECT 1; SELECT ';' ; -- a; b\nSELECT `x;y` FROM t; SEL");
        assert_eq!(
            s,
            vec!["SELECT 1", "SELECT ';'", "-- a; b\nSELECT `x;y` FROM t"]
        );
        assert_eq!(rest, " SEL");
    }

    #[test]
    fn doubled_quotes_and_block_comments() {
        let (s, rest) = split_statements("SELECT 'it''s;'; /* ; */ SELECT 2;");
        assert_eq!(s, vec!["SELECT 'it''s;'", "/* ; */ SELECT 2"]);
        assert_eq!(rest, "");
    }

    #[test]
    fn comment_only_pieces_are_dropped() {
        let (s, _) = split_statements("-- nothing\n; ;");
        assert!(s.is_empty(), "{s:?}");
    }

    #[test]
    fn statement_sets_hold_their_semicolons() {
        let sql = "EXECUTE STATEMENT SET BEGIN INSERT INTO a SELECT 1; INSERT INTO b SELECT 2; END; SELECT 3;";
        let (s, rest) = split_statements(sql);
        assert_eq!(
            s,
            vec![
                "EXECUTE STATEMENT SET BEGIN INSERT INTO a SELECT 1; INSERT INTO b SELECT 2; END",
                "SELECT 3"
            ]
        );
        assert_eq!(rest, "");
        let (s, rest) = split_statements("EXECUTE STATEMENT SET BEGIN INSERT INTO a SELECT 1;");
        assert!(s.is_empty());
        assert!(!rest.is_empty());
    }

    #[test]
    fn first_keywords() {
        assert_eq!(
            first_keyword("  -- c\n insert into t").as_deref(),
            Some("INSERT")
        );
        assert_eq!(first_keyword("/* x */SELECT 1").as_deref(), Some("SELECT"));
        assert_eq!(first_keyword("   ").as_deref(), None);
    }
}
