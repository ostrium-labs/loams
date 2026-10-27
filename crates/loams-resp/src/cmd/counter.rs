//! Counter commands (plan KV2 Task 4): `INCR`, `DECR`, `INCRBY`, `DECRBY`,
//! `INCRBYFLOAT`, with Valkey's error texts.

use std::sync::Arc;

use bytes::Bytes;

use loams_kv::TxnOptions;

use super::{Spec, int_arg, txn_with, wrongtype};
use crate::frame::Value;
use crate::server::{Conn, Shared};
use crate::types::counter::{self, IncrError};
use crate::types::now_ms;

pub(super) async fn run(
    shared: &Arc<Shared>,
    conn: &mut Conn,
    spec: &Spec,
    args: &[Bytes],
) -> Value {
    let db = conn.db;
    let key = args[1].clone();
    let by = match spec.name {
        "incr" => Ok(1),
        "decr" => Ok(-1),
        "incrby" => int_arg(&args[2]),
        "decrby" => int_arg(&args[2]).and_then(|n| {
            n.checked_neg()
                .ok_or_else(|| Value::err("ERR decrement would overflow"))
        }),
        "incrbyfloat" => return incr_by_float(shared, db, key, &args[2]).await,
        other => Err(Value::err(format!("ERR '{other}' has no handler"))),
    };
    let by = match by {
        Ok(by) => by,
        Err(e) => return e,
    };
    let result = txn_with(shared, counter_opts("resp.incr"), move |t| {
        let key = key.clone();
        Box::pin(async move { counter::incr_by(t, db, &key, by, now_ms()).await })
    })
    .await;
    match result {
        Ok(Ok(n)) => Value::Integer(n),
        Ok(Err(e)) => incr_error(e),
        Err(e) => e,
    }
}

async fn incr_by_float(shared: &Shared, db: u8, key: Bytes, by: &[u8]) -> Value {
    let Some(by) = counter::parse_float(by) else {
        return Value::err("ERR value is not a valid float");
    };
    let result = txn_with(shared, counter_opts("resp.incrbyfloat"), move |t| {
        let key = key.clone();
        Box::pin(async move { counter::incr_by_float(t, db, &key, by, now_ms()).await })
    })
    .await;
    match result {
        Ok(Ok(text)) => Value::bulk(text),
        Ok(Err(IncrError::NotANumber)) => Value::err("ERR value is not a valid float"),
        Ok(Err(e)) => incr_error(e),
        Err(e) => e,
    }
}

/// A counter is one hot key: concurrent increments conflict and the runner
/// retries them, so they get more attempts than the default 8 (still
/// bounded by the 10 s deadline).
fn counter_opts(op: &'static str) -> TxnOptions {
    TxnOptions {
        max_attempts: 64,
        ..TxnOptions::new(op)
    }
}

fn incr_error(e: IncrError) -> Value {
    match e {
        IncrError::WrongType => wrongtype(),
        IncrError::NotANumber => Value::err("ERR value is not an integer or out of range"),
        IncrError::Overflow => Value::err("ERR increment or decrement would overflow"),
        IncrError::NotFinite => Value::err("ERR increment would produce NaN or Infinity"),
    }
}
