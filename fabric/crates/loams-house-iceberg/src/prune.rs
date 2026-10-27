//! Pruning (plan HE1 Task 26): a scan's filters, which name the table's
//! columns in their ClickHouse types, become filters on the data files' own
//! columns, and from those an Iceberg predicate.
//!
//! The order is the plan's: the Iceberg predicate prunes by **partition**
//! (iceberg's manifest-list summaries and partition projection, every
//! transform), then by **file statistics** (its metrics evaluator over each
//! file's bounds and counts); the file-level filter goes to DataFusion's
//! Parquet source, which prunes row groups by their statistics, then pages by
//! the **page index**, then by **bloom filters**, and evaluates it while
//! decoding (`pushdown_filters`).
//!
//! A filter is translated only where a column reads its stored values with a
//! plain cast (integers, floats, decimals, `Bool`, strings, dates, times,
//! `UUID`, `FixedString`, and `LowCardinality` of those): there the stored
//! column, cast, *is* the table's column, and DataFusion's simplifier moves
//! the cast onto the literal. `Enum`, `IPv4` and `IPv6` (stored as text, whose
//! order is not theirs), `JSON`, states and composites are not translated.
//! Every filter is still applied above the scan (`Inexact`), so pruning only
//! ever reads less.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, SchemaRef};
use datafusion::common::tree_node::{Transformed, TreeNode};
use datafusion::common::{Column, DFSchema, ScalarValue};
use datafusion::logical_expr::expr::InList;
use datafusion::logical_expr::simplify::SimplifyContext;
use datafusion::logical_expr::utils::split_conjunction;
use datafusion::logical_expr::{Between, BinaryExpr, Cast, Expr, Operator};
use datafusion::optimizer::simplify_expressions::ExprSimplifier;
use iceberg_engine::expr::{Predicate, Reference};
use iceberg_engine::spec::{Datum, PrimitiveType, Type};
use loams_house_types::ChType;

/// A table column a filter can name in stored terms.
#[derive(Debug, Clone)]
pub struct Translatable {
    /// The column's type as the scan gives it.
    pub table_type: DataType,
    /// Its Iceberg type in the data files.
    pub iceberg: PrimitiveType,
}

/// Whether a column of `t`, stored as `stored`, reads with a plain cast.
pub fn plain_cast(t: &ChType, stored: &Type) -> Option<PrimitiveType> {
    let Type::Primitive(p) = stored else {
        return None;
    };
    let base = match t.base() {
        ChType::SimpleAggregateFunction(spec) => spec.args[0].base().clone(),
        other => other.clone(),
    };
    let fine = match base {
        ChType::Int8
        | ChType::Int16
        | ChType::Int32
        | ChType::Int64
        | ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Float32
        | ChType::Float64
        | ChType::Decimal { .. }
        | ChType::Bool
        | ChType::String
        | ChType::FixedString(_)
        | ChType::Date
        | ChType::Date32
        | ChType::DateTime(_)
        | ChType::DateTime64(..)
        | ChType::Uuid => true,
        _ => false,
    };
    fine.then(|| p.clone())
}

/// The filters on the data files' columns, and the Iceberg predicate, for a
/// scan's `filters` (each in the table's terms).
#[derive(Debug, Default)]
pub struct Pruning {
    /// The conjuncts that translate, in stored terms, over the file schema.
    pub stored: Option<Expr>,
    /// The Iceberg predicate of those that convert exactly.
    pub predicate: Option<Predicate>,
}

/// Translates `filters` for a file schema `stored_schema`, through the columns
/// in `columns` (by name).
pub fn pruning(
    filters: &[Expr],
    columns: &HashMap<String, Translatable>,
    stored_schema: &SchemaRef,
) -> Pruning {
    let Ok(dfschema) = DFSchema::try_from(stored_schema.as_ref().clone()) else {
        return Pruning::default();
    };
    let dfschema = Arc::new(dfschema);
    let context = SimplifyContext::builder()
        .with_schema(dfschema.clone())
        .build();
    let simplifier = ExprSimplifier::new(context);
    let mut stored = Vec::new();
    let mut predicates = Vec::new();
    for filter in filters {
        for conjunct in split_conjunction(filter) {
            let Some(expr) = to_stored(conjunct, columns) else {
                continue;
            };
            let Ok(expr) = simplifier.simplify(expr) else {
                continue;
            };
            if let Some(p) = to_predicate(&expr, columns) {
                predicates.push(p);
            }
            stored.push(expr);
        }
    }
    Pruning {
        stored: stored.into_iter().reduce(Expr::and),
        predicate: predicates.into_iter().reduce(Predicate::and),
    }
}

