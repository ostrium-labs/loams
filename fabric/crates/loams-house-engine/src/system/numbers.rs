//! `numbers`, `numbers_mt`, `zeros` and `zeros_mt`, as table functions and as
//! `system` tables, and `system.one` (plan HE1 Task 38). They read nothing
//! outside the statement, so they are the table functions the engine runs;
//! every other one is refused by the guard (`344` for those that reach
//! outside the namespace).
//!
//! Semantics are ClickHouse's (`TableFunctionNumbers`, `StorageSystemNumbers`;
//! checked against the oracle in `corpus/he1/38/` and against ClickHouse's
//! `03032_numbers_zeros` and `03720_numbers_table_function_with_step_edge_cases`
//! tests): `numbers(n)` is `0 … n-1`; `numbers(offset, length)` is
//! `offset … offset+length-1`; `numbers(offset, length, step)` is every
//! `offset + k·step` with `k·step < length`; values wrap at 2^64;
//! `numbers()` and `system.numbers` never end (a `LIMIT` ends the read).
//! The `_mt` forms are the same rows: the engine reads them in one stream.

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, RecordBatch, UInt8Array, UInt64Array};
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::catalog::streaming::StreamingTable;
use datafusion::common::ScalarValue;
use datafusion::execution::{SendableRecordBatchStream, TaskContext, async_stream};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::streaming::PartitionStream;
use loams_house_types::ChError;

use super::build::{self, Cell, Rows};
use super::shapes;

/// What a numbers source generates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Series {
    /// The first value.
    pub offset: u64,
    /// How many rows; `None` for no end.
    pub count: Option<u64>,
    /// The distance between values.
    pub step: u64,
    /// `zeros`: one `UInt8` column of zeros named `zero`, in place of
    /// `number`.
    pub zeros: bool,
}

impl Series {
    /// `numbers(args…)`'s rows.
    ///
    /// # Errors
    ///
    /// `42` for more than three arguments; `36` for a zero step.
    pub fn numbers(args: &[u64]) -> Result<Self, ChError> {
        let (offset, length, step) = match *args {
            [] => (0, None, 1),
            [n] => (0, Some(n), 1),
            [o, n] => (o, Some(n), 1),
            [o, n, s] => (o, Some(n), s),
            _ => {
                return Err(ChError::number_of_arguments_doesnt_match(format!(
                    "Table function 'numbers' requires at most 3 arguments, got {}",
                    args.len()
                )));
            }
        };
        if step == 0 {
            return Err(ChError::bad_arguments(
                "Table function 'numbers' requires step to be a positive number",
            ));
        }
        Ok(Self {
            offset,
            count: length.map(|l| l.div_ceil(step)),
            step,
            zeros: false,
        })
    }

    /// `zeros(args…)`'s rows.
    ///
    /// # Errors
    ///
    /// `42` for more than one argument.
    pub fn zeros(args: &[u64]) -> Result<Self, ChError> {
        let count = match *args {
            [] => None,
            [n] => Some(n),
            _ => {
                return Err(ChError::number_of_arguments_doesnt_match(format!(
                    "Table function 'zeros' requires at most 1 argument, got {}",
                    args.len()
                )));
            }
        };
        Ok(Self {
            offset: 0,
            count,
            step: 1,
            zeros: true,
        })
    }

    /// The `n` rows from the `first`-th, as one column.
    pub fn column(&self, first: u64, n: usize) -> ArrayRef {
        if self.zeros {
            return Arc::new(UInt8Array::from(vec![0u8; n]));
        }
        let values: UInt64Array = (0..n as u64)
            .map(|i| {
                self.offset
                    .wrapping_add(first.wrapping_add(i).wrapping_mul(self.step))
            })
            .collect();
        Arc::new(values)
    }

    fn schema(&self) -> Result<SchemaRef, ChError> {
        build::schema(if self.zeros {
            shapes::ZEROS
        } else {
            shapes::NUMBERS
        })
    }
}

/// The one partition of a numbers source.
#[derive(Debug)]
struct SeriesPartition {
    schema: SchemaRef,
    series: Series,
}

impl PartitionStream for SeriesPartition {
    fn schema(&self) -> &SchemaRef {
        &self.schema
    }

    fn execute(&self, ctx: Arc<TaskContext>) -> SendableRecordBatchStream {
        let batch_size = ctx.session_config().batch_size().max(1) as u64;
        let series = self.series;
        let schema = self.schema.clone();
        let stream = async_stream(move |mut emitter| async move {
            let mut done: u64 = 0;
            loop {
                let left = match series.count {
                    Some(c) => c - done,
                    None => u64::MAX,
                };
                if left == 0 {
                    break;
                }
                let n = left.min(batch_size);
                let batch =
                    RecordBatch::try_new(schema.clone(), vec![series.column(done, n as usize)])
                        .map_err(Into::into);
                let failed = batch.is_err();
                emitter.emit(batch).await;
                if failed {
                    break;
                }
                done = done.wrapping_add(n);
            }
        });
        Box::pin(RecordBatchStreamAdapter::new(self.schema.clone(), stream))
    }
}

