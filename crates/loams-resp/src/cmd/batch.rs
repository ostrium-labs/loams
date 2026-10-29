//! Pipelined batches (BQ1, owner 2026-10-28: "add pipelining in every
//! point"): a run of plain `GET key` or plain `SET key value` requests that
//! arrived in one socket read runs as **one** `loams-kv` transaction, whose
//! reads are one `batch_get` of the meta keys and whose writes commit
//! together, instead of one transaction (on TiKV: a TSO request, a read and
//! a commit) per command.
//!
//! The replies are the ones the commands would get one after another with
//! nothing in between, which Redis allows for a pipeline: a run of `GET`s
//! reads one snapshot; a run of `SET`s commits atomically (stronger than
//! needed, never weaker). Only runs of one kind are batched, so a `GET`
//! after a `SET` of the same key in the same read still sees it. A `SET`
//! run with a repeated key or a value too large to inline runs command by
//! command (the version bookkeeping of [`string::write`] assumes one write
//! per key and transaction).

use std::collections::HashSet;

use bytes::Bytes;

use super::{txn, wrongtype};
use crate::frame::Value;
use crate::layout::{self, Kind};
use crate::server::{Conn, ReplyMode, Shared};
use crate::types::{self, string};

/// The most requests one batch takes; a longer run is split.
pub const MAX_BATCH: usize = 1024;

/// What can be batched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Batch {
    Get,
    Set,
}

/// Whether `args` can join a batch on `conn`: an authenticated connection
/// replying normally, and a plain `GET key` or `SET key value`.
pub fn kind(conn: &Conn, args: &[Bytes]) -> Option<Batch> {
    if !conn.authenticated || conn.reply != ReplyMode::On {
        return None;
    }
    match args {
        [name, _] if name.eq_ignore_ascii_case(b"get") => Some(Batch::Get),
        [name, _, _] if name.eq_ignore_ascii_case(b"set") => Some(Batch::Set),
        _ => None,
    }
}

/// Whether a `SET` run can run as one transaction.
pub fn sets_batchable(run: &[Vec<Bytes>]) -> bool {
    let mut seen = HashSet::with_capacity(run.len());
    run.iter()
        .all(|a| a[2].len() <= layout::INLINE_MAX && seen.insert(a[1].clone()))
}

/// Runs a batch of `GET`s: one reply per request, in order.
pub async fn gets(shared: &Shared, conn: &Conn, run: &[Vec<Bytes>]) -> Vec<Value> {
    count(shared, conn, run.len(), "get");
    let db = conn.db;
    let keys: Vec<Bytes> = run.iter().map(|a| a[1].clone()).collect();
    let n = keys.len();
    let reads = txn(shared, "resp.get.batch", move |t| {
        let keys = keys.clone();
        Box::pin(async move {
            let now = types::now_ms();
            let metas = types::read_metas(t, db, &keys).await?;
            let mut out = Vec::with_capacity(keys.len());
            for (k, m) in keys.iter().zip(metas) {
                out.push(match m.filter(|m| !m.expired(now)) {
                    Some(m) if m.kind != Kind::String => Err(()),
                    Some(m) => Ok(Some(string::body(t, db, k, &m).await?)),
                    None => Ok(None),
                });
            }
            Ok(out)
        })
    })
    .await;
    match reads {
        Err(e) => vec![e; n],
        Ok(v) => v
            .into_iter()
            .map(|r| match r {
                Err(()) => wrongtype(),
                Ok(v) => v.map_or(Value::Null, Value::bulk),
            })
            .collect(),
    }
}

/// Runs a batch of plain `SET`s (see [`sets_batchable`]): `OK` each.
pub async fn sets(shared: &Shared, conn: &Conn, run: &[Vec<Bytes>]) -> Vec<Value> {
    count(shared, conn, run.len(), "set");
    let db = conn.db;
    let pairs: Vec<(Bytes, Bytes)> = run.iter().map(|a| (a[1].clone(), a[2].clone())).collect();
    let n = pairs.len();
    let done = txn(shared, "resp.set.batch", move |t| {
        let pairs = pairs.clone();
        Box::pin(async move {
            let keys: Vec<&Bytes> = pairs.iter().map(|(k, _)| k).collect();
            let stored = types::read_metas(t, db, &keys).await?;
            for ((k, v), old) in pairs.iter().zip(stored) {
                string::write(t, db, k, v, 0, old.as_ref()).await?;
            }
            Ok(())
        })
    })
    .await;
    match done {
        Err(e) => vec![e; n],
        Ok(()) => vec![Value::ok(); n],
    }
}

/// What `dispatch` does per command: the command counter and the client's
/// last command.
fn count(shared: &Shared, conn: &Conn, n: usize, name: &'static str) {
    shared.stats.commands.fetch_add(
        u64::try_from(n).unwrap_or(u64::MAX),
        std::sync::atomic::Ordering::Relaxed,
    );
    shared.with_client(conn.id, |c| c.last_command = Some(name.to_owned()));
}
