//! `prepare`: a ClickHouse statement's text to a DataFusion logical plan
//! (plan HE1 Task 22): parameters bound, the lexical pre-pass, sqlparser's
//! `ClickHouseDialect`, the guard, name resolution ([`crate::scope`], Task
//! 25), then `SqlToRel` over the House's context.
//! [`command`] also reads the metadata statements (plan HE1 Task 38).

use datafusion::common::DataFusionError;
use datafusion::logical_expr::LogicalPlan;
use datafusion::sql::planner::{ParserOptions, SqlToRel};
use datafusion::sql::sqlparser::ast::Statement;
use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::parser::Parser;
use loams_house_types::ChError;

use crate::context::{Catalog, HouseContext, Registry};
use crate::guard::{Guard, Resolution};
use crate::lex::{self, Strictness};
use crate::meta::{self, Meta};
use crate::params;

/// A planned statement.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The logical plan, before the engine's analyzer and optimizer.
    pub plan: LogicalPlan,
    /// What it reads and calls.
    pub resolution: Resolution,
    /// The statement's own `SETTINGS`.
    pub settings: Vec<(String, String)>,
    /// The statement's `FORMAT`, if any.
    pub format: Option<String>,
    /// ClickHouse's output column names, when the plan's differ: ClickHouse
    /// allows duplicate names (`SELECT a, a`), DataFusion does not, so the
    /// plan names duplicates apart and the engine names the answer's columns
    /// with these (plan HE1 Task 25).
    pub names: Option<Vec<String>>,
}

/// A parsed and guarded statement, not yet planned.
#[derive(Debug, Clone)]
pub struct Guarded {
    /// The statement, rewritten by the guard.
    pub statement: Statement,
    /// What it reads and calls.
    pub resolution: Resolution,
    /// The statement's own `SETTINGS`.
    pub settings: Vec<(String, String)>,
    /// The statement's `FORMAT`, if any.
    pub format: Option<String>,
}

/// Binds, pre-passes, parses and guards `sql` (one statement).
///
/// `SHOW TABLES` and `SHOW DATABASES` are guarded as the `SELECT` ClickHouse
/// rewrites them into ([`meta::Meta::show_select`]), over `current` as the
/// current database; `DESCRIBE`, `SHOW CREATE` and `EXISTS` are not queries
/// and are refused here ([`command`] answers them).
pub fn guard(sql: &str, params: &[(String, String)]) -> Result<Guarded, ChError> {
    guard_in(sql, params, "default")
}

/// [`guard`] with `current` as the statement's current database.
pub fn guard_in(sql: &str, params: &[(String, String)], current: &str) -> Result<Guarded, ChError> {
    let bound = params::bind(sql, params)?;
    let pre = lex::pre_pass(&bound)?;
    match meta::parse(&pre.sql)? {
        None => guard_pre(pre),
        Some(m) => match m.show_select(current) {
            Some((select, _)) => guard_show(&select, pre.marks),
            None => Err(ChError::not_implemented(format!(
                "{} is a metadata statement, not a query",
                statement_head(&pre.sql)
            ))),
        },
    }
}

/// The first words of a statement, for messages.
fn statement_head(sql: &str) -> String {
    sql.split_whitespace().take(3).collect::<Vec<_>>().join(" ")
}

/// Guards the `SELECT` a `SHOW` list is rewritten into, with the original
/// statement's `SETTINGS` and `FORMAT`.
fn guard_show(select: &str, outer: lex::Marks) -> Result<Guarded, ChError> {
    let mut pre = lex::pre_pass(select)?;
    pre.marks.settings = outer.settings;
    pre.marks.format = outer.format;
    guard_pre(pre)
}

fn guard_pre(pre: lex::PrePass) -> Result<Guarded, ChError> {
    if pre
        .marks
        .strictness
        .iter()
        .any(|(_, s)| *s != Strictness::All)
    {
        return Err(ChError::not_implemented(
            "ANY / SEMI / ANTI / ASOF join strictness is not supported by the Loams House engine yet (plan HE1 Task 35)",
        ));
    }
    let mut statements = Parser::parse_sql(&ClickHouseDialect {}, &pre.sql)
        .map_err(|e| ChError::syntax_error(e.to_string()))?;
    if statements.len() != 1 {
        return Err(ChError::syntax_error("Multi-statements are not allowed"));
    }
    let statement = statements.pop().expect("one statement");
    let mut guard = Guard::new(&pre.marks);
    match statement {
        Statement::Query(mut q) => {
            let (settings, format) = guard.top_query(&mut q)?;
            Ok(Guarded {
                statement: Statement::Query(q),
                resolution: guard.resolution,
                settings,
                format,
            })
        }
        other => Err(ChError::not_implemented(format!(
            "The statement `{}` is not supported by the Loams House engine",
            statement_head(&other.to_string())
        ))),
    }
}

/// Plans `sql` against `catalog` and `registry`, `database` being the
/// statement's current database.
pub fn prepare(
    sql: &str,
    params: &[(String, String)],
    catalog: &dyn Catalog,
    registry: &Registry,
    database: &str,
) -> Result<Prepared, ChError> {
    plan_guarded(
        guard_in(sql, params, database)?,
        catalog,
        registry,
        database,
    )
}

