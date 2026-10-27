//! The tikv backend (feature `tikv`): `loams-tikv`'s handle behind the seam.
//! Every type converts losslessly: [`Ts`] is the TSO version, the options,
//! error classes and fault hooks map one to one, and the runner is
//! `loams_tikv::Tikv::run_as` with the body working on [`Txn`].

use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use loams_tikv::{RunTxn, Tikv, Timestamp, TimestampExt};

use crate::gc::Inner;
use crate::{CommitMode, Committed, GcBarrier, KvError, Mode, Ts, Txn, TxnError, TxnOptions};

impl From<Timestamp> for Ts {
    fn from(ts: Timestamp) -> Self {
        Ts(ts.version())
    }
}

impl From<Ts> for Timestamp {
    fn from(ts: Ts) -> Self {
        Timestamp::from_version(ts.0)
    }
}

impl From<loams_tikv::TxnError> for TxnError {
    fn from(e: loams_tikv::TxnError) -> Self {
        use loams_tikv::TxnError as T;
        match e {
            T::Conflict => TxnError::Conflict,
            T::NotApplied(m) => TxnError::NotApplied(m),
            T::Undetermined { token } => TxnError::Undetermined { token },
            T::AlreadyExists(m) => TxnError::AlreadyExists(m),
            T::Fatal(m) => TxnError::Fatal(m),
            T::Deadline => TxnError::Deadline,
        }
    }
}

impl From<TxnError> for loams_tikv::TxnError {
    fn from(e: TxnError) -> Self {
        use loams_tikv::TxnError as T;
        match e {
            TxnError::Conflict => T::Conflict,
            TxnError::NotApplied(m) => T::NotApplied(m),
            TxnError::Undetermined { token } => T::Undetermined { token },
            TxnError::AlreadyExists(m) => T::AlreadyExists(m),
            TxnError::Fatal(m) => T::Fatal(m),
            TxnError::Deadline => T::Deadline,
        }
    }
}

fn commit_mode(mode: CommitMode) -> loams_tikv::CommitMode {
    match mode {
        CommitMode::Async1pc => loams_tikv::CommitMode::Async1pc,
        CommitMode::TwoPc => loams_tikv::CommitMode::TwoPc,
    }
}

impl From<TxnOptions> for loams_tikv::TxnOptions {
    fn from(o: TxnOptions) -> Self {
        loams_tikv::TxnOptions {
            mode: match o.mode {
                Mode::Optimistic => loams_tikv::Mode::Optimistic,
                Mode::Pessimistic => loams_tikv::Mode::Pessimistic,
            },
            max_attempts: o.max_attempts,
            deadline: o.deadline,
            commit_token: o.commit_token,
            op: o.op,
            commit_mode: o.commit_mode.map(commit_mode),
        }
    }
}

impl RunTxn for Txn {
    fn wrap(txn: loams_tikv::Txn) -> Self {
        Txn::Tikv(txn)
    }

    fn txn(&mut self) -> &mut loams_tikv::Txn {
        match self {
            Txn::Embedded(_) => unreachable!("the TiKV runner wraps TiKV transactions"),
            Txn::Tikv(t) => t,
        }
    }
}

/// [`Store::run`](crate::Store::run) on TiKV.
pub(crate) async fn run<T: Send, F>(
    tikv: &Tikv,
    opts: TxnOptions,
    body: F,
) -> Result<Committed<T>, TxnError>
where
    F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<T, TxnError>>,
{
    let pessimistic = opts.mode == Mode::Pessimistic;
    let until = crate::embedded::locks::lock_wait_until(Instant::now(), opts.deadline);
    let run = tikv.run_as::<Txn, T, TxnError, F>(opts.into(), body);
    // In a pessimistic run, lock waits give up with `Conflict` before the
    // run's deadline (KV0 Task 5), as on the embedded backend. The body
    // runs on this task, inside `run_as`, so it sees the task-local.
    let committed = if pessimistic {
        LOCK_UNTIL.scope(until.into(), run).await?
    } else {
        run.await?
    };
    Ok(Committed {
        value: committed.value,
        commit_ts: committed.commit_ts.into(),
        attempts: committed.attempts,
        earlier_unknown: committed.earlier_unknown,
    })
}

tokio::task_local! {
    /// The lock-wait limit of the pessimistic run whose body this is.
    static LOCK_UNTIL: tokio::time::Instant;
}

/// Runs a TiKV call that may wait for a pessimistic lock: in a pessimistic
/// run's body, a wait past the run's lock-wait limit is `Conflict` (the
/// runner rolls the transaction back, which releases whatever the call
/// locked); elsewhere the call as is.
pub(crate) async fn lock_wait<T>(
    call: impl Future<Output = Result<T, loams_tikv::TxnError>>,
) -> Result<T, TxnError> {
    match LOCK_UNTIL.try_with(|until| *until) {
        Ok(until) => match tokio::time::timeout_at(until, call).await {
            Ok(result) => result.map_err(TxnError::from),
            Err(_) => Err(TxnError::Conflict),
        },
        Err(_) => call.await.map_err(TxnError::from),
    }
}

/// [`Store::barrier`](crate::Store::barrier) on TiKV: a PD service safe
/// point `loams/<name>`.
pub(crate) async fn barrier(
    tikv: &Tikv,
    name: &str,
    at: Ts,
    ttl: Duration,
) -> Result<GcBarrier, KvError> {
    let service_id = format!("loams/{name}");
    let barrier = loams_tikv::GcBarrier::new(tikv);
    barrier.set(&service_id, &at.into(), ttl).await?;
    Ok(GcBarrier {
        service_id,
        at,
        inner: Inner::Tikv(barrier),
    })
}

/// Our fault plan as `loams-tikv`'s.
#[cfg(feature = "faults")]
pub(crate) fn faults(
    plan: std::sync::Arc<dyn crate::FaultPlan>,
) -> std::sync::Arc<dyn loams_tikv::FaultPlan> {
    std::sync::Arc::new(Faults(plan))
}

#[cfg(feature = "faults")]
struct Faults(std::sync::Arc<dyn crate::FaultPlan>);

#[cfg(feature = "faults")]
impl loams_tikv::FaultPlan for Faults {
    fn at(
        &self,
        op: &str,
        point: loams_tikv::FaultPoint,
        attempt: u32,
    ) -> Option<loams_tikv::Fault> {
        use crate::{Fault, FaultPoint};
        let point = match point {
            loams_tikv::FaultPoint::BeforeBegin => FaultPoint::BeforeBegin,
            loams_tikv::FaultPoint::BeforePrewrite => FaultPoint::BeforePrewrite,
            loams_tikv::FaultPoint::BeforeCommit => FaultPoint::BeforeCommit,
            loams_tikv::FaultPoint::AfterCommit => FaultPoint::AfterCommit,
        };
        self.0.at(op, point, attempt).map(|f| match f {
            Fault::Refuse => loams_tikv::Fault::Refuse,
            Fault::Conflict => loams_tikv::Fault::Conflict,
            Fault::LoseAck => loams_tikv::Fault::LoseAck,
            Fault::Delay(d) => loams_tikv::Fault::Delay(d),
        })
    }
}
