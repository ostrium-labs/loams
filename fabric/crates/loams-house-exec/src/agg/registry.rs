//! Building an aggregate from its name and argument types, as ClickHouse's
//! `AggregateFunctionFactory::get` does (`src/AggregateFunctions/
//! AggregateFunctionFactory.cpp`, Apache-2.0).

use std::sync::Arc;

use loams_house_sql::aggregates::{AggregateName, Combinator, Param};
use loams_house_types::{ChError, ChType};

use super::{AggRef, combinators, core};

/// `43 ILLEGAL_TYPE_OF_ARGUMENT` for `function`'s argument of type `t`.
pub fn illegal_type(t: &ChType, function: &str) -> ChError {
    ChError::illegal_type_of_argument(format!(
        "Illegal type {t} of argument for aggregate function {function}"
    ))
}

/// `42 NUMBER_OF_ARGUMENTS_DOESNT_MATCH`.
pub fn arity(function: &str, wanted: &str, got: usize) -> ChError {
    ChError::number_of_arguments_doesnt_match(format!(
        "Aggregate function {function} requires {wanted}, got {got}"
    ))
}

/// `133 AGGREGATE_FUNCTION_DOESNT_ALLOW_PARAMETERS`.
pub fn no_params(function: &str) -> ChError {
    ChError {
        code: 133,
        name: "AGGREGATE_FUNCTION_DOESNT_ALLOW_PARAMETERS",
        message: format!("Aggregate function {function} cannot have parameters"),
    }
}

fn later(function: &str, task: u32) -> ChError {
    ChError::not_implemented(format!(
        "The aggregate function {function} is not supported by the Loams House engine yet (plan HE1 Task {task})"
    ))
}

/// The aggregate `name` over arguments of `args`.
///
/// # Errors
///
/// ClickHouse's error for arguments or parameters the function refuses, and
/// `48` for a function or combinator the engine does not have.
pub fn build(name: &AggregateName, args: &[ChType]) -> Result<AggRef, ChError> {
    // `LowCardinality` arguments are read as their values
    // (`convertLowCardinalityTypesToNested`); the `Decode` accumulator and the
    // combinators' own decoding read them so.
    let args: Vec<ChType> = args
        .iter()
        .map(|t| match t {
            ChType::LowCardinality(inner) => (**inner).clone(),
            other => other.clone(),
        })
        .collect();
    if args.iter().any(ChType::is_nullable) {
        return combinators::null(name, &args);
    }
    build_nested(name, &args)
}

/// [`build`] once the `Null` combinator has been considered.
pub fn build_nested(name: &AggregateName, args: &[ChType]) -> Result<AggRef, ChError> {
    let Some((outer, rest)) = name.combinators.split_first() else {
        return base(&name.base, &name.params, args);
    };
    let inner = AggregateName {
        base: name.base.clone(),
        combinators: rest.to_vec(),
        params: name.params.clone(),
    };
    let function = name.function();
    match outer {
        Combinator::If => combinators::if_(&inner, args, &function),
        Combinator::Array => combinators::array(&inner, args, &function),
        Combinator::ForEach => combinators::for_each(&inner, args, &function),
        Combinator::Distinct => Ok(combinators::distinct(build(&inner, args)?, args)),
        Combinator::OrNull => combinators::or_fill(build(&inner, args)?, true),
        Combinator::OrDefault => combinators::or_fill(build(&inner, args)?, false),
        Combinator::SimpleState => combinators::simple_state(&inner, args),
        Combinator::Map => combinators::map(&inner, args, &function),
        Combinator::State | Combinator::Merge => Err(later(&function, 29)),
        Combinator::Resample
        | Combinator::ArgMax
        | Combinator::ArgMin
        | Combinator::Tuple
        | Combinator::Null => Err(ChError::unsupported(format!(
            "The aggregate function combinator -{}",
            outer.suffix()
        ))),
    }
}

/// The base function `base` (no combinators left).
fn base(base: &str, params: &[Param], args: &[ChType]) -> Result<AggRef, ChError> {
    let parametric = matches!(
        base,
        "groupArray" | "groupUniqArray" | "topK" | "approx_top_k" | "windowFunnel"
    );
    if !parametric && !params.is_empty() {
        return Err(no_params(base));
    }
    let agg: AggRef = match base {
        "count" => Arc::new(core::Count::new(args)),
        "sum" => Arc::new(core::Sum::new(args)?),
        "avg" => Arc::new(core::Avg::new(args)?),
        "min" | "max" | "any" | "anyLast" => Arc::new(core::Pick::new(base, args)?),
        "argMin" | "argMax" => Arc::new(core::ArgPick::new(base, args)?),
        "groupArray" => super::arrays::group_array(params, args)?,
        "groupUniqArray" => super::arrays::group_uniq_array(params, args)?,
        "topK" | "approx_top_k" => super::topk::top_k(base, params, args)?,
        "sumMappedArrays" | "minMappedArrays" | "maxMappedArrays" => {
            super::maps::mapped_arrays(&base[..3], args)?
        }
        "windowFunnel" => super::funnel::window_funnel(params, args)?,
        b if b.starts_with("uniq")
            || b.starts_with("quantile")
            || b == "median"
            || b.starts_with("median") =>
        {
            return Err(later(b, 29));
        }
        b => {
            return Err(ChError::unsupported(format!("The aggregate function {b}")));
        }
    };
    Ok(agg)
}
