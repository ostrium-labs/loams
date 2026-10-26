//! What a statement is (HS1 Task 4; FL2 Task 4 with FL2 Rulings 4 and 6).
//!
//! Two halves decide it:
//!
//! * **Loams's half** — the statements Loams owns, because it must act on them
//!   rather than pass them to chDB: DDL (which later tasks map onto Iceberg tables,
//!   pipes and materialized views), `INSERT`, `SET`, `USE`, `KILL QUERY`, `UNDROP`,
//!   `RENAME`, `OPTIMIZE`. These are parsed with sqlparser's `ClickHouseDialect`,
//!   or read by keyword for the ClickHouse forms sqlparser does not have
//!   (`EXISTS`, `UNDROP`, `KILL QUERY … WHERE query_id = …`).
//! * **ClickHouse's half** — everything else. Queries (`SELECT`, `WITH`, `(`) need
//!   no parsing and go to chDB unchanged. A statement sqlparser cannot parse is
//!   [`Classified::Unparsed`]; the caller asks a worker for ClickHouse's own class
//!   (`chdb_classify_query_n`, FL2 Ruling 6) and [`decide_unparsed`] answers:
//!   read-only, or not ClickHouse either, runs unchanged (chDB then gives its own
//!   `62`); anything that would change state is `62` with sqlparser's message and
//!   the hint that the form is outside the surface (FL2 Ruling 4).
//!
//! The text chDB runs is the input minus a trailing `FORMAT <f>` — never rewritten
//! otherwise (`queries_are_never_rewritten`), except that the front replaces the
//! calls of host functions with the House's values before a worker sees the text
//! (HS1 Task 5, `deny::rewrite_host_functions`).
//!
//! The deny list's statements (`SYSTEM`, `ATTACH`, `BACKUP`, `RESTORE`, `CREATE
//! FUNCTION | DICTIONARY | NAMED COLLECTION`) and engines are `344` here, before any
//! worker is asked (HS1 Task 5); the rest of the deny list runs over the worker's
//! explain of the statement (`deny::check`).
//!
//! Two checks run before either half (fix round 1): no statement may read or write
//! a host file ([`check_text`]: `INTO OUTFILE`, `FROM INFILE`, `344`), and every
//! statement the front sends to chDB as text — queries included — is gated by
//! ClickHouse's own class ([`decide`]).

use loams_house_ipc::{Classification, QueryClass};
use sqlparser::ast::{ObjectType, Statement};
use sqlparser::dialect::ClickHouseDialect;
use sqlparser::parser::Parser;

use crate::deny;
use crate::errors::{ChError, HouseError};
use crate::request::{self, Token};

/// A statement, as the House treats it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stmt {
    /// A query: chDB runs `text` unchanged.
    Query { text: String },
    /// An `INSERT`: the head before its data, and the table when sqlparser could
    /// read it (HS1 Task 12 maps it onto a lake table).
    Insert(InsertStmt),
    /// `CREATE DATABASE`.
    CreateDatabase { name: String },
    /// `CREATE TABLE` (a lake table, HS1 Task 10).
    CreateTable {
        name: String,
        engine: Option<String>,
    },
    /// `CREATE TABLE … AS SELECT` (HS1 Task 12).
    CreateTableAs {
        name: String,
        engine: Option<String>,
    },
    /// `CREATE TEMPORARY TABLE`: session-local, in chDB, on a pinned worker.
    CreateTemporaryTable { name: String },
    /// `CREATE TABLE … ENGINE = LoamsStream | IggyTopic | S3Queue` (HS1 Tasks 16, 18).
    CreatePipe { name: String, engine: String },
    /// `CREATE MATERIALIZED VIEW` (HS1 Task 17).
    CreateMaterializedView { name: String },
    /// `CREATE [OR REPLACE] VIEW` (HE1 R9.13): a catalog view.
    CreateView { name: String },
    /// `CREATE [OR REPLACE] FUNCTION` (HE1 R14.11): a SQL function of the
    /// namespace.
    CreateFunction { name: String },
    /// `CREATE [OR REPLACE] DICTIONARY` (HE1 R14.12): a catalog dictionary.
    CreateDictionary { name: String },
    /// `DROP …`.
    Drop(DropStmt),
    /// `TRUNCATE TABLE`.
    Truncate { name: String },
    /// `ALTER TABLE …`, read by the House's own reader (HE1 R9.12): the table
    /// and each operation's leading words.
    /// A mutation of a lake table (HE1 R9.5): `ALTER TABLE … DELETE|UPDATE|CLEAR
    /// COLUMN|DROP PARTITION`, `DELETE FROM`, `UPDATE … SET`.
    Mutation { table: String },
    Alter {
        table: String,
        operations: Vec<String>,
    },
    /// `RENAME TABLE a TO b, …`.
    Rename { pairs: Vec<(String, String)> },
    /// `EXCHANGE TABLES a AND b` (HE1 R14.9).
    Exchange { first: String, second: String },
    /// `UNDROP TABLE`.
    Undrop { name: String },
    /// `SHOW …`: chDB answers until HS1 Task 11's system tables.
    Show { text: String },
    /// `DESCRIBE …`.
    Describe { text: String },
    /// `EXISTS …`.
    Exists { text: String },
    /// `USE db`.
    Use(String),
    /// `SET a = 1, b = 'x'`.
    Set(Vec<(String, String)>),
    /// `OPTIMIZE TABLE` (HS1 Task 13).
    Optimize { name: String },
    /// `KILL QUERY WHERE query_id = '…' [SYNC|ASYNC|TEST]` (HS1 Task 21).
    KillQuery(KillQuery),
    /// `EXPLAIN …`: chDB answers.
    Explain { text: String },
    /// Parsed, but not on the surface (access DDL, `SYSTEM`, views, other
    /// `ALTER`s): `48`.
    Unsupported { kind: String },
}

