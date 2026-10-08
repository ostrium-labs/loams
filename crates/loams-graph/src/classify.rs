//! What a statement may do, decided before the engine runs it (design §48 §11.2; GR1 Task 3;
//! rulings R0.8, R0.10, R0.11).
//!
//! Two classifiers, and a gate that uses both:
//!
//! * [`classify`], the **keyword guard**: Loams's own reading of a statement's bare words. It is
//!   deliberately lopsided — a read refused as a write costs one retry, a write let through as a
//!   read costs correctness — so anything it cannot place is a write.
//! * [`engine_classify`], **Grafeo's own translator** (`translate_full`): a plan with mutations is
//!   a write, one without is a read, a schema command is admin, and a session command is mapped
//!   per command.
//! * [`gate`] refuses what no caller may run (GQL's transaction statements, graph management,
//!   file access, unbounded paths) and answers the larger of the two classifications. The engine
//!   session that then runs the statement has the matching Grafeo role, so a statement both
//!   classifiers got wrong is still refused by the engine.
//!
//! Nothing here changes a byte of a statement (D634): it is read to be classified or refused.

use grafeo_adapters::query::gql::ast::SessionCommand;
use grafeo_engine::query::LogicalOperator;
use grafeo_engine::query::translators::gql::{GqlTranslationResult, translate_full};
use loams_proto::loams::graph::v1::QueryLanguage;

use crate::engine::GraphError;

/// The longest variable-length path a statement may ask for (R0.8 (b)). Task 6's
/// `StatementLimits.max_path_hops` makes it per graph.
pub const MAX_PATH_HOPS: u32 = 10;

/// What a statement needs, least to most. Ordered, so the stricter of two answers is `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Access {
    /// Reads only.
    Read,
    /// Writes data.
    Write,
    /// Changes the schema (types, indexes, constraints, procedures).
    Admin,
}

/// The keyword guard: Loams's own reading of a statement, without the engine.
///
/// `language` is the statement's language; this build serves GQL only, and the guard reads every
/// language's keywords the same conservative way.
#[must_use]
pub fn classify(statement: &str, _language: QueryLanguage) -> Access {
    let words = bare_words(statement);
    if is_ddl(&words) {
        Access::Admin
    } else if writes_words(&words) {
        Access::Write
    } else {
        Access::Read
    }
}

/// Grafeo's own classification of a statement, through its GQL translator.
///
/// # Errors
///
/// * [`GraphError::Engine`] when the statement does not parse (the engine's own message);
/// * [`GraphError::TransactionStatement`] for `START TRANSACTION`, `COMMIT`, `ROLLBACK` and the
///   savepoint statements: the RPC owns the transaction (§48 §7.3);
/// * [`GraphError::StatementNotAllowed`] for graph management (`CREATE`/`DROP`/`USE GRAPH`,
///   `SESSION SET GRAPH`, projections; one Loams graph is one Grafeo database's default graph,
///   R0.10 (b)) and for file access (`LOAD DATA` and the plan operators that load a graph; R0.11);
/// * [`GraphError::UnboundedPath`] for a variable-length pattern with no upper bound or one above
///   [`MAX_PATH_HOPS`] (R0.8 (b)).
pub fn engine_classify(statement: &str) -> Result<Access, GraphError> {
    // On a large stack: the parser recurses per operator-chain link (re-review 2c).
    let translated = on_big_stack(|| translate_full(statement))
        .map_err(|_| GraphError::Engine("the statement could not be parsed".to_string()))?
        .map_err(|err| GraphError::Engine(err.to_string()))?;
    match translated {
        GqlTranslationResult::Plan(plan) => {
            // Rendered once, for both the plan check and the procedure-call test.
            let text = format!("{:?}", plan.root);
            check_plan(&plan.root, &text)?;
            // A procedure can read or write, and nothing in the plan says which, so a call
            // anywhere in it (subqueries included, through the plan's `Debug` rendering) is at
            // least a write (security review I1).
            let calls = text.contains("CallProcedure(");
            // An EXPLAIN plans without running; a PROFILE runs.
            if (plan.root.has_mutations() || calls) && (plan.profile || !plan.explain) {
                Ok(Access::Write)
            } else {
                Ok(Access::Read)
            }
        }
        GqlTranslationResult::SchemaCommand(_) => Ok(Access::Admin),
        GqlTranslationResult::SessionCommand(command) => session_command(&command),
        // `#[non_exhaustive]`: a kind this build does not know is treated as schema-changing,
        // the most privileged answer.
        _ => Ok(Access::Admin),
    }
}