fn plan_guarded(
    guarded: Guarded,
    catalog: &dyn Catalog,
    registry: &Registry,
    database: &str,
) -> Result<Prepared, ChError> {
    let Guarded {
        mut statement,
        resolution,
        settings,
        format,
    } = guarded;
    // Name resolution (plan HE1 Task 25): aliases, scalar WITH, matchers.
    let names = crate::scope::resolve(&mut statement, catalog, database)?;
    crate::coerce::mark_conditions(&mut statement);
    let ctx = HouseContext::new(catalog, registry, database);
    let options = ParserOptions {
        enable_ident_normalization: false,
        ..ParserOptions::default()
    };
    let plan = SqlToRel::new_with_options(&ctx, options)
        .sql_statement_to_plan(statement)
        .map_err(ch_error)?;
    let plan = crate::untuple::expand(plan, registry).map_err(ch_error)?;
    Ok(Prepared {
        plan,
        resolution,
        settings,
        format,
        names,
    })
}

/// A statement the engine runs (plan HE1 Task 38): a query, or a metadata
/// statement it answers from its catalog.
#[derive(Debug, Clone)]
pub enum Command {
    /// A query: `SELECT`, and `SHOW TABLES` / `SHOW DATABASES` as the
    /// `SELECT` ClickHouse rewrites them into.
    Query(Prepared),
    /// `DESCRIBE`, `SHOW CREATE` or `EXISTS`.
    Meta {
        /// The statement.
        meta: Meta,
        /// The statement's own `SETTINGS`.
        settings: Vec<(String, String)>,
        /// The statement's `FORMAT`, if any.
        format: Option<String>,
    },
}

/// `UNKNOWN_DATABASE`.
pub fn unknown_database(database: &str) -> ChError {
    ChError {
        code: 81,
        name: "UNKNOWN_DATABASE",
        message: format!("Database {database} does not exist"),
    }
}

/// Reads `sql` as a query or a metadata statement and plans the queries, as
/// [`prepare`] does.
///
/// # Errors
///
/// As [`prepare`]; `81` for `SHOW TABLES FROM` a database the catalog does
/// not have.
pub fn command(
    sql: &str,
    params: &[(String, String)],
    catalog: &dyn Catalog,
    registry: &Registry,
    database: &str,
) -> Result<Command, ChError> {
    let bound = params::bind(sql, params)?;
    let pre = lex::pre_pass(&bound)?;
    let Some(m) = meta::parse(&pre.sql)? else {
        return plan_guarded(guard_pre(pre)?, catalog, registry, database).map(Command::Query);
    };
    match m.show_select(database) {
        Some((select, listed)) => {
            if let Some(db) = listed
                && !catalog.has_database(&db)
            {
                return Err(unknown_database(&db));
            }
            let guarded = guard_show(&select, pre.marks)?;
            plan_guarded(guarded, catalog, registry, database).map(Command::Query)
        }
        None => Ok(Command::Meta {
            meta: m,
            settings: pre.marks.settings,
            format: pre.marks.format,
        }),
    }
}

/// A DataFusion error as ClickHouse would answer it.
pub fn ch_error(e: DataFusionError) -> ChError {
    match e {
        DataFusionError::External(inner) => match inner.downcast::<ChError>() {
            Ok(ch) => *ch,
            Err(other) => ChError::logical_error(other.to_string()),
        },
        DataFusionError::Context(_, inner) => ch_error(*inner),
        DataFusionError::Diagnostic(_, inner) => ch_error(*inner),
        DataFusionError::SchemaError(e, _) => ChError::unknown_identifier(e.to_string()),
        DataFusionError::Plan(m) if m.contains("No field named") => ChError::unknown_identifier(m),
        // Misplaced aggregates (plan HE1 Task 28): ClickHouse's `184` for an
        // aggregate inside another, `215` for a column outside the keys.
        DataFusionError::Plan(m) if m.contains("Aggregate function calls cannot be nested") => {
            ChError {
                code: 184,
                name: "ILLEGAL_AGGREGATION",
                message: m,
            }
        }
        DataFusionError::Plan(m)
            if m.contains("must be in GROUP BY or an aggregate function")
                || m.contains("must appear in the GROUP BY clause") =>
        {
            ChError {
                code: 215,
                name: "NOT_AN_AGGREGATE",
                message: m,
            }
        }
        DataFusionError::Plan(m) => {
            ChError::not_implemented(format!("{m} (the Loams House engine cannot plan this yet)"))
        }
        DataFusionError::NotImplemented(m) => ChError::not_implemented(m),
        // Integer `%` by zero (plan HE1 Task 24): ClickHouse's `153`.
        DataFusionError::ArrowError(e, _)
            if matches!(*e, datafusion::arrow::error::ArrowError::DivideByZero) =>
        {
            ChError {
                code: 153,
                name: "ILLEGAL_DIVISION",
                message: "Division by zero".into(),
            }
        }
        other => ChError::logical_error(other.to_string()),
    }
}
