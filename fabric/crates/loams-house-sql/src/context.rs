//! The House's `ContextProvider` (plan HE1 Task 22, step 5): tables from a
//! [`Catalog`], functions from a [`Registry`], and ClickHouse types through
//! [`ClickHouseTypes`], DataFusion's `TypePlanner` hook.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, FieldRef};
use datafusion::common::config::ConfigOptions;
use datafusion::common::{DataFusionError, Result, TableReference};
use datafusion::logical_expr::planner::{ExprPlanner, TypePlanner};
use datafusion::logical_expr::{AggregateUDF, HigherOrderUDF, ScalarUDF, TableSource, WindowUDF};
use datafusion::sql::planner::ContextProvider;
use datafusion::sql::sqlparser::ast;
use loams_house_types::ChError;

/// The tables a statement can name: the namespace's catalog, pinned.
pub trait Catalog: fmt::Debug + Send + Sync {
    /// The table `database.name`, if it exists.
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>>;
}

/// The functions the engine has, by the names DataFusion looks them up with
/// ([`crate::functions::internal_name`]).
#[derive(Debug, Default, Clone)]
pub struct Registry {
    /// Scalar functions.
    pub scalars: HashMap<String, Arc<ScalarUDF>>,
    /// Aggregate functions.
    pub aggregates: HashMap<String, Arc<AggregateUDF>>,
    /// Window functions.
    pub windows: HashMap<String, Arc<WindowUDF>>,
    /// Higher-order functions (lambdas).
    pub higher_order: HashMap<String, Arc<HigherOrderUDF>>,
    /// Planners `SqlToRel` asks first for operators and special syntax, with
    /// the operands' types in hand: date arithmetic with intervals (plan HE1
    /// Task 30, R30.3).
    pub expr_planners: Vec<Arc<dyn ExprPlanner>>,
}

/// ClickHouse type text in casts (`CAST(x AS LowCardinality(String))`,
/// `CAST(x, 'Enum8(…)')`) to the engine's Arrow layout.
#[derive(Debug, Default)]
pub struct ClickHouseTypes;