/// `expr` with every column `c` as `CAST(stored c AS table type)`, if every
/// column it names translates (and it holds no subquery or placeholder).
fn to_stored(expr: &Expr, columns: &HashMap<String, Translatable>) -> Option<Expr> {
    let mut fine = true;
    let out = expr
        .clone()
        .transform_up(|e| match e {
            Expr::Column(c) => match columns.get(&c.name) {
                Some(t) => Ok(Transformed::yes(Expr::Cast(Cast::new(
                    Box::new(Expr::Column(Column::new_unqualified(c.name))),
                    t.table_type.clone(),
                )))),
                None => {
                    fine = false;
                    Ok(Transformed::no(Expr::Column(c)))
                }
            },
            Expr::ScalarSubquery(_)
            | Expr::Exists(_)
            | Expr::InSubquery(_)
            | Expr::Placeholder(_)
            | Expr::OuterReferenceColumn(..)
            | Expr::ScalarVariable(..) => {
                fine = false;
                Ok(Transformed::no(e))
            }
            other => Ok(Transformed::no(other)),
        })
        .ok()?
        .data;
    fine.then_some(out)
}

/// The Iceberg predicate `expr` (in stored terms) is, when it converts
/// exactly.
fn to_predicate(expr: &Expr, columns: &HashMap<String, Translatable>) -> Option<Predicate> {
    match expr {
        Expr::BinaryExpr(BinaryExpr { left, op, right }) => match op {
            Operator::And => Some(to_predicate(left, columns)?.and(to_predicate(right, columns)?)),
            Operator::Or => Some(to_predicate(left, columns)?.or(to_predicate(right, columns)?)),
            Operator::Eq
            | Operator::NotEq
            | Operator::Lt
            | Operator::LtEq
            | Operator::Gt
            | Operator::GtEq => {
                let (column, value, op) = match (column(left), literal(right)) {
                    (Some(c), Some(v)) => (c, v, *op),
                    _ => (column(right)?, literal(left)?, op.swap()?),
                };
                let t = columns.get(&column)?;
                let datum = datum(value, &t.iceberg)?;
                let r = Reference::new(column);
                Some(match op {
                    Operator::Eq => r.equal_to(datum),
                    Operator::NotEq => r.not_equal_to(datum),
                    Operator::Lt => r.less_than(datum),
                    Operator::LtEq => r.less_than_or_equal_to(datum),
                    Operator::Gt => r.greater_than(datum),
                    _ => r.greater_than_or_equal_to(datum),
                })
            }
            _ => None,
        },
        Expr::Not(inner) => Some(to_predicate(inner, columns)?.negate()),
        Expr::IsNull(inner) => {
            let c = column(inner)?;
            columns.get(&c)?;
            Some(Reference::new(c).is_null())
        }
        Expr::IsNotNull(inner) => {
            let c = column(inner)?;
            columns.get(&c)?;
            Some(Reference::new(c).is_not_null())
        }
        Expr::InList(InList {
            expr,
            list,
            negated,
        }) => {
            let c = column(expr)?;
            let t = columns.get(&c)?;
            let values = list
                .iter()
                .map(|v| datum(literal(v)?, &t.iceberg))
                .collect::<Option<Vec<_>>>()?;
            let r = Reference::new(c);
            Some(if *negated {
                r.is_not_in(values)
            } else {
                r.is_in(values)
            })
        }
        Expr::Between(Between {
            expr,
            negated,
            low,
            high,
        }) => {
            let c = column(expr)?;
            let t = columns.get(&c)?;
            let low = datum(literal(low)?, &t.iceberg)?;
            let high = datum(literal(high)?, &t.iceberg)?;
            let p = Reference::new(c.clone())
                .greater_than_or_equal_to(low)
                .and(Reference::new(c).less_than_or_equal_to(high));
            Some(if *negated { p.negate() } else { p })
        }
        _ => None,
    }
}

fn column(e: &Expr) -> Option<String> {
    match e {
        Expr::Column(c) => Some(c.name.clone()),
        _ => None,
    }
}

fn literal(e: &Expr) -> Option<&ScalarValue> {
    match e {
        Expr::Literal(v, _) if !v.is_null() => Some(v),
        _ => None,
    }
}