/// Refuses what no caller may run and answers what the statement needs.
///
/// The keyword backstop for file access runs first and on any text, so a statement the
/// translator cannot parse is still refused if it names `LOAD DATA` (R0.11). A statement the
/// translator rejects otherwise takes the guard's answer, and the engine reports its syntax error.
///
/// # Errors
///
/// [`GraphError::EmptyStatement`], or any refusal of [`engine_classify`] except a parse error.
pub fn gate(statement: &str, language: QueryLanguage) -> Result<Access, GraphError> {
    if statement.trim().is_empty() {
        return Err(GraphError::EmptyStatement);
    }
    // Before anything parses it: a chain long enough to overflow the parser's stack is refused.
    let links = nesting_estimate(statement);
    if links > MAX_CHAIN_TOKENS {
        return Err(GraphError::TooComplex {
            links,
            limit: MAX_CHAIN_TOKENS,
        });
    }
    if names_file_access(&all_words(statement)) {
        return Err(GraphError::StatementNotAllowed {
            file_access: true,
            what: "LOAD reads server files".to_string(),
        });
    }
    // Shortest-path searches have no hop bound Loams can check (Grafeo's `ShortestPathOp` has
    // none): refused by keyword and by operator until Task 6 bounds them (security review M2).
    if bare_words(statement)
        .iter()
        .any(|w| matches!(w.as_str(), "SHORTEST" | "SHORTESTPATH" | "ALLSHORTESTPATHS"))
    {
        return Err(GraphError::UnboundedPath {
            max_hops: MAX_PATH_HOPS,
        });
    }
    let guard = classify(statement, language);
    match engine_classify(statement) {
        Ok(engine) => Ok(guard.max(engine)),
        Err(GraphError::Engine(_)) => Ok(guard),
        Err(refused) => Err(refused),
    }
}

/// Session commands: none is served. The transaction statements are the RPC's job; graph and
/// projection management would switch away from the one graph a Loams graph is (R0.10 (b)); and
/// the rest set state on a session that ends with the call, which Grafeo's parameterised path
/// refuses anyway ("Session commands cannot be executed as queries", measured).
fn session_command(command: &SessionCommand) -> Result<Access, GraphError> {
    match command {
        SessionCommand::StartTransaction { .. }
        | SessionCommand::Commit
        | SessionCommand::Rollback
        | SessionCommand::Savepoint(_)
        | SessionCommand::RollbackToSavepoint(_)
        | SessionCommand::ReleaseSavepoint(_) => Err(GraphError::TransactionStatement),
        SessionCommand::UseGraph(_)
        | SessionCommand::SessionSetGraph(_)
        | SessionCommand::CreateGraph { .. }
        | SessionCommand::DropGraph { .. }
        | SessionCommand::CreateProjection { .. }
        | SessionCommand::DropProjection { .. } => Err(GraphError::StatementNotAllowed {
            file_access: false,
            what: "one Loams graph is one engine graph; graph and projection management is \
                   not served"
                .to_string(),
        }),
        _ => Err(GraphError::StatementNotAllowed {
            file_access: false,
            what: "session commands are not served: each call is its own session".to_string(),
        }),
    }
}

