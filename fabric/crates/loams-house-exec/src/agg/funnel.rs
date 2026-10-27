//! `windowFunnel(window[, mode, …])(timestamp, cond1, …, condN)`, ported
//! from ClickHouse 26.9's
//! `src/AggregateFunctions/AggregateFunctionWindowFunnel.cpp` (Apache-2.0),
//! with every mode: `strict_deduplication`, `strict_order`,
//! `strict_increase`, `strict_once` and `allow_reentry`.
//!
//! The answer is the length of the longest prefix of the condition chain
//! whose events happen in order within `window` of the first one. The
//! timestamp is an unsigned integer, a `Date` or a `DateTime`; the
//! conditions are `UInt8` (or `Bool`).

use std::sync::Arc;

use datafusion::arrow::array::{Array, ArrayRef, AsArray, UInt8Array, UInt64Array};
use datafusion::arrow::compute::cast;
use datafusion::arrow::datatypes::{DataType, UInt8Type, UInt64Type};
use datafusion::common::{Result, ScalarValue};
use datafusion::logical_expr::Accumulator;
use loams_house_sql::aggregates::Param;
use loams_house_types::{ChError, ChType};

use super::combinators::condition;
use super::{Agg, AggRef, state_list, state_list_type};

/// ClickHouse's `MAX_EVENTS`.
const MAX_EVENTS: usize = 32;

/// The funnel's options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    /// `strict_deduplication`: a repeated condition stops the chain.
    pub strict_deduplication: bool,
    /// `strict_order`: no other event may come between the chain's events.
    pub strict_order: bool,
    /// `strict_increase`: the chain's timestamps strictly increase.
    pub strict_increase: bool,
    /// `strict_once`: an event counts once even when it meets several
    /// conditions.
    pub strict_once: bool,
    /// `allow_reentry` (with `strict_order`): an event of a later step seen
    /// too early is skipped rather than ending the chain.
    pub allow_reentry: bool,
}

/// `windowFunnel` with `params` over `args`.
pub fn window_funnel(params: &[Param], args: &[ChType]) -> Result<AggRef, ChError> {
    let Some(window) = params.first() else {
        return Err(ChError::number_of_arguments_doesnt_match(
            "Aggregate function windowFunnel requires at least one parameter: <window>, [option, [option, ...]]",
        ));
    };
    let window = window.as_u64().ok_or_else(|| {
        ChError::bad_arguments(format!(
            "The window parameter of windowFunnel must be a non-negative integer, got {window}"
        ))
    })?;
    let mut modes = Modes::default();
    for p in &params[1..] {
        match p.as_str() {
            Some("strict_deduplication") => modes.strict_deduplication = true,
            Some("strict_order") => modes.strict_order = true,
            Some("strict_increase") => modes.strict_increase = true,
            Some("allow_reentry") => modes.allow_reentry = true,
            Some("strict_once") => modes.strict_once = true,
            Some("strict") => {
                return Err(ChError::bad_arguments(
                    "strict is replaced with strict_deduplication in Aggregate function windowFunnel",
                ));
            }
            _ => {
                return Err(ChError::bad_arguments(format!(
                    "Aggregate function windowFunnel doesn't support a parameter: {p}"
                )));
            }
        }
    }
    if modes.allow_reentry && !modes.strict_order {
        return Err(ChError::bad_arguments(
            "allow_reentry option requires strict_order to be enabled in Aggregate function windowFunnel",
        ));
    }
    if args.len() < 2 {
        return Err(ChError::number_of_arguments_doesnt_match(
            "Aggregate function windowFunnel requires one timestamp argument and at least one event condition.",
        ));
    }
    if args.len() > MAX_EVENTS + 1 {
        return Err(ChError::number_of_arguments_doesnt_match(
            "Too many event arguments for aggregate function windowFunnel",
        ));
    }
    for (i, c) in args.iter().enumerate().skip(1) {
        if !matches!(c, ChType::UInt8 | ChType::Bool) {
            return Err(ChError::illegal_type_of_argument(format!(
                "Illegal type {c} of argument {} of aggregate function windowFunnel, must be UInt8",
                i + 1
            )));
        }
    }
    match &args[0] {
        ChType::UInt8
        | ChType::UInt16
        | ChType::UInt32
        | ChType::UInt64
        | ChType::Date
        | ChType::DateTime(_) => {}
        other => {
            return Err(ChError::illegal_type_of_argument(format!(
                "Illegal type {other} of first argument of aggregate function windowFunnel, must be Unsigned Number, Date, DateTime"
            )));
        }
    }
    Ok(Arc::new(Funnel {
        window,
        events: (args.len() - 1) as u8,
        modes,
        result: ChType::UInt8,
    }))
}

#[derive(Debug)]
struct Funnel {
    window: u64,
    events: u8,
    modes: Modes,
    result: ChType,
}

impl Agg for Funnel {
    fn result(&self) -> &ChType {
        &self.result
    }