/// An Arrow scalar as a datum of the stored Iceberg type `t`, when it is one
/// exactly.
fn datum(value: &ScalarValue, t: &PrimitiveType) -> Option<Datum> {
    let int = || -> Option<i64> {
        match value {
            ScalarValue::Int8(Some(v)) => Some(i64::from(*v)),
            ScalarValue::Int16(Some(v)) => Some(i64::from(*v)),
            ScalarValue::Int32(Some(v)) => Some(i64::from(*v)),
            ScalarValue::Int64(Some(v)) => Some(*v),
            ScalarValue::UInt8(Some(v)) => Some(i64::from(*v)),
            ScalarValue::UInt16(Some(v)) => Some(i64::from(*v)),
            ScalarValue::UInt32(Some(v)) => Some(i64::from(*v)),
            ScalarValue::UInt64(Some(v)) => i64::try_from(*v).ok(),
            _ => None,
        }
    };
    Some(match t {
        PrimitiveType::Int => Datum::int(i32::try_from(int()?).ok()?),
        PrimitiveType::Long => Datum::long(int()?),
        PrimitiveType::Float => match value {
            ScalarValue::Float32(Some(v)) if !v.is_nan() => Datum::float(*v),
            _ => return None,
        },
        PrimitiveType::Double => match value {
            ScalarValue::Float64(Some(v)) if !v.is_nan() => Datum::double(*v),
            _ => return None,
        },
        PrimitiveType::Decimal { precision, scale } => match value {
            ScalarValue::Decimal128(Some(v), _, s) if i64::from(*s) == i64::from(*scale) => {
                Datum::decimal_from_str(decimal_text(*v, *scale))
                    .ok()?
                    .to(&Type::Primitive(PrimitiveType::Decimal {
                        precision: *precision,
                        scale: *scale,
                    }))
                    .ok()?
            }
            _ => return None,
        },
        PrimitiveType::Boolean => match value {
            ScalarValue::Boolean(Some(v)) => Datum::bool(*v),
            _ => return None,
        },
        PrimitiveType::String => match value {
            ScalarValue::Utf8(Some(v))
            | ScalarValue::LargeUtf8(Some(v))
            | ScalarValue::Utf8View(Some(v)) => Datum::string(v),
            _ => return None,
        },
        PrimitiveType::Date => match value {
            ScalarValue::Date32(Some(v)) => Datum::date(*v),
            _ => return None,
        },
        PrimitiveType::Timestamptz | PrimitiveType::Timestamp => {
            let micros = match value {
                ScalarValue::TimestampMicrosecond(Some(v), _) => *v,
                _ => return None,
            };
            if matches!(t, PrimitiveType::Timestamptz) {
                Datum::timestamptz_micros(micros)
            } else {
                Datum::timestamp_micros(micros)
            }
        }
        PrimitiveType::TimestamptzNs | PrimitiveType::TimestampNs => {
            let nanos = match value {
                ScalarValue::TimestampNanosecond(Some(v), _) => *v,
                _ => return None,
            };
            if matches!(t, PrimitiveType::TimestamptzNs) {
                Datum::timestamptz_nanos(nanos)
            } else {
                Datum::timestamp_nanos(nanos)
            }
        }
        PrimitiveType::Uuid => match value {
            ScalarValue::FixedSizeBinary(16, Some(v)) => {
                Datum::uuid_from_str(uuid_text(v)?).ok()?
            }
            _ => return None,
        },
        PrimitiveType::Fixed(n) => match value {
            ScalarValue::FixedSizeBinary(w, Some(v)) if i64::from(*w) == *n as i64 => {
                Datum::fixed(v.iter().copied())
            }
            _ => return None,
        },
        _ => return None,
    })
}

fn decimal_text(mantissa: i128, scale: u32) -> String {
    let digits = mantissa.unsigned_abs().to_string();
    let scale = scale as usize;
    let sign = if mantissa < 0 { "-" } else { "" };
    if scale == 0 {
        return format!("{sign}{digits}");
    }
    let padded = format!("{digits:0>width$}", width = scale + 1);
    let (int, frac) = padded.split_at(padded.len() - scale);
    format!("{sign}{int}.{frac}")
}

fn uuid_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 16 {
        return None;
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    ))
}

#[cfg(test)]
mod tests {
    use datafusion::arrow::datatypes::{Field, Schema};
    use datafusion::logical_expr::{col, lit};

    use super::*;

    #[test]
    fn filters_translate_through_plain_casts() {
        let stored = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Decimal128(20, 0), true),
            Field::new("p", DataType::Int32, true),
            Field::new("e", DataType::Utf8, true),
        ]));
        let mut columns = HashMap::new();
        columns.insert(
            "k".to_string(),
            Translatable {
                table_type: DataType::UInt64,
                iceberg: PrimitiveType::Decimal {
                    precision: 20,
                    scale: 0,
                },
            },
        );
        columns.insert(
            "p".to_string(),
            Translatable {
                table_type: DataType::UInt8,
                iceberg: PrimitiveType::Int,
            },
        );
        let filters = vec![
            col("p").eq(lit(3u8)).and(col("k").gt(lit(10u64))),
            // `e` is an Enum: not translated, still applied above the scan.
            col("e").eq(lit(1i8)),
        ];
        let pruning = pruning(&filters, &columns, &stored);
        let predicate = pruning.predicate.expect("predicate").to_string();
        assert!(predicate.contains("p = 3"), "{predicate}");
        assert!(predicate.contains("k > 10"), "{predicate}");
        let stored = pruning.stored.expect("stored").to_string();
        assert!(!stored.contains("e ="), "{stored}");
        assert_eq!(decimal_text(-5, 2), "-0.05");
        assert_eq!(decimal_text(12345, 2), "123.45");
    }
}