/// A numbers source as a table.
///
/// # Errors
///
/// A schema that does not build (a programming error).
pub fn provider(series: Series) -> Result<Arc<dyn TableProvider>, ChError> {
    let schema = series.schema()?;
    let partition = Arc::new(SeriesPartition {
        schema: schema.clone(),
        series,
    });
    let table = StreamingTable::try_new(schema, vec![partition])
        .map_err(|e| ChError::logical_error(e.to_string()))?;
    Ok(Arc::new(table))
}

/// A table function argument as a count.
fn count_arg(name: &str, v: &ScalarValue) -> Result<u64, ChError> {
    let bad = || {
        ChError::illegal_type_of_argument(format!(
            "The arguments of table function '{name}' must be non-negative integer constants, not {v}"
        ))
    };
    match v {
        ScalarValue::UInt64(Some(n)) => Ok(*n),
        ScalarValue::UInt32(Some(n)) => Ok(u64::from(*n)),
        ScalarValue::UInt16(Some(n)) => Ok(u64::from(*n)),
        ScalarValue::UInt8(Some(n)) => Ok(u64::from(*n)),
        ScalarValue::Int64(Some(n)) => u64::try_from(*n).map_err(|_| bad()),
        ScalarValue::Int32(Some(n)) => u64::try_from(*n).map_err(|_| bad()),
        _ => Err(bad()),
    }
}

/// The table function `name(args)`, if it is one of these.
pub fn table_function(
    name: &str,
    args: &[ScalarValue],
) -> Option<Result<Arc<dyn TableProvider>, ChError>> {
    let numbers = match name {
        "numbers" | "numbers_mt" => true,
        "zeros" | "zeros_mt" => false,
        _ => return None,
    };
    Some((|| {
        let values = args
            .iter()
            .map(|v| count_arg(name, v))
            .collect::<Result<Vec<_>, _>>()?;
        let series = if numbers {
            Series::numbers(&values)?
        } else {
            Series::zeros(&values)?
        };
        provider(series)
    })())
}

/// `system.one`: one row, `dummy = 0`.
///
/// # Errors
///
/// A schema that does not build (a programming error).
pub fn one() -> Result<Arc<dyn TableProvider>, ChError> {
    let mut rows = Rows::new(&["dummy"]);
    rows.push(vec![Cell::UInt(0)]);
    Ok(build::table(shapes::ONE, &rows)?)
}

/// `system.numbers`, `numbers_mt`, `zeros`, `zeros_mt` and `one`: the
/// `system` tables that need no catalog.
pub fn system_table(name: &str) -> Option<Arc<dyn TableProvider>> {
    let found = match name {
        "numbers" | "numbers_mt" => provider(Series {
            offset: 0,
            count: None,
            step: 1,
            zeros: false,
        }),
        "zeros" | "zeros_mt" => provider(Series {
            offset: 0,
            count: None,
            step: 1,
            zeros: true,
        }),
        "one" => one(),
        _ => return None,
    };
    found.ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::AsArray;
    use datafusion::arrow::datatypes::UInt64Type;

    fn values(s: &Series, n: usize) -> Vec<u64> {
        s.column(0, n)
            .as_primitive::<UInt64Type>()
            .values()
            .to_vec()
    }

    #[test]
    fn numbers_follow_clickhouse() {
        assert_eq!(Series::numbers(&[3]).unwrap().count, Some(3));
        let s = Series::numbers(&[10, 10, 2]).unwrap();
        assert_eq!(s.count, Some(5));
        assert_eq!(values(&s, 5), vec![10, 12, 14, 16, 18]);
        // numbers(5, 1000, 7): 143 rows (03720's reference).
        assert_eq!(Series::numbers(&[5, 1000, 7]).unwrap().count, Some(143));
        // Values wrap at 2^64.
        let s = Series::numbers(&[u64::MAX - 1, 5, 1]).unwrap();
        assert_eq!(values(&s, 5), vec![u64::MAX - 1, u64::MAX, 0, 1, 2]);
        assert_eq!(Series::numbers(&[]).unwrap().count, None);
        assert_eq!(Series::numbers(&[1, 2, 0]).unwrap_err().code, 36);
        assert_eq!(Series::numbers(&[1, 2, 3, 4]).unwrap_err().code, 42);
        assert_eq!(Series::zeros(&[4]).unwrap().count, Some(4));
        assert_eq!(Series::zeros(&[1, 2]).unwrap_err().code, 42);
    }

    #[test]
    fn arguments_are_counts() {
        assert!(table_function("numbers", &[ScalarValue::Int64(Some(10))]).is_some());
        let e = match table_function("numbers", &[ScalarValue::Int64(Some(-1))]) {
            Some(Err(e)) => e,
            other => panic!("{other:?}"),
        };
        assert_eq!(e.code, 43);
        assert!(table_function("url", &[]).is_none());
        assert!(system_table("numbers").is_some());
        assert!(system_table("one").is_some());
        assert!(system_table("tables").is_none());
    }
}