/// The ClickHouse type text of a sqlparser data type. sqlparser prints some
/// ClickHouse types in other dialects' spellings (`INT8`, `FLOAT64`,
/// `STRING`), and reads ClickHouse's `Int8` as Postgres's `int8`, so the
/// ClickHouse spellings are written back here.
pub fn type_text(sql_type: &ast::DataType) -> String {
    use ast::DataType as D;
    match sql_type {
        D::Custom(name, args) if args.is_empty() => match name.0.as_slice() {
            [ast::ObjectNamePart::Identifier(i)] => i.value.clone(),
            _ => name.to_string(),
        },
        D::Int8(None) => "Int8".into(),
        D::Int64 => "Int64".into(),
        D::Float64 => "Float64".into(),
        D::String(None) => "String".into(),
        D::Bool | D::Boolean => "Bool".into(),
        D::Date => "Date".into(),
        D::Datetime(None) => "DateTime".into(),
        D::Datetime(Some(p)) => format!("DateTime64({p})"),
        D::Datetime64(p, None) => format!("DateTime64({p})"),
        D::Datetime64(p, Some(tz)) => format!("DateTime64({p}, '{tz}')"),
        D::Uuid => "UUID".into(),
        D::JSON => "JSON".into(),
        D::Nullable(t) => format!("Nullable({})", type_text(t)),
        D::LowCardinality(t) => format!("LowCardinality({})", type_text(t)),
        D::Array(ast::ArrayElemTypeDef::Parenthesis(t)) => format!("Array({})", type_text(t)),
        D::Map(k, v) => format!("Map({}, {})", type_text(k), type_text(v)),
        D::Tuple(fields) => format!(
            "Tuple({})",
            fields
                .iter()
                .map(|f| match &f.field_name {
                    Some(n) => format!("{} {}", n.value, type_text(&f.field_type)),
                    None => type_text(&f.field_type),
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        D::Enum(vals, bits) => {
            let members: Vec<String> = vals
                .iter()
                .map(|m| match m {
                    ast::EnumMember::Name(n) => loams_house_types::ty::quote(n),
                    ast::EnumMember::NamedValue(n, v) => {
                        format!("{} = {v}", loams_house_types::ty::quote(n))
                    }
                })
                .collect();
            format!(
                "Enum{}({})",
                bits.map(|b| b.to_string()).unwrap_or_default(),
                members.join(", ")
            )
        }
        other => other.to_string(),
    }
}

impl TypePlanner for ClickHouseTypes {
    fn plan_type_field(&self, sql_type: &ast::DataType) -> Result<Option<FieldRef>> {
        // The cast's field carries the ClickHouse type, so `CAST(x AS Date)`
        // is a `Date`, not the `Date32` its Arrow layout would read as
        // (plan HE1 Task 24, R24.1).
        let text = type_text(sql_type);
        let t = loams_house_types::parse(&text).map_err(external)?;
        let field = loams_house_types::arrow::to_arrow("", &t).map_err(external)?;
        Ok(Some(Arc::new(field.with_nullable(true))))
    }
}

/// A ClickHouse error carried through DataFusion.
pub fn external(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

/// The provider `SqlToRel` plans against.
#[derive(Debug)]
pub struct HouseContext<'a> {
    catalog: &'a dyn Catalog,
    registry: &'a Registry,
    database: String,
    options: ConfigOptions,
    types: Arc<dyn TypePlanner>,
    planners: Vec<Arc<dyn ExprPlanner>>,
}

impl<'a> HouseContext<'a> {
    /// A provider over `catalog` and `registry`, with `database` as the
    /// statement's current database.
    pub fn new(catalog: &'a dyn Catalog, registry: &'a Registry, database: &str) -> Self {
        Self {
            catalog,
            registry,
            database: database.to_string(),
            options: ConfigOptions::default(),
            types: Arc::new(ClickHouseTypes),
            // The registry's planners first (date arithmetic, plan HE1 Task
            // 30), then the operators' type rules (Task 24).
            planners: registry
                .expr_planners
                .iter()
                .cloned()
                .chain(std::iter::once(
                    Arc::new(crate::coerce::ClickHouseOperators) as Arc<dyn ExprPlanner>,
                ))
                .collect(),
        }
    }
}

impl ContextProvider for HouseContext<'_> {
    fn get_table_source(&self, name: TableReference) -> Result<Arc<dyn TableSource>> {
        let (db, table) = match &name {
            TableReference::Bare { table } => (self.database.as_str(), table.as_ref()),
            TableReference::Partial { schema, table } => (schema.as_ref(), table.as_ref()),
            TableReference::Full { .. } => {
                return Err(external(ChError::syntax_error(format!(
                    "Table name {name} has too many parts"
                ))));
            }
        };
        self.catalog.table(db, table).ok_or_else(|| {
            external(ChError {
                code: 60,
                name: "UNKNOWN_TABLE",
                message: format!("Unknown table expression identifier '{db}.{table}'"),
            })
        })
    }

    fn get_type_planner(&self) -> Option<Arc<dyn TypePlanner>> {
        Some(self.types.clone())
    }

    fn get_expr_planners(&self) -> &[Arc<dyn ExprPlanner>] {
        &self.planners
    }

    fn get_function_meta(&self, name: &str) -> Option<Arc<ScalarUDF>> {
        if name == crate::coerce::COND {
            return Some(crate::coerce::plan_time::cond_udf());
        }
        self.registry.scalars.get(name).cloned()
    }

    fn get_higher_order_meta(&self, name: &str) -> Option<Arc<HigherOrderUDF>> {
        self.registry.higher_order.get(name).cloned()
    }

    fn get_aggregate_meta(&self, name: &str) -> Option<Arc<AggregateUDF>> {
        self.registry.aggregates.get(name).cloned()
    }

    fn get_window_meta(&self, name: &str) -> Option<Arc<WindowUDF>> {
        self.registry.windows.get(name).cloned()
    }

    fn get_variable_type(&self, _variable_names: &[String]) -> Option<DataType> {
        None
    }

    fn get_variable_field(&self, _variable_names: &[String]) -> Option<FieldRef> {
        None
    }

    fn options(&self) -> &ConfigOptions {
        &self.options
    }

    fn udf_names(&self) -> Vec<String> {
        self.registry.scalars.keys().cloned().collect()
    }

    fn higher_order_function_names(&self) -> Vec<String> {
        self.registry.higher_order.keys().cloned().collect()
    }

    fn udaf_names(&self) -> Vec<String> {
        self.registry.aggregates.keys().cloned().collect()
    }

    fn udwf_names(&self) -> Vec<String> {
        self.registry.windows.keys().cloned().collect()
    }
}