/// Checks a whole plan, subqueries included.
///
/// Two passes. [`check_operator`] walks the operator tree. That walk does not see operators
/// *inside expressions* (`EXISTS { … }`, `COUNT { … }`, `VALUE { … }`, pattern comprehensions),
/// and walking every expression of every operator by hand cannot be shown complete against a
/// `#[non_exhaustive]` plan type (security review C1). So the second pass reads the plan's
/// **derived `Debug` rendering**, which by construction prints every field of every operator and
/// expression, nested plans included, and checks every occurrence it finds. It fails closed: a
/// string literal that happens to contain an operator's name only adds a refusal, and cannot hide
/// a real operator.
///
/// `text` is `format!("{root:?}")`. The test `grafeo_debug_format_canary` pins that the rendering
/// still names `ExistsSubquery(`, `CallProcedure(` and `max_hops: None`; a Grafeo bump (D759) that
/// changes it fails that test before it can weaken this check.
fn check_plan(root: &LogicalOperator, text: &str) -> Result<(), GraphError> {
    check_operator(root)?;
    if text.contains("LoadData(") || text.contains("LoadGraph(") {
        return Err(GraphError::StatementNotAllowed {
            file_access: true,
            what: "the statement reads a server file".to_string(),
        });
    }
    for operator in [
        "CreateGraph(",
        "DropGraph(",
        "CopyGraph(",
        "MoveGraph(",
        "AddGraph(",
        "ClearGraph(",
        "CreatePropertyGraph(",
    ] {
        if text.contains(operator) {
            return Err(GraphError::StatementNotAllowed {
                file_access: false,
                what: "graph management is not served".to_string(),
            });
        }
    }
    if text.contains("ShortestPath(") {
        return Err(GraphError::UnboundedPath {
            max_hops: MAX_PATH_HOPS,
        });
    }
    // Every `ExpandOp` prints `max_hops: None` or `max_hops: Some(<n>)`.
    for (at, _) in text.match_indices("max_hops: ") {
        let rest = &text[at + "max_hops: ".len()..];
        let bounded = rest
            .strip_prefix("Some(")
            .and_then(|tail| tail.split(')').next())
            .and_then(|n| n.trim().parse::<u32>().ok())
            .is_some_and(|max| max <= MAX_PATH_HOPS);
        if !bounded {
            return Err(GraphError::UnboundedPath {
                max_hops: MAX_PATH_HOPS,
            });
        }
    }
    Ok(())
}

/// Walks a plan's operator tree for operators no caller may run.
fn check_operator(operator: &LogicalOperator) -> Result<(), GraphError> {
    match operator {
        LogicalOperator::LoadData(_) | LogicalOperator::LoadGraph(_) => {
            return Err(GraphError::StatementNotAllowed {
                file_access: true,
                what: "the statement reads a server file".to_string(),
            });
        }
        LogicalOperator::CreateGraph(_)
        | LogicalOperator::DropGraph(_)
        | LogicalOperator::CopyGraph(_)
        | LogicalOperator::MoveGraph(_)
        | LogicalOperator::AddGraph(_)
        | LogicalOperator::ClearGraph(_)
        | LogicalOperator::CreatePropertyGraph(_) => {
            return Err(GraphError::StatementNotAllowed {
                file_access: false,
                what: "graph management is not served".to_string(),
            });
        }
        LogicalOperator::Expand(expand) => match expand.max_hops {
            None => {
                return Err(GraphError::UnboundedPath {
                    max_hops: MAX_PATH_HOPS,
                });
            }
            Some(max) if max > MAX_PATH_HOPS => {
                return Err(GraphError::UnboundedPath {
                    max_hops: MAX_PATH_HOPS,
                });
            }
            Some(_) => {}
        },
        _ => {}
    }
    for child in operator.children() {
        check_operator(child)?;
    }
    Ok(())
}

