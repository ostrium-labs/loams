//! An `INSERT`'s head: its table as written and its column list, for the header
//! block a client fills (`INSERT INTO [TABLE] [db.]t [(a, b, …)] [SETTINGS …]`).

/// What an `INSERT` writes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// The table, as written (quotes kept), for `DESCRIBE TABLE`.
    pub(crate) table: String,
    /// The listed columns, unquoted; `None` for every insertable column.
    pub(crate) columns: Option<Vec<String>>,
}

/// The target of `head`, the statement before its `FORMAT` or `VALUES`;
/// `None` for a form this does not read (`INSERT INTO FUNCTION`, a `SELECT`).
pub(crate) fn parse(head: &str) -> Option<Target> {
    let mut p = Parser { text: head, pos: 0 };
    p.keyword("INSERT")?;
    p.keyword("INTO")?;
    let save = p.pos;
    if p.keyword("TABLE").is_none() {
        p.pos = save;
    }
    let save = p.pos;
    if p.keyword("FUNCTION").is_some() {
        return None;
    }
    p.pos = save;
    let start = p.skip_space();
    p.identifier()?;
    if p.peek() == Some('.') {
        p.pos += 1;
        p.identifier()?;
    }
    let table = head[start..p.pos].to_string();
    p.skip_space();
    let columns = if p.peek() == Some('(') {
        p.pos += 1;
        let mut list = Vec::new();
        loop {
            p.skip_space();
            let mut name = p.identifier()?;
            // A nested column, `n.a`.
            while p.peek() == Some('.') {
                p.pos += 1;
                name.push('.');
                name.push_str(&p.identifier()?);
            }
            list.push(name);
            p.skip_space();
            match p.peek() {
                Some(',') => p.pos += 1,
                Some(')') => {
                    p.pos += 1;
                    break;
                }
                _ => return None,
            }
        }
        Some(list)
    } else {
        None
    };
    p.skip_space();
    let rest = &head[p.pos..];
    if !(rest.is_empty() || rest.to_ascii_uppercase().starts_with("SETTINGS")) {
        return None;
    }
    Some(Target { table, columns })
}

struct Parser<'a> {
    text: &'a str,
    pos: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    /// Skips whitespace and comments; answers where the next token starts.
    fn skip_space(&mut self) -> usize {
        loop {
            let rest = &self.text[self.pos..];
            let trimmed = rest.trim_start();
            self.pos += rest.len() - trimmed.len();
            if trimmed.starts_with("--") {
                self.pos += trimmed.find('\n').unwrap_or(trimmed.len());
            } else if let Some(body) = trimmed.strip_prefix("/*") {
                self.pos += 2 + body.find("*/").map_or(body.len(), |end| end + 2);
            } else {
                return self.pos;
            }
        }
    }

    fn keyword(&mut self, word: &str) -> Option<()> {
        self.skip_space();
        let rest = &self.text[self.pos..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        rest[..end].eq_ignore_ascii_case(word).then(|| {
            self.pos += end;
        })
    }

    /// A bare or quoted identifier, unquoted.
    fn identifier(&mut self) -> Option<String> {
        self.skip_space();
        let rest = &self.text[self.pos..];
        let quote = rest.chars().next()?;
        if quote == '`' || quote == '"' {
            let mut out = String::new();
            let mut chars = rest.char_indices().skip(1);
            while let Some((i, c)) = chars.next() {
                if c == '\\' {
                    if let Some((_, escaped)) = chars.next() {
                        out.push(escaped);
                    }
                } else if c == quote {
                    self.pos += i + 1;
                    return Some(out);
                } else {
                    out.push(c);
                }
            }
            return None;
        }
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        self.pos += end;
        Some(rest[..end].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heads_of_the_clients() {
        // golang-migrate (clickhouse-go v1) and clickhouse-go v2's batch.
        assert_eq!(
            parse("INSERT INTO schema_migrations (version, dirty, sequence)"),
            Some(Target {
                table: "schema_migrations".into(),
                columns: Some(vec!["version".into(), "dirty".into(), "sequence".into()]),
            })
        );
        assert_eq!(
            parse("insert into `db`.`my table`"),
            Some(Target {
                table: "`db`.`my table`".into(),
                columns: None,
            })
        );
        assert_eq!(
            parse("INSERT INTO TABLE t (`a b`, n.x) SETTINGS async_insert = 1")
                .and_then(|t| t.columns),
            Some(vec!["a b".to_string(), "n.x".to_string()])
        );
        assert_eq!(parse("INSERT INTO FUNCTION s3('x')"), None);
        assert_eq!(parse("INSERT INTO t SELECT 1"), None);
    }
}
