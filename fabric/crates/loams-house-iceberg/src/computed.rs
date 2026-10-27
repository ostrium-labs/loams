//! What a lake table computes at read time (HE1 R9.12, R10.6), as
//! `views.rs`'s `casts` and `computed` do for chDB: a column `ALTER … ADD
//! COLUMN` made reads its `DEFAULT` for rows written before it; `ALIAS`
//! columns are computed; a delete `TTL` hides expired rows (ClickHouse
//! deletes them at its next merge; until then they are read, so the House
//! answers as ClickHouse does after that merge).
//!
//! Each expression is catalog text, which another writer of a shared REST
//! catalog can set. It must parse as exactly one ClickHouse expression, is
//! printed again from its tree (R11.11), and is planned by the engine's own
//! front end (`loams-house-sql`, with its functions and type rules) over the
//! table's columns; a subquery is refused (`497`).
//!
//! The result is a view over [`LakeTable`] that DataFusion inlines, so the
//! query's filters still reach the scan through it.

use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, TimeUnit};
use datafusion::catalog::view::ViewTable;
use datafusion::common::tree_node::TreeNode;
use datafusion::common::{ScalarValue, TableReference};
use datafusion::datasource::TableProvider;
use datafusion::datasource::provider_as_source;
use datafusion::logical_expr::expr::FieldMetadata;
use datafusion::logical_expr::{
    Cast, Expr, LogicalPlan, LogicalPlanBuilder, TableSource, cast, col, lit,
};
use datafusion::sql::sqlparser::dialect::ClickHouseDialect;
use datafusion::sql::sqlparser::parser::Parser;
use datafusion::sql::sqlparser::tokenizer::Token;
use loams_house_sql::Registry;
use loams_house_types::arrow::{CH_TYPE, data_type};
use loams_house_types::{ChError, ChType};

use crate::pin::{ReadMode, TablePin, ttl_rules};
use crate::provider::LakeTable;

/// The name the inner scan has inside the view.
pub const RAW: &str = "__loams_raw";
const DATABASE: &str = "__loams";

/// `497`: catalog text the engine does not run as an expression (R11.11).
fn refusal(pin: &TablePin, why: &str) -> ChError {
    ChError {
        code: 497,
        name: "ACCESS_DENIED",
        message: format!(
            "Table {}.{}: its catalog properties hold an expression the House does not run ({why})",
            pin.db, pin.name
        ),
    }
}

/// One catalog table holding the raw scan, for planning catalog text.
struct Raw(Arc<dyn TableSource>);

impl std::fmt::Debug for Raw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Raw")
    }
}

impl loams_house_sql::Catalog for Raw {
    fn table(&self, database: &str, name: &str) -> Option<Arc<dyn TableSource>> {
        (database == DATABASE && name == RAW).then(|| self.0.clone())
    }
}

/// Plans one catalog expression over `raw`'s columns.
///
/// # Errors
///
/// `497` for text that is not exactly one expression or holds a subquery;
/// the planner's error (often `48`, `46` or `47`) for one the engine cannot
/// plan.
pub fn expression(
    pin: &TablePin,
    text: &str,
    raw: &Arc<dyn TableSource>,
    registry: &Registry,
) -> Result<Expr, ChError> {
    let dialect = ClickHouseDialect {};
    let mut parser = Parser::new(&dialect)
        .try_with_sql(text)
        .map_err(|e| refusal(pin, &e.to_string()))?;
    let parsed = parser
        .parse_expr()
        .map_err(|e| refusal(pin, &e.to_string()))?;
    if parser.peek_token().token != Token::EOF {
        return Err(refusal(pin, &format!("{text:?} is not one expression")));
    }
    let sql = format!("SELECT ({parsed}) AS `__loams_e` FROM `{RAW}`");
    let catalog = Raw(raw.clone());
    let prepared = loams_house_sql::prepare(&sql, &[], &catalog, registry, DATABASE)?;
    let LogicalPlan::Projection(projection) = &prepared.plan else {
        return Err(refusal(
            pin,
            &format!("{text:?} does not plan as an expression"),
        ));
    };
    if !matches!(projection.input.as_ref(), LogicalPlan::TableScan(_)) || projection.expr.len() != 1
    {
        return Err(refusal(
            pin,
            &format!("{text:?} does not plan as an expression"),
        ));
    }
    let expr = projection.expr[0].clone().unalias();
    let subquery = expr
        .exists(|e| {
            Ok(matches!(
                e,
                Expr::ScalarSubquery(_) | Expr::Exists(_) | Expr::InSubquery(_)
            ))
        })
        .unwrap_or(true);
    if subquery {
        return Err(refusal(pin, &format!("{text:?} holds a subquery")));
    }
    Ok(expr)
}

