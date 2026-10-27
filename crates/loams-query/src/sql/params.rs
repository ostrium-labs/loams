//! Positional parameters for a read-only statement (`loams.sql.v1`'s
//! `QueryRequest.params`; SDKP P0b).
//!
//! `$1…$n` bind, in order, to JSON scalars. Each value is cast to the type the
//! planner inferred for its placeholder, so `n > $1` on an `i64` column binds
//! `2` as an `Int64` rather than comparing a float. The statement's highest
//! placeholder number must equal the number of values; anything else is
//! [`ParamsError::Count`], which the Connect handler answers as
//! `invalid_argument` with reason `sql_param_count`. Named placeholders and
//! non-scalar values are `invalid_argument`.

use std::collections::HashMap;

use datafusion::arrow::datatypes::DataType;
use datafusion::common::{ParamValues, ScalarValue};
use datafusion::prelude::{DataFrame, SessionContext};
use serde_json::Value;

use super::{planning, refresh_catalogs, verify_read_only};
use crate::error::ServiceError;

/// Why a parameterised statement was refused before it ran.
#[derive(Debug, thiserror::Error)]
pub enum ParamsError {
    /// The statement's placeholders and the values given differ in number.
    #[error(
        "the statement has {expected} parameter(s) and {given} value(s) were given: \
         $1…$n bind to params in order"
    )]
    Count { expected: usize, given: usize },
    #[error(transparent)]
    Service(#[from] ServiceError),
}

/// Plans `sql` read-only in `ctx`, binding `params` to `$1…$n`.
///
/// The same planning [`super::plan_read_only`] does (catalog refresh, the
/// read-only check), plus the binding.
pub async fn plan_read_only_with_params(
    ctx: &SessionContext,
    sql: &str,
    params: &[Value],
) -> Result<DataFrame, ParamsError> {
    refresh_catalogs(ctx).await?;
    let plan = ctx
        .state()
        .create_logical_plan(sql)
        .await
        .map_err(planning)?;
    verify_read_only(&plan)?;
    let types = plan.get_parameter_types().map_err(planning)?;
    let expected = placeholder_count(&types)?;
    if expected != params.len() {
        return Err(ParamsError::Count {
            expected,
            given: params.len(),
        });
    }
    let plan = if expected == 0 {
        plan
    } else {
        let values = params
            .iter()
            .enumerate()
            .map(|(i, value)| {
                bind(
                    i + 1,
                    value,
                    types.get(&format!("${}", i + 1)).cloned().flatten(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        plan.replace_params_with_values(&ParamValues::from(values))
            .map_err(|err| ServiceError::InvalidArgument(format!("sql params: {err}")))?
    };
    Ok(ctx.execute_logical_plan(plan).await.map_err(planning)?)
}

/// The highest `$n` a statement names: `$1` and `$3` need three values.
fn placeholder_count(types: &HashMap<String, Option<DataType>>) -> Result<usize, ServiceError> {
    let mut highest = 0;
    for id in types.keys() {
        let n = id
            .strip_prefix('$')
            .and_then(|digits| digits.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                ServiceError::InvalidArgument(format!(
                    "sql placeholder {id} is not positional: use $1…$n"
                ))
            })?;
        highest = highest.max(n);
    }
    Ok(highest)
}

/// One JSON scalar as the placeholder's type.
fn bind(n: usize, value: &Value, want: Option<DataType>) -> Result<ScalarValue, ServiceError> {
    let scalar = match value {
        Value::Null => {
            return Ok(match want {
                Some(dt) => ScalarValue::try_from(&dt).unwrap_or(ScalarValue::Null),
                None => ScalarValue::Null,
            });
        }
        Value::Bool(b) => ScalarValue::Boolean(Some(*b)),
        Value::Number(number) => {
            if let Some(i) = number.as_i64() {
                ScalarValue::Int64(Some(i))
            } else if let Some(u) = number.as_u64() {
                ScalarValue::UInt64(Some(u))
            } else {
                ScalarValue::Float64(number.as_f64())
            }
        }
        Value::String(s) => ScalarValue::Utf8(Some(s.clone())),
        Value::Array(_) | Value::Object(_) => {
            return Err(ServiceError::InvalidArgument(format!(
                "sql param ${n} is a list or an object; params are scalars"
            )));
        }
    };
    match want {
        Some(dt) if dt != scalar.data_type() => scalar.cast_to(&dt).map_err(|err| {
            ServiceError::InvalidArgument(format!("sql param ${n} is not a {dt}: {err}"))
        }),
        _ => Ok(scalar),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_is_the_highest_placeholder() {
        let types: HashMap<String, Option<DataType>> =
            [("$1".to_string(), None), ("$3".to_string(), None)].into();
        assert_eq!(placeholder_count(&types).unwrap(), 3);
        let named: HashMap<String, Option<DataType>> = [("$foo".to_string(), None)].into();
        assert!(placeholder_count(&named).is_err());
    }

    #[test]
    fn scalars_cast_to_the_inferred_type() {
        assert_eq!(
            bind(1, &serde_json::json!(2), Some(DataType::Float64)).unwrap(),
            ScalarValue::Float64(Some(2.0))
        );
        assert_eq!(
            bind(1, &serde_json::json!("a"), None).unwrap(),
            ScalarValue::Utf8(Some("a".into()))
        );
        assert!(bind(1, &serde_json::json!([1]), None).is_err());
        assert_eq!(
            bind(1, &Value::Null, Some(DataType::Int64)).unwrap(),
            ScalarValue::Int64(None)
        );
    }
}