/// `KILL QUERY WHERE query_id = '…'` (HS1 Task 21, §49 §12): the one condition
/// the House serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillQuery {
    /// The statement to kill.
    pub query_id: String,
    /// `ASYNC` (the default), `SYNC` or `TEST`.
    pub mode: KillMode,
}

/// How `KILL QUERY` waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillMode {
    /// Answers once the kill is sent (`waiting`).
    Async,
    /// Answers once the statement's worker is gone (`finished`).
    Sync,
    /// Kills nothing; answers what it would kill.
    Test,
}

/// An `INSERT`'s head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertStmt {
    /// The statement before `FORMAT` and its data.
    pub text: String,
    /// The data's format, when the statement names one.
    pub format: Option<String>,
    /// The target table, when sqlparser read it (`None` for table functions it does
    /// not know).
    pub table: Option<String>,
}

/// A `DROP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropStmt {
    /// `TABLE`, `DATABASE`, `VIEW`, `DICTIONARY`, …
    pub kind: String,
    /// The names dropped.
    pub names: Vec<String>,
    /// `DROP TEMPORARY TABLE`: chDB's, on the session's worker.
    pub temporary: bool,
}

/// Which half decided a statement (FL2 Ruling 6: the surface page records it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decided {
    /// sqlparser or a keyword rule.
    Loams,
    /// `chdb_classify_query_n`.
    ClickHouse,
}

/// A classified statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classified {
    /// Decided here.
    Known {
        /// What it is.
        stmt: Stmt,
        /// A trailing `FORMAT <f>` (queries) or the `INSERT`'s format.
        format: Option<String>,
    },
    /// sqlparser could not parse it: ask a worker, then [`decide_unparsed`].
    Unparsed {
        /// The text to classify (and run, if it is let through), minus `FORMAT`.
        text: String,
        /// A trailing `FORMAT <f>`.
        format: Option<String>,
        /// sqlparser's message, for the `62`.
        message: String,
    },
}

/// The hint every refused unparseable statement carries (FL2 Ruling 4).
pub const OUTSIDE_SURFACE: &str = "this statement form is outside the House's ClickHouse surface";

impl Stmt {
    /// The variant's name: the corpus's `-- kind:` line.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Query { .. } => "Query",
            Self::Insert(_) => "Insert",
            Self::CreateDatabase { .. } => "CreateDatabase",
            Self::CreateTable { .. } => "CreateTable",
            Self::CreateTableAs { .. } => "CreateTableAs",
            Self::CreateTemporaryTable { .. } => "CreateTemporaryTable",
            Self::CreatePipe { .. } => "CreatePipe",
            Self::CreateMaterializedView { .. } => "CreateMaterializedView",
            Self::CreateView { .. } => "CreateView",
            Self::CreateFunction { .. } => "CreateFunction",
            Self::CreateDictionary { .. } => "CreateDictionary",
            Self::Drop(_) => "Drop",
            Self::Truncate { .. } => "Truncate",
            Self::Mutation { .. } => "Mutation",
            Self::Alter { .. } => "Alter",
            Self::Rename { .. } => "Rename",
            Self::Exchange { .. } => "Exchange",
            Self::Undrop { .. } => "Undrop",
            Self::Show { .. } => "Show",
            Self::Describe { .. } => "Describe",
            Self::Exists { .. } => "Exists",
            Self::Use(_) => "Use",
            Self::Set(_) => "Set",
            Self::Optimize { .. } => "Optimize",
            Self::KillQuery(_) => "KillQuery",
            Self::Explain { .. } => "Explain",
            Self::Unsupported { .. } => "Unsupported",
        }
    }

    /// Whether it only reads (what GET and read-only users may run). `USE` is
    /// allowed too: it changes only the session's current database.
    pub fn is_read(&self) -> bool {
        matches!(
            self,
            Self::Query { .. }
                | Self::Show { .. }
                | Self::Describe { .. }
                | Self::Exists { .. }
                | Self::Explain { .. }
                | Self::Use(_)
        )
    }
}

/// `164` for a statement that is not a read on a read-only path (GET, or a
/// read-only user), with ClickHouse's own text.
pub fn readonly_error() -> HouseError {
    HouseError::from(ChError::readonly(
        "Cannot execute query in readonly mode. For queries over HTTP, method GET implies \
         readonly. You should use method POST for modifying queries",
    ))
}

/// What ClickHouse's lexer makes of a statement, as far as [`check_text`] needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lexeme {
    /// A bare word (`[A-Za-z0-9_$]+`), as `start..end`.
    Word(usize, usize),
    /// `;`.
    Semicolon,
    /// A literal, a quoted name, or a heredoc.
    Other,
    /// Any other ASCII byte outside those (`(`, `,`, `=`, …).
    Symbol(u8),
}

/// The lexemes of a complete statement, the way ClickHouse's lexer cuts it — not
/// the request scanner's approximation, which does not know heredocs, nested
/// comments or `#` comments. Measured on chDB (fix round 1): `$name$ … $name$`
/// (a name of word characters) is a string, so `SELECT $$'$$ INTO OUTFILE 'x' --'`
/// writes a file while the scanner sees only a string; `/* /* */ … */` nests; `#`
/// starts a comment only before a space or `!`. Bytes outside ASCII are skipped:
/// ClickHouse reads Unicode spaces as whitespace and anything else there fails to
/// parse, so skipping them can only join words, never hide one.
fn lexemes(text: &str) -> Vec<Lexeme> {
    lex(text).into_iter().map(|(lexeme, _)| lexeme).collect()
}