    fn state(&self) -> Vec<DataType> {
        vec![
            state_list_type(&DataType::UInt64),
            state_list_type(&DataType::UInt8),
            state_list_type(&DataType::UInt64),
        ]
    }

    fn accumulator(&self) -> Result<Box<dyn Accumulator>> {
        Ok(Box::new(FunnelAcc {
            funnel: Level {
                window: self.window,
                events: self.events,
                modes: self.modes,
            },
            list: Vec::new(),
            next_id: 1,
        }))
    }
}

/// One event: (timestamp, condition number from 1, or 0 for "no condition"
/// under `strict_order`; unique id under `strict_once`).
pub type Event = (u64, u8, u64);

/// The level computation, separate from the accumulator so it can be tested
/// on event lists.
#[derive(Debug, Clone, Copy)]
pub struct Level {
    /// The window.
    pub window: u64,
    /// The number of conditions.
    pub events: u8,
    /// The modes.
    pub modes: Modes,
}

impl Level {
    /// The events one input row adds, as `add` does: conditions from the last
    /// to the first, and a 0 event under `strict_order` when none holds.
    pub fn add(&self, list: &mut Vec<Event>, ts: u64, conds: &[bool], id: u64) {
        let mut any = false;
        for i in (1..=self.events).rev() {
            if conds[usize::from(i - 1)] {
                list.push((ts, i, id));
                any = true;
            }
        }
        if self.modes.strict_order && !any {
            list.push((ts, 0, id));
        }
    }

    /// The level reached by `list` (`getEventLevel`).
    pub fn level(&self, list: &mut [Event]) -> u8 {
        if list.is_empty() {
            return 0;
        }
        if !self.modes.strict_order && self.events == 1 {
            return 1;
        }
        if self.modes.strict_once {
            list.sort_by_key(|e| (e.0, e.1, e.2));
            self.level_strict_once(list)
        } else {
            list.sort_by_key(|e| (e.0, e.1));
            self.level_plain(list)
        }
    }

    fn level_plain(&self, list: &[Event]) -> u8 {
        let n = usize::from(self.events);
        let m = &self.modes;
        let mut ts: Vec<Option<(u64, u64)>> = vec![None; n];
        let mut first_event = false;
        let highest = |ts: &[Option<(u64, u64)>]| {
            (1..=ts.len())
                .rev()
                .find(|&e| ts[e - 1].is_some())
                .map_or(0, |e| e as u8)
        };
        for &(timestamp, event, _) in list {
            let idx = i32::from(event) - 1;
            if m.strict_order && idx == -1 {
                if first_event {
                    break;
                }
                continue;
            }
            let idx = idx as usize;
            if idx == 0 {
                ts[0] = Some((timestamp, timestamp));
                first_event = true;
            } else if m.strict_deduplication && ts[idx].is_some() {
                return highest(&ts);
            } else if m.strict_order && first_event && ts[idx - 1].is_none() {
                if m.allow_reentry {
                    continue;
                }
                if let Some(e) = (0..n).find(|&e| ts[e].is_none()) {
                    return e as u8;
                }
            } else if let Some((first, prev)) = ts[idx - 1] {
                let mut matched = timestamp <= first.wrapping_add(self.window);
                if m.strict_increase {
                    matched = matched && prev < timestamp;
                }
                if matched {
                    ts[idx] = Some((first, timestamp));
                    if idx + 1 == n {
                        return self.events;
                    }
                }
            }
        }
        highest(&ts)
    }

    fn level_strict_once(&self, list: &[Event]) -> u8 {
        #[derive(Clone)]
        struct Seq {
            first: u64,
            last: u64,
            path: [u64; MAX_EVENTS],
        }
        let n = usize::from(self.events);
        let m = &self.modes;
        let mut seqs: Vec<Vec<Seq>> = vec![Vec::new(); n];
        let mut has_first = false;
        let highest = |seqs: &[Vec<Seq>]| {
            (1..=seqs.len())
                .rev()
                .find(|&e| !seqs[e - 1].is_empty())
                .map_or(0, |e| e as u8)
        };
        for &(timestamp, event, id) in list {
            let idx = i64::from(event) - 1;
            if m.strict_order && idx == -1 {
                if has_first {
                    break;
                }
                continue;
            }
            let idx = idx as usize;
            if idx == 0 {
                let mut path = [0u64; MAX_EVENTS];
                path[0] = id;
                seqs[0].push(Seq {
                    first: timestamp,
                    last: timestamp,
                    path,
                });
                has_first = true;
            } else if m.strict_deduplication && !seqs[idx].is_empty() {
                return highest(&seqs);
            } else if m.strict_order && has_first && seqs[idx - 1].is_empty() {
                if m.allow_reentry {
                    continue;
                }
                if let Some(e) = (0..n).find(|&e| seqs[e].is_empty()) {
                    return e as u8;
                }
            } else if !seqs[idx - 1].is_empty() {
                let mut k = 0;
                while k < seqs[idx - 1].len() {
                    let s = seqs[idx - 1][k].clone();
                    let mut matched = timestamp <= s.first.wrapping_add(self.window);
                    if !matched && seqs[idx - 1].len() > 1 {
                        // Out of the window: dropped, keeping at least one.
                        seqs[idx - 1].remove(k);
                        continue;
                    }
                    for j in 0..idx {
                        if !matched {
                            break;
                        }
                        matched = s.path[j] != id;
                    }
                    if matched && m.strict_increase {
                        matched = s.last < timestamp;
                    }
                    if matched {
                        let mut path = s.path;
                        path[idx] = id;
                        seqs[idx].push(Seq {
                            first: s.first,
                            last: timestamp,
                            path,
                        });
                        if idx + 1 == n {
                            return self.events;
                        }
                    }
                    k += 1;
                }
            }
        }
        highest(&seqs)
    }
}

