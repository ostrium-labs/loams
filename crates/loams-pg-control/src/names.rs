//! Resource names and PgDog's routed database names (design §46 §8.1; the
//! plan's shared contract).
//!
//! ```text
//! name := project [ "__" branch [ ( "." database ) | "__ro" ] ]
//! ```
//!
//! `project` and `branch` are names, `[a-z0-9][a-z0-9-]{0,62}` (so never
//! `__`), and `database` is a Postgres identifier: 1 to 63 bytes, no NUL.
//! [`parse_routed`] and [`format_routed`] are the only implementations of
//! the grammar, and they round-trip.
//!
//! The grammar is unambiguous because a name holds neither `_` nor `.`:
//! the project ends at the first `__`, the branch at the next `.` or `__`,
//! and everything after the `.` is the database, dots and `__` included.

use std::fmt;

/// The longest name, and the longest database name, in bytes
/// (Postgres's `NAMEDATALEN - 1`).
pub const MAX_NAME_LEN: usize = 63;

/// The suffix of a read-only routed name.
pub const RO_SUFFIX: &str = "__ro";

/// A name or routed name that the grammar rejects.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {what} {text:?}: {why}")]
pub struct NameError {
    pub what: &'static str,
    pub text: String,
    pub why: &'static str,
}

fn err(what: &'static str, text: &str, why: &'static str) -> NameError {
    NameError {
        what,
        text: text.to_string(),
        why,
    }
}

