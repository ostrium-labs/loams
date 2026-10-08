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
    let translated =
        translate_full(statement).map_err(|err| GraphError::Engine(err.to_string()))?;
    match translated {
        GqlTranslationResult::Plan(plan) => {
            check_operator(&plan.root)?;
            // An EXPLAIN plans without running; a PROFILE runs.
            if plan.root.has_mutations() && (plan.profile || !plan.explain) {
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
    if names_file_access(&all_words(statement)) {
        return Err(GraphError::StatementNotAllowed {
            file_access: true,
            what: "LOAD reads server files".to_string(),
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

/// Walks a plan for operators no caller may run.
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

/// A statement's bare words: uppercased, with strings, quoted identifiers and comments removed.
///
/// Comments are dropped because a comment is not a statement: `/* sync */ INSERT ...` writes, and a
/// guard that read the comment as the first word would call it a read. Strings are dropped for the
/// other reason: a value is data, not syntax.
pub(crate) fn bare_words(statement: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = statement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            quote @ ('\'' | '"' | '`') => {
                push_word(&mut word, &mut words);
                for c in chars.by_ref() {
                    if c == quote {
                        break;
                    }
                }
            }
            '-' | '/' if matches!(chars.peek(), Some('-' | '/')) => {
                push_word(&mut word, &mut words);
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                push_word(&mut word, &mut words);
                chars.next();
                let mut previous = '\0';
                for c in chars.by_ref() {
                    if previous == '*' && c == '/' {
                        break;
                    }
                    previous = c;
                }
            }
            c if c.is_ascii_alphabetic() || c == '_' => word.push(c.to_ascii_uppercase()),
            _ => push_word(&mut word, &mut words),
        }
    }
    push_word(&mut word, &mut words);
    words
}

fn push_word(word: &mut String, words: &mut Vec<String>) {
    if !word.is_empty() {
        words.push(std::mem::take(word));
    }
}
