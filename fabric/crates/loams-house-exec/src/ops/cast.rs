//! A CAST whose result retains ClickHouse metadata, including Nullable and enums.
use super::external;
use datafusion::{
    arrow::{
        array::new_empty_array,
        datatypes::{DataType, FieldRef},
    },
    common::{Result, ScalarValue},
    logical_expr::{
        ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDFImpl, Signature, Volatility,
    },
};
use loams_house_types::ChError;
use std::sync::Arc;
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Cast {
    signature: Signature,
}
impl Cast {
    pub fn new() -> Self {
        Self {
            signature: Signature::any(2, Volatility::Immutable),
        }
    }
}
impl ScalarUDFImpl for Cast {
    fn name(&self) -> &str {
        "__ch_cast"
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, _: &[DataType]) -> Result<DataType> {
        Err(external(ChError::logical_error(
            "CAST needs its constant ClickHouse type",
        )))
    }
    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let Some(Some(ScalarValue::Utf8(Some(text)))) = args.scalar_arguments.get(1) else {
            return Err(external(ChError::bad_arguments(
                "CAST type must be constant text",
            )));
        };
        let ty = loams_house_types::parse(text).map_err(external)?;
        Ok(Arc::new(
            loams_house_types::arrow::to_arrow("cast", &ty).map_err(external)?,
        ))
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let ty = loams_house_types::arrow::from_arrow(&args.return_field).map_err(external)?;
        if args.number_rows == 0 {
            return Ok(ColumnarValue::Array(new_empty_array(args.return_type())));
        }
        let input = args.args[0].to_array(args.number_rows)?;
        let values = (0..args.number_rows)
            .map(|i| {
                let v = ScalarValue::try_from_array(input.as_ref(), i)?;
                super::value::cast(&v, &ty).map_err(external)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ColumnarValue::Array(ScalarValue::iter_to_array(values)?))
    }
}