/// Checks a project or branch name: `[a-z0-9][a-z0-9-]{0,62}`.
pub fn validate_name(name: &str) -> Result<(), NameError> {
    let bad = |why| err("name", name, why);
    if name.is_empty() {
        return Err(bad("empty"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(bad("longer than 63 bytes"));
    }
    if name.contains("__") {
        return Err(bad("contains '__', the routed-name separator"));
    }
    let first = name.as_bytes()[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(bad("must start with a-z or 0-9"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(bad("may hold only a-z, 0-9 and '-'"));
    }
    Ok(())
}

/// Checks a database name: a Postgres identifier of 1 to 63 bytes with no NUL.
pub fn validate_database(name: &str) -> Result<(), NameError> {
    let bad = |why| err("database name", name, why);
    if name.is_empty() {
        return Err(bad("empty"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(bad("longer than 63 bytes"));
    }
    if name.contains('\0') {
        return Err(bad("contains NUL"));
    }
    Ok(())
}

/// What a client's `dbname` routes to (§46 §8.1's table).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Routed {
    /// `<project>`: the default branch's default database, read-write.
    Project { project: String },
    /// `<project>__<branch>`: the branch's default database, read-write.
    Branch { project: String, branch: String },
    /// `<project>__<branch>.<database>`: that database, read-write.
    Database {
        project: String,
        branch: String,
        database: String,
    },
    /// `<project>__<branch>__ro`: the branch's read-only endpoints.
    ReadOnly { project: String, branch: String },
}

impl Routed {
    pub fn project(&self) -> &str {
        match self {
            Routed::Project { project }
            | Routed::Branch { project, .. }
            | Routed::Database { project, .. }
            | Routed::ReadOnly { project, .. } => project,
        }
    }

    pub fn branch(&self) -> Option<&str> {
        match self {
            Routed::Project { .. } => None,
            Routed::Branch { branch, .. }
            | Routed::Database { branch, .. }
            | Routed::ReadOnly { branch, .. } => Some(branch),
        }
    }

    pub fn database(&self) -> Option<&str> {
        match self {
            Routed::Database { database, .. } => Some(database),
            _ => None,
        }
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self, Routed::ReadOnly { .. })
    }
}

/// Parses a client `dbname` by the routed-name grammar.
pub fn parse_routed(s: &str) -> Result<Routed, NameError> {
    let bad = |why| err("routed name", s, why);
    let Some((project, rest)) = s.split_once("__") else {
        validate_name(s).map_err(|_| bad("the project is not a valid name"))?;
        return Ok(Routed::Project {
            project: s.to_string(),
        });
    };
    validate_name(project).map_err(|_| bad("the project is not a valid name"))?;
    // The branch ends at the first '.' or '_' (a name holds neither).
    let end = rest.find(['.', '_']).unwrap_or(rest.len());
    let (branch, tail) = rest.split_at(end);
    validate_name(branch).map_err(|_| bad("the branch is not a valid name"))?;
    let (project, branch) = (project.to_string(), branch.to_string());
    if tail.is_empty() {
        Ok(Routed::Branch { project, branch })
    } else if tail == RO_SUFFIX {
        Ok(Routed::ReadOnly { project, branch })
    } else if let Some(database) = tail.strip_prefix('.') {
        validate_database(database).map_err(|_| bad("the database is not a valid name"))?;
        Ok(Routed::Database {
            project,
            branch,
            database: database.to_string(),
        })
    } else {
        Err(bad(
            "after the branch, expected nothing, '.<database>' or '__ro'",
        ))
    }
}

/// Writes a routed name, checking each part. `parse_routed` inverts it.
pub fn format_routed(r: &Routed) -> Result<String, NameError> {
    validate_name(r.project())?;
    if let Some(b) = r.branch() {
        validate_name(b)?;
    }
    if let Some(d) = r.database() {
        validate_database(d)?;
    }
    Ok(r.to_string())
}

/// The routed name's text, unchecked; [`format_routed`] checks it.
impl fmt::Display for Routed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Routed::Project { project } => f.write_str(project),
            Routed::Branch { project, branch } => write!(f, "{project}__{branch}"),
            Routed::Database {
                project,
                branch,
                database,
            } => write!(f, "{project}__{branch}.{database}"),
            Routed::ReadOnly { project, branch } => write!(f, "{project}__{branch}{RO_SUFFIX}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> String {
        x.to_string()
    }

    #[test]
    fn rejects_double_underscore_in_names() {
        for bad in ["a__b", "__a", "a__", "my__proj", "a___b"] {
            let e = validate_name(bad).unwrap_err();
            assert!(e.why.contains("__"), "{bad}: {e}");
        }
        // A routed project or branch cannot smuggle one in either.
        assert!(format_routed(&Routed::Project { project: s("a__b") }).is_err());
        assert!(
            format_routed(&Routed::Branch {
                project: s("a"),
                branch: s("b__ro")
            })
            .is_err()
        );
    }

    #[test]
    fn validate_name_cases() {
        for ok in ["a", "0", "main", "my-proj", "a-", "x".repeat(63).as_str()] {
            assert_eq!(validate_name(ok), Ok(()), "{ok}");
        }
        for bad in ["", "-a", "A", "a_b", "a.b", "a b", "é", &"x".repeat(64)] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn ro_suffix_parses() {
        assert_eq!(
            parse_routed("shop__main__ro").unwrap(),
            Routed::ReadOnly {
                project: s("shop"),
                branch: s("main")
            }
        );
        // A branch may be called "ro"; only a suffix after a branch is `__ro`.
        assert_eq!(
            parse_routed("shop__ro").unwrap(),
            Routed::Branch {
                project: s("shop"),
                branch: s("ro")
            }
        );
        assert_eq!(
            parse_routed("shop__ro__ro").unwrap(),
            Routed::ReadOnly {
                project: s("shop"),
                branch: s("ro")
            }
        );
        for bad in [
            "shop__main__rw",
            "shop__main__ro__ro",
            "shop__main__",
            "shop__main_ro",
        ] {
            assert!(parse_routed(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn database_with_dot_parses() {
        assert_eq!(
            parse_routed("shop__dev.my.db").unwrap(),
            Routed::Database {
                project: s("shop"),
                branch: s("dev"),
                database: s("my.db")
            }
        );
        // Everything after the first '.' is the database, `__ro` included.
        assert_eq!(
            parse_routed("shop__dev.x__ro").unwrap().database(),
            Some("x__ro")
        );
        assert_eq!(
            parse_routed("shop__dev.Weird Name_1").unwrap().database(),
            Some("Weird Name_1")
        );
        assert!(parse_routed("shop__dev.").is_err());
        assert!(parse_routed(&format!("shop__dev.{}", "d".repeat(64))).is_err());
        assert!(parse_routed("shop__dev.a\0b").is_err());
    }

    #[test]
    fn project_and_branch_forms_parse() {
        assert_eq!(
            parse_routed("shop").unwrap(),
            Routed::Project { project: s("shop") }
        );
        assert_eq!(
            parse_routed("shop__main").unwrap(),
            Routed::Branch {
                project: s("shop"),
                branch: s("main")
            }
        );
        for bad in [
            "",
            "Shop",
            "shop.db",
            "__main",
            "shop__",
            "shop__.db",
            "shop___main",
        ] {
            assert!(parse_routed(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn format_routed_writes_each_form() {
        let cases = [
            (Routed::Project { project: s("p") }, "p"),
            (
                Routed::Branch {
                    project: s("p"),
                    branch: s("b"),
                },
                "p__b",
            ),
            (
                Routed::Database {
                    project: s("p"),
                    branch: s("b"),
                    database: s("d.e"),
                },
                "p__b.d.e",
            ),
            (
                Routed::ReadOnly {
                    project: s("p"),
                    branch: s("b"),
                },
                "p__b__ro",
            ),
        ];
        for (r, want) in cases {
            assert_eq!(format_routed(&r).unwrap(), want);
            assert_eq!(parse_routed(want).unwrap(), r);
        }
        assert!(
            format_routed(&Routed::Database {
                project: s("p"),
                branch: s("b"),
                database: s("")
            })
            .is_err()
        );
    }
}
