//! Exact ClickHouse operators; DataFusion does not coerce their operands.
//! Numeric rules derive from ClickHouse v26.9.2.8-stable (Apache-2.0), see NOTICE.
mod cast;
pub mod types;
mod value;
use datafusion::{
    arrow::{
        array::ArrayRef,
        datatypes::{DataType, Field, FieldRef},
    },
    common::{DataFusionError, Result, ScalarValue},
    logical_expr::{
        ColumnarValue, ReturnFieldArgs, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
        Volatility,
    },
};
use loams_house_types::{ChError, ChType};
use std::sync::Arc;

/// Operators and small type/conditional functions needed by the analyzer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Op {
    Add,
    Subtract,
    Multiply,
    Divide,
    IntDiv,
    Modulo,
    Negate,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Not,
    IsNull,
    IsNotNull,
    TypeName,
    If,
    MultiIf,
    Coalesce,
    IfNull,
    Array,
}
impl Op {
    /// ClickHouse's canonical function name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "plus",
            Self::Subtract => "minus",
            Self::Multiply => "multiply",
            Self::Divide => "divide",
            Self::IntDiv => "intDiv",
            Self::Modulo => "modulo",
            Self::Negate => "negate",
            Self::Eq => "equals",
            Self::Ne => "notEquals",
            Self::Lt => "less",
            Self::Le => "lessOrEquals",
            Self::Gt => "greater",
            Self::Ge => "greaterOrEquals",
            Self::And => "and",
            Self::Or => "or",
            Self::Not => "not",
            Self::IsNull => "isNull",
            Self::IsNotNull => "isNotNull",
            Self::TypeName => "toTypeName",
            Self::If => "if",
            Self::MultiIf => "multiIf",
            Self::Coalesce => "coalesce",
            Self::IfNull => "ifNull",
            Self::Array => "array",
        }
    }
    /// Lookup by canonical name.
    pub fn named(name: &str) -> Option<Self> {
        ALL.iter().copied().find(|o| o.name() == name)
    }
    /// Conditional calls are lowered to lazy CASE expressions by the analyzer.
    pub fn conditional(self) -> bool {
        matches!(
            self,
            Self::If | Self::MultiIf | Self::Coalesce | Self::IfNull
        )
    }
}
const ALL: &[Op] = &[
    Op::Add,
    Op::Subtract,
    Op::Multiply,
    Op::Divide,
    Op::IntDiv,
    Op::Modulo,
    Op::Negate,
    Op::Eq,
    Op::Ne,
    Op::Lt,
    Op::Le,
    Op::Gt,
    Op::Ge,
    Op::And,
    Op::Or,
    Op::Not,
    Op::IsNull,
    Op::IsNotNull,
    Op::TypeName,
    Op::If,
    Op::MultiIf,
    Op::Coalesce,
    Op::IfNull,
    Op::Array,
];
/// The scalar functions every House session registers.
pub fn functions() -> Vec<Arc<ScalarUDF>> {
    let mut functions = ALL.iter().map(|o| udf(*o)).collect::<Vec<_>>();
    functions.push(cast_udf());
    functions
}
/// A type-aware operator without DataFusion's default numeric coercion.
pub fn udf(op: Op) -> Arc<ScalarUDF> {
    Arc::new(ScalarUDF::from(Kernel {
        op,
        signature: Signature::variadic_any(Volatility::Immutable),
    }))
}
pub(crate) fn external(e: ChError) -> DataFusionError {
    DataFusionError::External(Box::new(e))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Kernel {
    op: Op,
    signature: Signature,
}
impl Kernel {
    fn result(&self, fields: &[FieldRef]) -> Result<ChType> {
        let ts = fields
            .iter()
            .map(|f| loams_house_types::arrow::from_arrow(f))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(external)?;
        let arity = match self.op {
            Op::Negate | Op::Not | Op::IsNull | Op::IsNotNull | Op::TypeName => Some(1),
            Op::If => Some(3),
            Op::MultiIf | Op::Coalesce | Op::Array | Op::And | Op::Or => None,
            _ => Some(2),
        };
        if arity.is_some_and(|n| n != ts.len())
            || matches!(self.op, Op::MultiIf) && (ts.len() < 3 || ts.len() % 2 == 0)
            || matches!(self.op, Op::Coalesce | Op::And | Op::Or) && ts.is_empty()
        {
            return Err(external(ChError::number_of_arguments_doesnt_match(
                self.op.name(),
            )));
        }
        let null = ts.iter().any(types::nullable);
        Ok(match self.op {
            Op::Add
            | Op::Subtract
            | Op::Multiply
            | Op::Divide
            | Op::IntDiv
            | Op::Modulo
            | Op::Negate => types::arithmetic(self.op, &ts).map_err(external)?,
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Not
                if ts.iter().any(|t| matches!(types::base(t), ChType::Nothing)) =>
            {
                ChType::Nullable(Box::new(ChType::Nothing))
            }
            Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::And | Op::Or | Op::Not => {
                types::wrap(ChType::UInt8, null)
            }
            Op::IsNull | Op::IsNotNull => ChType::UInt8,
            Op::TypeName => ChType::String,
            Op::If => types::least(&ts[1..]).map_err(external)?,
            Op::MultiIf => types::least(
                &ts.iter()
                    .enumerate()
                    .filter(|(i, _)| i % 2 == 1 || *i == ts.len() - 1)
                    .map(|(_, t)| t.clone())
                    .collect::<Vec<_>>(),
            )
            .map_err(external)?,
            Op::IfNull | Op::Coalesce => {
                let mut branches = Vec::new();
                let mut all_null = true;
                for t in &ts {
                    if !matches!(types::base(t), ChType::Nothing) {
                        branches.push(types::base(t).clone());
                    }
                    if !types::nullable(t) {
                        all_null = false;
                        break;
                    }
                }
                types::wrap(types::least(&branches).map_err(external)?, all_null)
            }
            Op::Array => ChType::Array(Box::new(types::least(&ts).map_err(external)?)),
        })
    }
}
impl ScalarUDFImpl for Kernel {
    fn name(&self) -> &str {
        self.op.name()
    }
    fn signature(&self) -> &Signature {
        &self.signature
    }
    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        let fields = args
            .iter()
            .map(|t| Arc::new(Field::new("arg", t.clone(), false)))
            .collect::<Vec<_>>();
        Ok(
            loams_house_types::arrow::to_arrow("result", &self.result(&fields)?)
                .map_err(external)?
                .data_type()
                .clone(),
        )
    }
    fn return_field_from_args(&self, args: ReturnFieldArgs) -> Result<FieldRef> {
        let t = self.result(args.arg_fields)?;
        Ok(Arc::new(
            loams_house_types::arrow::to_arrow(self.name(), &t).map_err(external)?,
        ))
    }
    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        if self.op.conditional() {
            return Err(external(ChError::logical_error(
                "Conditional operator reached execution without the House analyzer",
            )));
        }
        if self.op == Op::TypeName {
            let ty = loams_house_types::arrow::from_arrow(&args.arg_fields[0]).map_err(external)?;
            return Ok(ColumnarValue::Scalar(ScalarValue::Utf8(Some(
                ty.to_compact_string(),
            ))));
        }
        let arrays = args
            .args
            .iter()
            .map(|a| a.to_array(args.number_rows))
            .collect::<Result<Vec<ArrayRef>>>()?;
        let types = args
            .arg_fields
            .iter()
            .map(|f| loams_house_types::arrow::from_arrow(f))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(external)?;
        let result = loams_house_types::arrow::from_arrow(&args.return_field).map_err(external)?;
        if args.number_rows == 0 {
            return Ok(ColumnarValue::Array(
                datafusion::arrow::array::new_empty_array(args.return_type()),
            ));
        }
        let mut values = Vec::with_capacity(args.number_rows);
        for row in 0..args.number_rows {
            let operands = arrays
                .iter()
                .map(|a| ScalarValue::try_from_array(a.as_ref(), row))
                .collect::<Result<Vec<_>>>()?;
            values.push(value::evaluate(self.op, &operands, &types, &result).map_err(external)?);
        }
        Ok(ColumnarValue::Array(ScalarValue::iter_to_array(values)?))
    }
}

/// Internal CAST, emitted after the SQL guard validates its type.
pub fn cast_udf() -> Arc<ScalarUDF> {
    Arc::new(ScalarUDF::from(cast::Cast::new()))
}
