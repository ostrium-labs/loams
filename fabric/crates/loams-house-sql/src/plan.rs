//! `prepare`: a ClickHouse statement's text to a DataFusion logical plan
//! (plan HE1 Task 22): parameters bound, the lexical pre-pass, sqlparser's
//! `ClickHouseDialect`, the guard, name resolution ([`crate::scope`], Task
//! 25), then `SqlToRel` over the House's context.

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
pub fn guard(sql: &str, params: &[(String, String)]) -> Result<Guarded, ChError> {
    let bound = params::bind(sql, params)?;
    let pre = lex::pre_pass(&bound)?;
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
            other
                .to_string()
                .split_whitespace()
                .take(3)
                .collect::<Vec<_>>()
                .join(" ")
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
    let Guarded {
        mut statement,
        resolution,
        settings,
        format,
    } = guard(sql, params)?;
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