fn metadata(ty: &ChType) -> FieldMetadata {
    FieldMetadata::from(
        [(CH_TYPE.to_string(), ty.to_compact_string())]
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
}

/// The provider of `pin`: [`LakeTable`], under a view when the table
/// computes something at read time.
///
/// # Errors
///
/// What [`LakeTable::open`] or [`expression`] refuses.
pub async fn provider(
    pin: &TablePin,
    store: Arc<dyn datafusion::object_store::ObjectStore>,
    registry: &Registry,
) -> Result<Arc<dyn TableProvider>, ChError> {
    let raw = Arc::new(LakeTable::open(pin, store).await?);
    match view(pin, &raw, registry)? {
        Some(plan) => Ok(Arc::new(ViewTable::new(plan, None))),
        None => Ok(raw),
    }
}

/// The view over `raw` that computes `pin`'s `DEFAULT`s, `ALIAS`es and
/// `TTL`, or `None` when it computes nothing.
///
/// # Errors
///
/// What [`expression`] refuses.
pub fn view(
    pin: &TablePin,
    raw: &Arc<LakeTable>,
    registry: &Registry,
) -> Result<Option<LogicalPlan>, ChError> {
    if pin.external {
        return Ok(None);
    }
    let plain = pin.mode == ReadMode::Plain;
    let source = provider_as_source(raw.clone() as Arc<dyn TableProvider>);
    let schema = raw.schema();
    let types = pin.types()?;
    let mut computes = false;
    let mut exprs = Vec::new();
    for column in &pin.ch.columns {
        let ty = &types[&column.name];
        if column.stored() {
            if schema.field_with_name(&column.name).is_err() {
                continue;
            }
            if raw.optional(&column.name) && !ty.is_nullable() {
                // R10.6: rows written before `ALTER … ADD COLUMN` read the
                // default.
                computes = true;
                let (dt, _) = data_type(ty)?;
                let default = if matches!(column.default_kind.as_str(), "DEFAULT" | "MATERIALIZED")
                    && !column.default_expression.trim().is_empty()
                {
                    cast(
                        expression(pin, &column.default_expression, &source, registry)?,
                        dt.clone(),
                    )
                } else {
                    let array = loams_house_types::default::default_array(ty)?;
                    let value = ScalarValue::try_from_array(&array, 0)
                        .map_err(|e| ChError::logical_error(e.to_string()))?;
                    lit(value)
                };
                exprs.push(
                    datafusion::functions::core::expr_fn::coalesce(vec![
                        col(qualified(&column.name)),
                        default,
                    ])
                    .alias_with_metadata(&column.name, Some(metadata(ty))),
                );
            } else {
                exprs.push(col(qualified(&column.name)).alias(&column.name));
            }
        } else if plain {
            computes = true;
            let (dt, _) = data_type(ty)?;
            let value = expression(pin, &column.default_expression, &source, registry)?;
            exprs.push(cast(value, dt).alias_with_metadata(&column.name, Some(metadata(ty))));
        }
    }
    for field in schema.fields() {
        if crate::virtual_columns::is_virtual(field.name()) {
            exprs.push(col(qualified(field.name())).alias(field.name()));
        }
    }
    let mut filters = Vec::new();
    if plain {
        for rule in ttl_rules(&pin.ch.ttl)? {
            computes = true;
            let utc = DataType::Timestamp(TimeUnit::Nanosecond, Some(Arc::from("+00:00")));
            let when = Expr::Cast(Cast::new(
                Box::new(expression(pin, &rule.expression, &source, registry)?),
                utc.clone(),
            ));
            let now = Expr::Cast(Cast::new(
                Box::new(datafusion::functions::datetime::expr_fn::now()),
                utc,
            ));
            let expired = when.lt_eq(now);
            let expired = match &rule.condition {
                Some(condition) => expired.and(expression(pin, condition, &source, registry)?),
                None => expired,
            };
            filters.push(Expr::Not(Box::new(expired)));
        }
    }
    if !computes {
        return Ok(None);
    }
    let mut builder = LogicalPlanBuilder::scan(RAW, source, None).map_err(plan_error)?;
    if let Some(filter) = filters.into_iter().reduce(Expr::and) {
        builder = builder.filter(filter).map_err(plan_error)?;
    }
    let plan = builder
        .project(exprs)
        .map_err(plan_error)?
        .build()
        .map_err(plan_error)?;
    Ok(Some(plan))
}

fn qualified(name: &str) -> datafusion::common::Column {
    datafusion::common::Column::new(Some(TableReference::bare(RAW)), name)
}

fn plan_error(e: datafusion::error::DataFusionError) -> ChError {
    loams_house_sql::plan::ch_error(e)
}