#[derive(Debug)]
struct FunnelAcc {
    funnel: Level,
    list: Vec<Event>,
    next_id: u64,
}

/// The timestamp column as `u64`: unsigned integers as they are, `Date` as
/// days, `DateTime` as seconds.
fn timestamps(a: &ArrayRef) -> Result<UInt64Array> {
    let a = match a.data_type() {
        DataType::Date32 | DataType::Timestamp(..) => {
            cast(&cast(a, &DataType::Int64)?, &DataType::UInt64)?
        }
        _ => cast(a, &DataType::UInt64)?,
    };
    Ok(a.as_primitive::<UInt64Type>().clone())
}

impl Accumulator for FunnelAcc {
    fn update_batch(&mut self, values: &[ArrayRef]) -> Result<()> {
        let ts = timestamps(&values[0])?;
        let conds = values[1..]
            .iter()
            .map(condition)
            .collect::<Result<Vec<_>>>()?;
        let mut row = vec![false; conds.len()];
        for i in 0..ts.len() {
            for (c, cond) in conds.iter().enumerate() {
                row[c] = cond.is_valid(i) && cond.value(i);
            }
            self.funnel
                .add(&mut self.list, ts.value(i), &row, self.next_id);
            if self.funnel.modes.strict_once {
                self.next_id += 1;
            }
        }
        Ok(())
    }

    fn evaluate(&mut self) -> Result<ScalarValue> {
        let mut list = self.list.clone();
        Ok(ScalarValue::UInt8(Some(self.funnel.level(&mut list))))
    }

    fn size(&self) -> usize {
        size_of::<Self>() + self.list.capacity() * size_of::<Event>()
    }

    fn state(&mut self) -> Result<Vec<ScalarValue>> {
        Ok(vec![
            state_list(Arc::new(UInt64Array::from_iter_values(
                self.list.iter().map(|e| e.0),
            )))?,
            state_list(Arc::new(UInt8Array::from_iter_values(
                self.list.iter().map(|e| e.1),
            )))?,
            state_list(Arc::new(UInt64Array::from_iter_values(
                self.list.iter().map(|e| e.2),
            )))?,
        ])
    }

    fn merge_batch(&mut self, states: &[ArrayRef]) -> Result<()> {
        for row in 0..states[0].len() {
            let (Some(ts), Some(ev), Some(ids)) = (
                super::state_list_row(&states[0], row),
                super::state_list_row(&states[1], row),
                super::state_list_row(&states[2], row),
            ) else {
                continue;
            };
            let ts = ts.as_primitive::<UInt64Type>();
            let ev = ev.as_primitive::<UInt8Type>();
            let ids = ids.as_primitive::<UInt64Type>();
            // `AggregateFunctionWindowFunnelStrictOnceData::merge`: the other
            // side's ids move past this side's.
            let mut next = self.next_id;
            for i in 0..ts.len() {
                let id = if self.funnel.modes.strict_once {
                    let id = ids.value(i) + self.next_id;
                    next = next.max(id + 1);
                    id
                } else {
                    0
                };
                self.list.push((ts.value(i), ev.value(i), id));
            }
            self.next_id = next;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(window: u64, modes: Modes, events: u8, rows: &[(u64, &[bool])]) -> u8 {
        let f = Level {
            window,
            events,
            modes,
        };
        let mut list = Vec::new();
        for (i, (ts, conds)) in rows.iter().enumerate() {
            f.add(&mut list, *ts, conds, i as u64 + 1);
        }
        f.level(&mut list)
    }

    #[test]
    fn plain_and_strict_order() {
        let t = true;
        let f = false;
        // A -> B -> D -> C, looking for A, B, C.
        let rows: &[(u64, &[bool])] = &[
            (1, &[t, f, f]),
            (2, &[f, t, f]),
            (3, &[f, f, f]),
            (4, &[f, f, t]),
        ];
        assert_eq!(level(10, Modes::default(), 3, rows), 3);
        let strict = Modes {
            strict_order: true,
            ..Modes::default()
        };
        assert_eq!(level(10, strict, 3, rows), 2);
        assert_eq!(level(2, Modes::default(), 3, rows), 2);
    }
}
