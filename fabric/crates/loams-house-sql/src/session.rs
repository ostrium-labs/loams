//! Functions whose answer is the session's, bound when a statement is
//! planned (plan HE1 Task 33): `currentDatabase()` (`DATABASE()`,
//! `current_database()`) answers the statement's current database.

use std::collections::HashMap;
use std::sync::Arc;

use datafusion::arrow::datatypes::{DataType, Field, FieldRef};
use datafusion::common::{DataFusionError, Result, ScalarValue};
use datafusion::logical_expr::{
    ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    Volatility,
};
use loams_house_types::ChError;
use loams_house_types::arrow::CH_TYPE;

/// `currentDatabase()` for a statement whose database is `database`.
#[derive(Debug, PartialEq, Eq, Hash)]
pub struct CurrentDatabase {
    database: String,
    signature: Signature,
}

impl CurrentDatabase {
    /// The function, answering `database`.
    pub fn udf(database: &str) -> Arc<ScalarUDF> {
        Arc::new(ScalarUDF::new_from_impl(Self {
            database: database.to_string(),
            signature: Signature::nullary(Volatility::Stable),
        }))
    }
}

impl ScalarUDFImpl for CurrentDatabase {
    fn name(&self) -> &str {
        "currentDatabase"
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(DataType::Utf8)
    }

    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        if !args.arg_fields.is_empty() {
            return Err(DataFusionError::External(Box::new(
                ChError::number_of_arguments_doesnt_match(format!(
                    "Number of arguments for function currentDatabase doesn't match: passed {}, should be 0",
                    args.arg_fields.len()
                )),
            )));
        }
        let metadata = HashMap::from([(CH_TYPE.to_string(), "String".to_string())]);
        Ok(Arc::new(
            Field::new("currentDatabase", DataType::Utf8, false).with_metadata(metadata),
        ))
    }

    fn invoke_with_args(&self, _args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        Ok(ColumnarValue::Scalar(ScalarValue::Utf8(Some(
            self.database.clone(),
        ))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_database_is_bound() {
        let f = CurrentDatabase::udf("db1");
        assert_eq!(f.name(), "currentDatabase");
    }
}