/// [`lexemes`] with the byte range each one covers (HS1 Task 5's rewrite of the
/// host functions needs to know where a call ends).
pub(crate) fn lex(text: &str) -> Vec<(Lexeme, std::ops::Range<usize>)> {
    let b = text.as_bytes();
    let n = b.len();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let line_end = |from: usize| {
        b[from..]
            .iter()
            .position(|c| *c == b'\n')
            .map_or(n, |p| from + p)
    };
    let mut out = Vec::new();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < n {
        let c = b[i];
        let start = i;
        match c {
            // ClickHouse's whitespace includes \v (0x0B), which
            // `is_ascii_whitespace` leaves out: `INTO\vOUTFILE` is one clause.
            _ if c.is_ascii_whitespace() || c == 0x0B || c >= 0x80 => i += 1,
            b'-' if b.get(i + 1) == Some(&b'-') => i = line_end(i),
            b'#' if matches!(b.get(i + 1), Some(b' ' | b'!')) => i = line_end(i),
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 1;
                i += 2;
                while i < n && depth > 0 {
                    if b[i..].starts_with(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if b[i..].starts_with(b"*/") {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            b'\'' | b'"' | b'`' => {
                i += 1;
                while i < n {
                    if b[i] == b'\\' {
                        i += 2;
                    } else if b[i] == c {
                        i += 1;
                        if b.get(i) != Some(&c) {
                            break;
                        }
                        i += 1;
                    } else {
                        i += 1;
                    }
                }
                out.push(Lexeme::Other);
            }
            b'$' => {
                let mut j = i + 1;
                while j < n && word(b[j]) {
                    j += 1;
                }
                let heredoc = (j < n && b[j] == b'$')
                    .then(|| {
                        let delimiter = &b[i..=j];
                        b[j + 1..]
                            .windows(delimiter.len())
                            .position(|w| w == delimiter)
                            .map(|at| j + 1 + at + delimiter.len())
                    })
                    .flatten();
                if let Some(end) = heredoc {
                    i = end;
                    out.push(Lexeme::Other);
                } else if b.get(i + 1).is_some_and(|c| word(*c)) {
                    let start = i;
                    i += 1;
                    while i < n && (word(b[i]) || b[i] == b'$') {
                        i += 1;
                    }
                    out.push(Lexeme::Word(start, i));
                } else {
                    i += 1;
                    out.push(Lexeme::Other);
                }
            }
            _ if word(c) => {
                let start = i;
                while i < n && (word(b[i]) || b[i] == b'$') {
                    i += 1;
                }
                out.push(Lexeme::Word(start, i));
            }
            b';' => {
                i += 1;
                out.push(Lexeme::Semicolon);
            }
            _ => {
                i += 1;
                out.push(Lexeme::Symbol(c));
            }
        }
        if out.len() > spans.len() {
            spans.push(start..i.min(n));
        }
    }
    out.into_iter().zip(spans).collect()
}

/// The statement without what follows its last lexeme: `;`s, whitespace and
/// comments, found with ClickHouse's lexing (HS1 Task 5 fix round 1). The worker
/// ends the text it explains with `FORMAT TabSeparated`, which a trailing `;`
/// would make a second statement; the text that runs is the same one. An
/// `INSERT` is left whole (what follows its head may be data), and so is a
/// trailer with an unclosed comment, which ClickHouse refuses.
pub fn without_trailer(sql: &str) -> &str {
    if route_words(sql).0 == "INSERT" {
        return sql;
    }
    let lexemes = lex(sql);
    let Some((_, last)) = lexemes
        .iter()
        .rev()
        .find(|(lexeme, _)| *lexeme != Lexeme::Semicolon)
    else {
        return sql;
    };
    let end = last.end.min(sql.len());
    let closed = lex(&format!("{}\nX", &sql[end..]))
        .iter()
        .any(|(lexeme, _)| matches!(lexeme, Lexeme::Word(..)));
    if closed { &sql[..end] } else { sql }
}

/// The first two words of a statement and whether it opens with `(`, as
/// ClickHouse's lexer reads it ([`lexemes`]): what the keyword routes go by, so a
/// heredoc or a nested comment cannot show the route one statement and chDB
/// another (the classifier oracle's finding, fix round 1).
fn route_words(text: &str) -> (String, String, bool) {
    let lexemes = lexemes(text);
    let mut words = lexemes.iter().filter_map(|l| match l {
        Lexeme::Word(start, end) => Some(text[*start..*end].to_ascii_uppercase()),
        _ => None,
    });
    let first = words.next().unwrap_or_default();
    let second = words.next().unwrap_or_default();
    (
        first,
        second,
        lexemes.first() == Some(&Lexeme::Symbol(b'(')),
    )
}

/// Refuses what no statement may carry, whatever its kind and wherever it goes
/// (fix round 1): `INTO OUTFILE` and `FROM INFILE`, which chDB's client layer
/// carries out on the host whatever `readonly` and the grants say (HS1 R1.10), are
/// `344`. The words count anywhere ClickHouse's lexer sees them ([`lexemes`]), so
/// an object named `outfile` or `infile` must be quoted with backticks.
///
/// `text` is a statement head: an `INSERT`'s data must not be passed.
pub fn check_text(text: &str) -> Result<(), HouseError> {
    let lexemes = lexemes(text);
    let is = |l: &Lexeme, word: &str| match l {
        Lexeme::Word(start, end) => {
            text.as_bytes()[*start..*end].eq_ignore_ascii_case(word.as_bytes())
        }
        _ => false,
    };
    for pair in lexemes.windows(2) {
        if pair[0] == Lexeme::Semicolon && pair[1] != Lexeme::Semicolon {
            return Err(multi_statement());
        }
        let Some((first, second)) = deny::DENIED_CLAUSES
            .iter()
            .find(|(first, second)| is(&pair[0], first) && is(&pair[1], second))
        else {
            continue;
        };
        let clause = if *first == "INTO" {
            format!(
                "{first} {second} (it writes a file on the server; read the result from the response)"
            )
        } else {
            format!(
                "{first} {second} (it reads a file on the server; send the data in the request body)"
            )
        };
        return Err(HouseError::from(ChError::support_is_disabled(format!(
            "{clause} is disabled on the House. A table or column named like the clause's \
             keyword can be quoted with backticks"
        ))));
    }
    Ok(())
}

/// What the front expects of a statement it sends to chDB as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// A read: a query, `SHOW`, `DESCRIBE`, `EXISTS`, `EXPLAIN`, or a form
    /// sqlparser could not parse (which runs only if ClickHouse calls it a read).
    Read,
    /// One of the owned writes chDB carries out itself, with the classes ClickHouse
    /// gives its kind (measured, fix round 1; `chdb.h`'s `chdb_query_class`): an
    /// `INSERT` is `Mutating`, or `Control` into a table function; a `CREATE` or
    /// `DROP TEMPORARY TABLE` (session-local) is `Control`. Never
    /// `MutatingGlobal`, and never more than one statement (`PARALLEL WITH`'s arms
    /// count).
    Write(&'static [QueryClass]),
}

/// An `INSERT`'s classes.
const INSERT_CLASSES: &[QueryClass] = &[QueryClass::Mutating, QueryClass::Control];
/// A temporary table's `CREATE` or `DROP`.
const TEMPORARY_CLASSES: &[QueryClass] = &[QueryClass::Control];

impl Stmt {
    /// What the front expects ClickHouse to make of this statement when chDB runs
    /// it as text; `None` for what never reaches chDB.
    pub fn expect(&self) -> Option<Expect> {
        match self {
            // The front answers `USE` itself (ClickHouse classes it `Control`).
            Self::Use(_) => None,
            _ if self.is_read() => Some(Expect::Read),
            Self::Insert(_) => Some(Expect::Write(INSERT_CLASSES)),
            Self::CreateTemporaryTable { .. }
            | Self::Drop(DropStmt {
                temporary: true, ..
            }) => Some(Expect::Write(TEMPORARY_CLASSES)),
            _ => None,
        }
    }
}

/// What ClickHouse's own class of a statement (`chdb_classify_query_n`, run by the
/// statement's worker on its control connection) means for the text the front is
/// about to send to chDB (fix round 1, C1, I1, I2). Every statement that runs as
/// text is decided here, on exactly that text.
///
/// * More than one statement is `62` (I1).
/// * A statement ClickHouse cannot parse (`Unknown`) is `62` with the hint that the
///   form is outside the surface, on every path (I2): chDB is never asked to run
///   what its own parser rejects.
/// * On a read-only path (`readonly`: GET, or a read-only user), anything but
///   `ReadOnly` is `164`.
/// * A read must be `ReadOnly`, and an owned write of the class its kind has;
///   anything else is `62` with the hint (FL2 Ruling 4). `unparsed` carries sqlparser's message for
///   a form it could not parse.
pub fn decide(
    classification: Classification,
    expect: Expect,
    readonly: bool,
    unparsed: Option<&str>,
) -> Result<(), HouseError> {
    let outside = |what: String| {
        Err(HouseError::from(ChError::syntax_error(format!(
            "{what}: {OUTSIDE_SURFACE}"
        ))))
    };
    if classification.statements > 1 {
        return Err(multi_statement());
    }
    let class = classification.class;
    if class == QueryClass::Unknown {
        return outside(
            unparsed
                .unwrap_or("ClickHouse's parser cannot parse this statement")
                .to_string(),
        );
    }
    let wanted: &[QueryClass] = match expect {
        Expect::Read => &[QueryClass::ReadOnly],
        Expect::Write(classes) => classes,
    };
    if readonly && class != QueryClass::ReadOnly {
        return Err(readonly_error());
    }
    if wanted.contains(&class) {
        return Ok(());
    }
    match unparsed {
        Some(message) => outside(message.to_string()),
        None => outside(format!(
            "ClickHouse classes this statement as {class:?}, not as {}",
            match expect {
                Expect::Read => "a read",
                Expect::Write(_) => "the write it opens with",
            }
        )),
    }
}

/// `62` for more than one statement in a request.
fn multi_statement() -> HouseError {
    HouseError::from(ChError::syntax_error(
        "Multi-statements are not allowed: send one statement per request",
    ))
}

/// The [`deny::DENIED_STATEMENTS`] a `CREATE` opens (`CREATE [OR REPLACE]
/// FUNCTION | DICTIONARY | NAMED COLLECTION`), read with ClickHouse's lexing.
fn denied_create(text: &str) -> Option<&'static str> {
    let mut words = lexemes(text).into_iter().filter_map(|l| match l {
        Lexeme::Word(start, end) => Some(text[start..end].to_ascii_uppercase()),
        _ => None,
    });
    let _create = words.next();
    let mut next = words.next()?;
    if next == "OR" {
        let _replace = words.next();
        next = words.next()?;
    }
    match next.as_str() {
        // HE1 R14.11, R14.12: `CREATE FUNCTION` and `CREATE DICTIONARY` are
        // the catalog's ([`classify`]).
        "NAMED" => Some("CREATE NAMED COLLECTION"),
        _ => None,
    }
}

/// Whether a statement creates something that outlives the session — `CREATE
/// [OR REPLACE] <not TEMPORARY> …` — rather than a temporary table, read with
/// ClickHouse's lexing (HS1 Task 5: the deny list holds a temporary table to
/// Memory or Null).
pub fn creates_shared_object(text: &str) -> bool {
    let mut words = lexemes(text).into_iter().filter_map(|l| match l {
        Lexeme::Word(start, end) => Some(text[start..end].to_ascii_uppercase()),
        _ => None,
    });
    if words.next().as_deref() != Some("CREATE") {
        return false;
    }
    let mut next = words.next();
    if next.as_deref() == Some("OR") {
        let _replace = words.next();
        next = words.next();
    }
    next.is_some_and(|word| word != "TEMPORARY")
}

/// `KILL QUERY WHERE query_id = '<id>' [SYNC|ASYNC|TEST]`, from its tokens;
/// `None` for any other form (`ON CLUSTER`, another condition).
fn kill_query(text: &str, toks: &[(String, Token)]) -> Option<KillQuery> {
    let rest: Vec<&(String, Token)> = toks.iter().skip(2).collect();
    let [where_, column, eq, literal, tail @ ..] = rest.as_slice() else {
        return None;
    };
    if where_.0 != "WHERE" || column.0 != "QUERY_ID" || eq.0 != "=" || literal.1.word {
        return None;
    }
    let raw = &text[literal.1.start..literal.1.end];
    let inner = raw.strip_prefix('\'')?.strip_suffix('\'')?;
    let query_id = inner
        .replace("\\\\", "\\")
        .replace("\\'", "'")
        .replace("''", "'");
    let mode = match tail {
        [] => KillMode::Async,
        [(word, _)] if word == "ASYNC" => KillMode::Async,
        [(word, _)] if word == "SYNC" => KillMode::Sync,
        [(word, _)] if word == "TEST" => KillMode::Test,
        _ => return None,
    };
    Some(KillQuery { query_id, mode })
}

/// Engines that make a `CREATE TABLE` a pipe.
const PIPE_ENGINES: &[&str] = &["LoamsStream", "IggyTopic", "S3Queue"];

/// Words of a statement, upper-cased, outside strings and comments.
fn words(text: &str) -> Vec<(String, Token)> {
    let bytes = text.as_bytes();
    request::tokens(bytes)
        .into_iter()
        .map(|t| {
            (
                String::from_utf8_lossy(&bytes[t.start..t.end]).to_ascii_uppercase(),
                t,
            )
        })
        .collect()
}

/// The `ENGINE = <name>` of a `CREATE TABLE`, read from its tokens.
fn engine_of(text: &str) -> Option<String> {
    let toks = words(text);
    toks.iter().enumerate().find_map(|(at, (w, t))| {
        if w == "ENGINE" && t.word && t.depth == 0 {
            let mut next = toks.iter().skip(at + 1);
            let mut name = next.next()?;
            if name.0 == "=" {
                name = next.next()?;
            }
            name.1
                .word
                .then(|| text[name.1.start..name.1.end].to_string())
        } else {
            None
        }
    })
}

fn name(object: &sqlparser::ast::ObjectName) -> String {
    object.to_string()
}

/// `SET` values as ClickHouse means them: a quoted string's contents, anything
/// else as written.
fn set_value(expr: &sqlparser::ast::Expr) -> String {
    use sqlparser::ast::{Expr, Value};
    match expr {
        Expr::Value(v) => match &v.value {
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => s.clone(),
            other => other.to_string(),
        },
        other => other.to_string(),
    }
}

/// Classifies one statement (no data after it: an `INSERT … FORMAT f` head only).
pub fn classify(sql: &str) -> Result<Classified, HouseError> {
    // The checks every statement gets, on the statement's head (an `INSERT`'s data,
    // if any came with it, is not SQL).
    if route_words(sql).0 == "INSERT" {
        check_text(&insert_stmt(sql).text)?;
    } else {
        check_text(sql)?;
    }
    let (text, format) = request::split_format(sql);
    let toks = words(&text);
    let (first, second, opens_with_paren) = route_words(&text);
    let (first, second) = (first.as_str(), second.as_str());
    let known = |stmt: Stmt, format: Option<String>| Ok(Classified::Known { stmt, format });

    // Queries need no parsing (FL2 Ruling 4); the keyword routes cover the
    // ClickHouse forms sqlparser does not have.
    match first {
        _ if opens_with_paren => return known(Stmt::Query { text }, format),
        "SELECT" | "WITH" => return known(Stmt::Query { text }, format),
        "EXPLAIN" => return known(Stmt::Explain { text }, format),
        "SHOW" => return known(Stmt::Show { text }, format),
        "DESCRIBE" | "DESC" => return known(Stmt::Describe { text }, format),
        "EXISTS" => return known(Stmt::Exists { text }, format),
        "INSERT" => {
            let insert = insert_stmt(sql);
            let format = insert.format.clone();
            return known(Stmt::Insert(insert), format);
        }
        "UNDROP" => {
            let name = toks
                .iter()
                .filter(|(_, t)| t.word)
                .nth(2)
                .map(|(_, t)| text[t.start..t.end].to_string())
                .unwrap_or_default();
            return known(Stmt::Undrop { name }, None);
        }
        // ClickHouseDialect has no `DROP TEMPORARY TABLE`.
        "DROP" if second == "TEMPORARY" => {
            let names = toks
                .iter()
                .filter(|(w, t)| {
                    t.word
                        && !matches!(w.as_str(), "DROP" | "TEMPORARY" | "TABLE" | "IF" | "EXISTS")
                })
                .map(|(_, t)| text[t.start..t.end].to_string())
                .collect();
            return known(
                Stmt::Drop(DropStmt {
                    kind: "TABLE".to_string(),
                    names,
                    temporary: true,
                }),
                None,
            );
        }
        "KILL" if second == "QUERY" => {
            return match kill_query(&text, &toks) {
                Some(kill) => known(Stmt::KillQuery(kill), format),
                None => known(
                    Stmt::Unsupported {
                        kind: "KILL QUERY with a condition other than `WHERE query_id = '…'`"
                            .to_string(),
                    },
                    None,
                ),
            };
        }
        // The deny list's statements (HS1 Task 5, §49 §13.2): `344`, before any
        // worker is asked.
        "SYSTEM" | "ATTACH" | "BACKUP" | "RESTORE" => {
            return Err(HouseError::from(deny::denied_statement(first)));
        }
        // sqlparser reads no `ENGINE` or `POPULATE` in a materialized view; the
        // House's own reader does (HS1 Task 16).
        // HE1 R9.13: a plain view is the catalog's, read by the House's reader.
        "CREATE"
            if second == "VIEW"
                || (second == "OR"
                    && toks
                        .iter()
                        .filter(|(_, t)| t.word)
                        .nth(3)
                        .is_some_and(|(w, _)| w == "VIEW")) =>
        {
            let view = crate::ddl::parse_create_view(&text)?;
            return known(Stmt::CreateView { name: view.name }, None);
        }
        // HE1 R14.11: a SQL function is the catalog's, read by the House's
        // reader (sqlparser's `CREATE FUNCTION` is another dialect's).
        "CREATE"
            if second == "FUNCTION"
                || (second == "OR"
                    && toks
                        .iter()
                        .filter(|(_, t)| t.word)
                        .nth(3)
                        .is_some_and(|(w, _)| w == "FUNCTION")) =>
        {
            let function = crate::ddl::parse_create_function(&text)?;
            return known(
                Stmt::CreateFunction {
                    name: function.name,
                },
                None,
            );
        }
        // HE1 R14.12: a dictionary is the catalog's, read by the House's
        // reader.
        "CREATE"
            if second == "DICTIONARY"
                || (second == "OR"
                    && toks
                        .iter()
                        .filter(|(_, t)| t.word)
                        .nth(3)
                        .is_some_and(|(w, _)| w == "DICTIONARY")) =>
        {
            let dictionary = crate::ddl::parse_create_dictionary(&text)?;
            return known(
                Stmt::CreateDictionary {
                    name: dictionary.name,
                },
                None,
            );
        }
        "DROP" if second == "FUNCTION" || second == "DICTIONARY" => {
            let drop = crate::ddl::parse_drop(&text)?;
            return known(
                Stmt::Drop(DropStmt {
                    kind: drop.kind,
                    names: drop
                        .names
                        .into_iter()
                        .map(|(db, name)| match db {
                            Some(db) => format!("{db}.{name}"),
                            None => name,
                        })
                        .collect(),
                    temporary: false,
                }),
                None,
            );
        }
        "CREATE" if second == "MATERIALIZED" => {
            let view = crate::ddl::parse_create_mv(&text)?;
            return known(Stmt::CreateMaterializedView { name: view.name }, None);
        }
        "CREATE" => {
            if let Some(what) = denied_create(&text) {
                return Err(HouseError::from(deny::denied_statement(what)));
            }
            // HS1 Task 10: a database is a catalog namespace; its engine is held
            // to the database engine lists before anything else.
            if second == "DATABASE" {
                let database = crate::ddl::parse_create_database(&text)?;
                if let Some(engine) = &database.engine {
                    deny::check_database_engine(engine).map_err(HouseError::from)?;
                }
                return known(
                    Stmt::CreateDatabase {
                        name: database.name,
                    },
                    format,
                );
            }
        }
        // HE1 R14.9: `EXCHANGE TABLES a AND b`, the House's own reader's
        // (sqlparser has no `EXCHANGE`).
        "EXCHANGE" => {
            let (first, second) = crate::ddl::parse_exchange(&text)?;
            let printed = |(db, name): crate::ddl::QualifiedName| match db {
                Some(db) => format!("{db}.{name}"),
                None => name,
            };
            return known(
                Stmt::Exchange {
                    first: printed(first),
                    second: printed(second),
                },
                None,
            );
        }
        // HE1 R9.12: every `ALTER TABLE` is the House's own reader's; sqlparser
        // 0.63 refuses most of ClickHouse's forms.
        "ALTER" if second == "TABLE" => return known(alter_stmt(&text)?, None),
        // Lightweight `DELETE FROM` and `UPDATE … SET` (HE1 R9.5): mutations, read
        // as the `ALTER` they mean.
        "DELETE" if second == "FROM" => return known(mutation_stmt(&text)?, None),
        "UPDATE" => return known(mutation_stmt(&text)?, None),
        "GRANT" | "REVOKE" | "DETACH" | "CHECK" => {
            return known(
                Stmt::Unsupported {
                    kind: format!("{first} …"),
                },
                None,
            );
        }
        _ => {}
    }

    let parsed = Parser::parse_sql(&ClickHouseDialect {}, &text);
    let statements = match parsed {
        Ok(statements) if statements.len() == 1 => statements,
        Ok(statements) if statements.is_empty() => {
            return Err(HouseError::from(ChError::syntax_error("Empty query")));
        }
        Ok(_) => return Err(multi_statement()),
        Err(err) => {
            // HS1 Task 10: DDL of lake tables is the front's, read by its own
            // reader and ClickHouse's (column codecs, which sqlparser 0.63 lacks,
            // R4.1); every other unparsed statement is ClickHouse's to class.
            if let Some(stmt) = lake_ddl(&text, first, second)? {
                return known(stmt, format);
            }
            return Ok(Classified::Unparsed {
                text,
                format,
                message: err.to_string(),
            });
        }
    };
    let stmt = match &statements[0] {
        Statement::Query(_) => Stmt::Query { text: text.clone() },
        Statement::CreateDatabase { db_name, .. } => Stmt::CreateDatabase {
            name: name(db_name),
        },
        Statement::CreateTable(create) => {
            let table = name(&create.name);
            let engine = engine_of(&text);
            // The deny list's engines (HS1 Task 5): a temporary table is Memory or
            // Null; no table names an engine that reaches outside, except a pipe's,
            // which is the front's own.
            if let Some(engine) = &engine
                && (create.temporary
                    || !PIPE_ENGINES.iter().any(|p| p.eq_ignore_ascii_case(engine)))
            {
                deny::check_engine(engine, create.temporary).map_err(HouseError::from)?;
            }
            if create.temporary {
                Stmt::CreateTemporaryTable { name: table }
            } else if let Some(pipe) = engine
                .as_ref()
                .filter(|e| PIPE_ENGINES.iter().any(|p| p.eq_ignore_ascii_case(e)))
            {
                Stmt::CreatePipe {
                    name: table,
                    engine: pipe.clone(),
                }
            } else if create.query.is_some() {
                Stmt::CreateTableAs {
                    name: table,
                    engine,
                }
            } else {
                Stmt::CreateTable {
                    name: table,
                    engine,
                }
            }
        }
        Statement::CreateView(view) if view.materialized => Stmt::CreateMaterializedView {
            name: name(&view.name),
        },
        Statement::CreateView(_) => Stmt::CreateView {
            name: crate::ddl::parse_create_view(&text)?.name,
        },
        Statement::Drop {
            object_type,
            names,
            temporary,
            ..
        } => Stmt::Drop(DropStmt {
            kind: match object_type {
                ObjectType::Table => "TABLE".to_string(),
                ObjectType::View => "VIEW".to_string(),
                other => other.to_string().to_ascii_uppercase(),
            },
            names: names.iter().map(name).collect(),
            temporary: *temporary,
        }),
        Statement::Truncate(truncate) => Stmt::Truncate {
            name: truncate
                .table_names
                .first()
                .map(|t| t.name.to_string())
                .unwrap_or_default(),
        },
        Statement::AlterTable(_) => alter_stmt(&text)?,
        Statement::RenameTable(renames) => Stmt::Rename {
            pairs: renames
                .iter()
                .map(|r| (name(&r.old_name), r.new_name.to_string()))
                .collect(),
        },
        Statement::OptimizeTable { name: table, .. } => Stmt::Optimize { name: name(table) },
        Statement::Use(used) => Stmt::Use(use_name(used)),
        Statement::Set(set) => match set_pairs(set) {
            Some(pairs) => Stmt::Set(pairs),
            None => Stmt::Unsupported {
                kind: "SET (this form)".to_string(),
            },
        },
        Statement::ShowTables { .. }
        | Statement::ShowDatabases { .. }
        | Statement::ShowColumns { .. }
        | Statement::ShowCreate { .. } => Stmt::Show { text: text.clone() },
        Statement::ExplainTable { .. } => Stmt::Describe { text: text.clone() },
        Statement::Explain { .. } => Stmt::Explain { text: text.clone() },
        other => Stmt::Unsupported {
            kind: other
                .to_string()
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" "),
        },
    };
    known(stmt, format)
}

/// A lake table's `CREATE TABLE` or `ALTER TABLE … ADD COLUMN` that sqlparser
/// could not parse, read with the DDL reader instead (HS1 Task 10).
fn lake_ddl(text: &str, first: &str, second: &str) -> Result<Option<Stmt>, HouseError> {
    // `DROP … ON CLUSTER c` (SigNoz; HE1 R9.16): sqlparser reads no cluster.
    if first == "DROP" && second != "TEMPORARY" {
        let Ok(drop) = crate::ddl::parse_drop(text) else {
            return Ok(None);
        };
        return Ok(Some(Stmt::Drop(DropStmt {
            kind: drop.kind,
            names: drop
                .names
                .into_iter()
                .map(|(db, name)| match db {
                    Some(db) => format!("{db}.{name}"),
                    None => name,
                })
                .collect(),
            temporary: false,
        })));
    }
    if first == "CREATE" && (second == "TABLE" || second == "OR") {
        let Ok(create) = crate::ddl::parse_create_table(text) else {
            return Ok(None);
        };
        let engine = create.engine.as_ref().map(|(name, _)| name.clone());
        if let Some(engine) = &engine
            && !PIPE_ENGINES.iter().any(|p| p.eq_ignore_ascii_case(engine))
        {
            deny::check_engine(engine, false).map_err(HouseError::from)?;
        }
        let name = create.name.clone();
        return Ok(Some(
            match engine
                .clone()
                .filter(|e| PIPE_ENGINES.iter().any(|p| p.eq_ignore_ascii_case(e)))
            {
                Some(pipe) => Stmt::CreatePipe { name, engine: pipe },
                None if create.as_select => Stmt::CreateTableAs { name, engine },
                None => Stmt::CreateTable { name, engine },
            },
        ));
    }
    Ok(None)
}

/// A lightweight `DELETE FROM` or `UPDATE … SET`.
fn mutation_stmt(text: &str) -> Result<Stmt, HouseError> {
    match crate::mutate::MutationPlan::parse(text, "")? {
        Some(plan) => Ok(Stmt::Mutation { table: plan.name }),
        None => Err(HouseError::from(ChError::syntax_error(format!(
            "Not a mutation: {}",
            text.chars().take(60).collect::<String>()
        )))),
    }
}

/// `ALTER TABLE`, as [`crate::alter::parse_alter`] reads it.
fn alter_stmt(text: &str) -> Result<Stmt, HouseError> {
    if let Some(plan) = crate::mutate::MutationPlan::parse(text, "")? {
        return Ok(Stmt::Mutation { table: plan.name });
    }
    let alter = crate::alter::parse_alter(text)?;
    let operations = alter
        .ops
        .iter()
        .map(|op| {
            format!("{op:?}")
                .split([' ', '(', '{'])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    Ok(Stmt::Alter {
        table: alter.table.1,
        operations,
    })
}

fn use_name(used: &sqlparser::ast::Use) -> String {
    use sqlparser::ast::Use;
    match used {
        Use::Object(object) | Use::Database(object) | Use::Schema(object) => name(object),
        other => other.to_string().trim_start_matches("USE ").to_string(),
    }
}

fn set_pairs(set: &sqlparser::ast::Set) -> Option<Vec<(String, String)>> {
    use sqlparser::ast::Set;
    match set {
        // ClickHouse's `SET a = 1, b = 2` reaches sqlparser 0.63 as one assignment
        // whose value list continues with the expression `b = 2`: unfold it.
        Set::SingleAssignment {
            variable, values, ..
        } => {
            use sqlparser::ast::{BinaryOperator, Expr};
            let mut values = values.iter();
            let mut pairs = vec![(variable.to_string(), set_value(values.next()?))];
            for more in values {
                match more {
                    Expr::BinaryOp {
                        left,
                        op: BinaryOperator::Eq,
                        right,
                    } => {
                        pairs.push((left.to_string(), set_value(right)));
                    }
                    _ => return None,
                }
            }
            Some(pairs)
        }
        Set::MultipleAssignments { assignments } => Some(
            assignments
                .iter()
                .map(|a| (a.name.to_string(), set_value(&a.value)))
                .collect(),
        ),
        _ => None,
    }
}

/// An `INSERT`'s head: its text, format and (when sqlparser reads it) table.
fn insert_stmt(sql: &str) -> InsertStmt {
    let plan = request::plan(sql.as_bytes(), false);
    let (text, format) = match plan.input {
        Some(spec) => (spec.insert, Some(spec.format)),
        None => (sql.trim_end().trim_end_matches(';').to_string(), None),
    };
    // A head with its data elsewhere (`INSERT INTO t (a)`) is not a whole
    // statement to sqlparser; with a stand-in source it is, and only the table is
    // read from it.
    let table = [text.clone(), format!("{text} SELECT 1")]
        .iter()
        .find_map(
            |candidate| match Parser::parse_sql(&ClickHouseDialect {}, candidate) {
                Ok(statements) => match statements.first() {
                    Some(Statement::Insert(insert)) => Some(insert.table.to_string()),
                    _ => None,
                },
                Err(_) => None,
            },
        );
    InsertStmt {
        text,
        format,
        table,
    }
}

/// What to do with a statement sqlparser could not parse, given ClickHouse's own
/// class for it (FL2 Ruling 6), on a writing path: [`decide`].
pub fn decide_unparsed(text: String, message: &str, class: QueryClass) -> Result<Stmt, HouseError> {
    decide(
        Classification {
            class,
            statements: 1,
        },
        Expect::Read,
        false,
        Some(message),
    )?;
    Ok(Stmt::Query { text })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmt(sql: &str) -> Stmt {
        match classify(sql).expect("classifies") {
            Classified::Known { stmt, .. } => stmt,
            other => panic!("{sql}: {other:?}"),
        }
    }

    #[test]
    fn trailers_are_cut_at_the_last_lexeme() {
        for (sql, cut) in [
            ("SELECT 1; -- done\n;", "SELECT 1"),
            ("SELECT 1 -- c", "SELECT 1"),
            ("SELECT 1 /* a /* b */ c */ ;", "SELECT 1"),
            ("SELECT 1 # c\n", "SELECT 1"),
            ("SELECT 'a;' ;", "SELECT 'a;'"),
            ("SELECT $$ -- $$", "SELECT $$ -- $$"),
            ("SELECT 1; SELECT 2;", "SELECT 1; SELECT 2"),
            ("SELECT 1 /* open", "SELECT 1 /* open"),
            (
                "INSERT INTO t FORMAT CSV\na -- b",
                "INSERT INTO t FORMAT CSV\na -- b",
            ),
            ("", ""),
        ] {
            assert_eq!(without_trailer(sql), cut, "{sql:?}");
        }
    }

    #[test]
    fn shared_and_temporary_creates() {
        assert!(creates_shared_object(
            "CREATE TABLE t (a UInt8) ENGINE = MergeTree"
        ));
        assert!(creates_shared_object(
            "/* c */ create or replace view v AS SELECT 1"
        ));
        assert!(!creates_shared_object("CREATE TEMPORARY TABLE t (a UInt8)"));
        assert!(!creates_shared_object(
            "CREATE OR REPLACE TEMPORARY TABLE t (a UInt8)"
        ));
        assert!(!creates_shared_object("SELECT 1"));
        assert!(!creates_shared_object("CREATE $$x$$ TEMPORARY"));
    }

    #[test]
    fn owned_statements() {
        assert_eq!(
            stmt("SET max_threads = 3, session_timezone = 'UTC'"),
            Stmt::Set(vec![
                ("max_threads".to_string(), "3".to_string()),
                ("session_timezone".to_string(), "UTC".to_string()),
            ])
        );
        assert_eq!(stmt("USE db1"), Stmt::Use("db1".to_string()));
        assert_eq!(
            stmt("KILL QUERY WHERE query_id = 'q-1'"),
            Stmt::KillQuery(KillQuery {
                query_id: "q-1".to_string(),
                mode: KillMode::Async
            })
        );
        assert_eq!(
            stmt("kill query where query_id='it''s' SYNC"),
            Stmt::KillQuery(KillQuery {
                query_id: "it's".to_string(),
                mode: KillMode::Sync
            })
        );
        assert_eq!(
            stmt("KILL QUERY WHERE query_id = 'q' TEST"),
            Stmt::KillQuery(KillQuery {
                query_id: "q".to_string(),
                mode: KillMode::Test
            })
        );
        for other in [
            "KILL QUERY WHERE user = 'x'",
            "KILL QUERY WHERE 1",
            "KILL QUERY ON CLUSTER c WHERE query_id = 'q'",
            "KILL QUERY WHERE query_id = 'q' OR 1",
            "KILL QUERY WHERE query_id = 'q' SYNC TEST",
        ] {
            assert!(
                matches!(stmt(other), Stmt::Unsupported { .. }),
                "{other}: {:?}",
                stmt(other)
            );
        }
        assert!(matches!(
            stmt("CREATE TEMPORARY TABLE t (n UInt8) ENGINE = Memory"),
            Stmt::CreateTemporaryTable { .. }
        ));
        assert_eq!(
            stmt("CREATE TABLE q (a String) ENGINE = LoamsStream('s', 'JSONEachRow')"),
            Stmt::CreatePipe {
                name: "q".to_string(),
                engine: "LoamsStream".to_string()
            }
        );
        assert!(matches!(
            stmt("DROP TEMPORARY TABLE t"),
            Stmt::Drop(DropStmt {
                temporary: true,
                ..
            })
        ));
    }
}