/// Every alphabetic word of a statement, uppercased, **including** those inside strings and
/// comments. The file-access backstop reads these rather than [`bare_words`]: a backstop that
/// trusted Loams's idea of where a comment ends (GQL 0.5.43 does not parse `//`, for one) could be
/// walked around, and refusing a statement that merely mentions `load data` in a string costs a
/// caller one rewording.
fn all_words(statement: &str) -> Vec<String> {
    statement
        .split(|c: char| !(c.is_ascii_alphabetic() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
}

/// `LOAD` followed by a data or graph source.
fn names_file_access(words: &[String]) -> bool {
    words.windows(2).any(|pair| {
        pair[0] == "LOAD"
            && matches!(
                pair[1].as_str(),
                "DATA" | "CSV" | "GRAPH" | "JSON" | "JSONL" | "PARQUET" | "FROM"
            )
    })
}

/// Schema DDL by its leading verb and object.
fn is_ddl(words: &[String]) -> bool {
    words.windows(2).any(|pair| {
        matches!(pair[0].as_str(), "CREATE" | "DROP" | "ALTER")
            && matches!(
                pair[1].as_str(),
                "NODE" | "EDGE" | "INDEX" | "CONSTRAINT" | "TYPE" | "PROCEDURE" | "SCHEMA"
            )
    })
}

/// Whether a statement can write, by its bare words: a statement whose first word is one of GQL's
/// read shapes, and that contains no writing word anywhere, is a read; everything else is a write.
fn writes_words(words: &[String]) -> bool {
    /// Leading keywords that cannot write. GQL's read shapes: a query expression with its optional
    /// `MATCH`, `FILTER`/`WHERE`, `RETURN`, `LET`/`FOR` and `ORDER BY`/`SKIP`/`LIMIT` clauses, plus
    /// `EXPLAIN` and `PROFILE`.
    const READS: [&str; 12] = [
        "MATCH", "RETURN", "FILTER", "WHERE", "FOR", "LET", "QUERY", "ORDER", "SKIP", "LIMIT",
        "EXPLAIN", "PROFILE",
    ];
    /// Words that write wherever they appear. `CALL` is here because a procedure can read or
    /// write, and a guard cannot tell which without running it.
    const WRITES: [&str; 12] = [
        "INSERT", "CREATE", "DELETE", "MERGE", "SET", "REMOVE", "DROP", "ALTER", "CALL", "LOAD",
        "UPSERT", "GRANT",
    ];
    let reads = words
        .first()
        .is_some_and(|first| READS.contains(&first.as_str()));
    !reads || words.iter().any(|word| WRITES.contains(&word.as_str()))
}

/// A statement's bare words, read the way Grafeo 0.5.43's GQL lexer reads them
/// (`grafeo-adapters` `query/gql/lexer.rs`, security review I1): uppercased with Unicode case
/// folding (`to_uppercase`, so `ſET` is `SET`), with strings, quoted identifiers and comments
/// removed.
///
/// * A string (`'…'` or `"…"`) runs to its own closing quote, and a backslash escapes the next
///   character, so `'\''` is one string.
/// * A backquoted identifier runs to its closing backquote; a doubled backquote is a literal one.
/// * `/* … */` is a comment. `--` is a line comment only when a space, tab or line break follows
///   and the character before it is not `<` or `-`; a bare `--`, and `<--`, are edges. `//` is not a comment in GQL.
/// * A word starts with a letter or `_` and continues with letters, digits and `_` (Unicode).
///
/// Comments are dropped because a comment is not a statement, strings because a value is data.
/// Neither step changes a byte of the statement.
pub(crate) fn bare_words(statement: &str) -> Vec<String> {
    lex(statement).0
}

/// [`bare_words`], and the number of operator characters outside strings and comments
/// (`+ - * / % ^ < > = | [`), for [`nesting_estimate`].
fn lex(statement: &str) -> (Vec<String>, usize) {
    let mut operators = 0;
    let mut words = Vec::new();
    let mut word = String::new();
    let chars: Vec<char> = statement.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' | '"' => {
                push_word(&mut word, &mut words);
                i += 1;
                while i < chars.len() && chars[i] != c {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            '`' => {
                push_word(&mut word, &mut words);
                i += 1;
                while i < chars.len() {
                    if chars[i] == '`' {
                        if chars.get(i + 1) == Some(&'`') {
                            i += 2;
                            continue;
                        }
                        break;
                    }
                    i += 1;
                }
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                push_word(&mut word, &mut words);
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            // `<--` and `---` are edges, whatever follows (re-review 2a).
            '-' if chars.get(i + 1) == Some(&'-')
                && matches!(chars.get(i + 2), Some(' ' | '\t' | '\n' | '\r'))
                && !matches!(i.checked_sub(1).map(|p| chars[p]), Some('<' | '-')) =>
            {
                push_word(&mut word, &mut words);
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            c if c.is_alphabetic() || c == '_' || (!word.is_empty() && c.is_alphanumeric()) => {
                word.extend(c.to_uppercase());
                i += 1;
            }
            _ => {
                if matches!(
                    c,
                    '+' | '-' | '*' | '/' | '%' | '^' | '<' | '>' | '=' | '|' | '['
                ) {
                    operators += 1;
                }
                push_word(&mut word, &mut words);
                i += 1;
            }
        }
    }
    push_word(&mut word, &mut words);
    (words, operators)
}

/// The most chained operators a statement may hold (re-review 2c). Grafeo's GQL parser recurses
/// once per link of an operator chain (`NOT NOT …`, `a AND b AND …`, `1 + 1 + …`, `x[0][0]…`,
/// `… NEXT … NEXT`), with no limit of its own, and overflowing the stack aborts the whole process.
/// Bracket nesting it caps itself at 128. Measured on 0.5.43 (debug build): about 27 KiB of stack
/// per chain link and 90 KiB per bracket level, so on [`PARSE_STACK_BYTES`] this limit leaves
/// more than a 2x margin; a release build uses about a fifth of that.
pub const MAX_CHAIN_TOKENS: usize = 4000;

/// The stack every parse and every engine call runs on (re-review 2c): see [`MAX_CHAIN_TOKENS`].
/// Reserved virtual memory, touched only as deep as a statement nests.
pub const PARSE_STACK_BYTES: usize = 256 << 20;

/// A conservative count of the operator-chain links in a statement: every operator character
/// and every chaining keyword, outside strings and comments, whether or not they share one
/// expression. It over-counts (an arrow's `-` and `>` count too), never under-counts.
fn nesting_estimate(statement: &str) -> usize {
    let (words, operators) = lex(statement);
    operators
        + words
            .iter()
            .filter(|w| {
                matches!(
                    w.as_str(),
                    "NOT"
                        | "AND"
                        | "OR"
                        | "XOR"
                        | "NEXT"
                        | "UNION"
                        | "EXCEPT"
                        | "INTERSECT"
                        | "OTHERWISE"
                )
            })
            .count()
}

/// Runs `f` on a thread with [`PARSE_STACK_BYTES`] of stack and answers its result, or
/// `Err(panic payload)` when it panicked.
pub(crate) fn on_big_stack<T: Send>(
    f: impl FnOnce() -> T + Send,
) -> Result<T, Box<dyn std::any::Any + Send + 'static>> {
    std::thread::scope(|scope| {
        match std::thread::Builder::new()
            .name("loams-graph-stmt".to_string())
            .stack_size(PARSE_STACK_BYTES)
            .spawn_scoped(scope, f)
        {
            Ok(handle) => handle.join(),
            Err(err) => Err(
                Box::new(format!("could not start a statement thread: {err}"))
                    as Box<dyn std::any::Any + Send>,
            ),
        }
    })
}

fn push_word(word: &mut String, words: &mut Vec<String>) {
    if !word.is_empty() {
        words.push(std::mem::take(word));
    }
}
