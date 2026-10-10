//! An in-memory [`SqlRuntime`] for saga tests: records every call, injects
//! one-shot failures before or after the effect, can hold new members in
//! `Starting`, crash members, and replaces every member on a class change.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;

use super::{JobOutcome, JobSpec, Member, MemberState, PoolStatus, RuntimeError, SqlRuntime};
use crate::model::{BranchId, Class};

/// A runtime operation, for [`FakeRuntime::fail_next`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Op {
    /// [`SqlRuntime::ensure_pool`].
    EnsurePool,
    /// [`SqlRuntime::scale`].
    Scale,
    /// [`SqlRuntime::pool_status`].
    PoolStatus,
    /// [`SqlRuntime::delete_pool`].
    DeletePool,
    /// [`SqlRuntime::run_job`].
    RunJob,
}

/// A recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// `ensure_pool(branch, class, replicas)`.
    EnsurePool(BranchId, Class, u32),
    /// `scale(branch, replicas)`.
    Scale(BranchId, u32),
    /// `pool_status(branch)`.
    PoolStatus(BranchId),
    /// `delete_pool(branch)`.
    DeletePool(BranchId),
    /// `run_job(spec)`, by job name.
    RunJob(String),
}

#[derive(Debug, Clone, Copy)]
struct Sim {
    /// Bumped whenever the member is replaced (class change, after exit).
    generation: u32,
    starting: bool,
    exited: Option<Option<i32>>,
}

#[derive(Debug)]
struct Pool {
    class: Class,
    replicas: u32,
    members: BTreeMap<u32, Sim>,
}

#[derive(Debug, Default)]
struct State {
    pools: BTreeMap<BranchId, Pool>,
    calls: Vec<Call>,
    fail: BTreeSet<Op>,
    fail_after: BTreeSet<Op>,
    hold_starting: bool,
    job_exit_code: i32,
    next_generation: u32,
}

/// See the module docs.
#[derive(Debug, Default)]
pub struct FakeRuntime {
    state: Mutex<State>,
}

impl FakeRuntime {
    /// An empty runtime: no pools, jobs exit 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next call of `op` fails with [`RuntimeError::Unavailable`] and
    /// changes nothing.
    pub fn fail_next(&self, op: Op) {
        self.lock().fail.insert(op);
    }

    /// The next call of `op` takes effect, then fails with
    /// [`RuntimeError::Unavailable`] (a crash after the effect).
    pub fn fail_after_next(&self, op: Op) {
        self.lock().fail_after.insert(op);
    }

    /// Marks a member as exited (a crash or OOM kill); the next
    /// `ensure_pool` or `scale` replaces it.
    pub fn crash_member(&self, branch: &BranchId, index: u32, code: Option<i32>) {
        if let Some(m) = self
            .lock()
            .pools
            .get_mut(branch)
            .and_then(|p| p.members.get_mut(&index))
        {
            m.exited = Some(code);
        }
    }

    /// While on, members created by `ensure_pool` or `scale` stay
    /// `Starting`; turning it off makes every member ready.
    pub fn hold_starting(&self, on: bool) {
        let mut st = self.lock();
        st.hold_starting = on;
        if !on {
            for m in st.pools.values_mut().flat_map(|p| p.members.values_mut()) {
                m.starting = false;
            }
        }
    }

    /// The exit code later jobs report.
    pub fn set_job_exit_code(&self, code: i32) {
        self.lock().job_exit_code = code;
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.lock().calls.clone()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicking test thread must not hide the state from the others.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl State {
    fn enter(&mut self, op: Op, call: Call) -> Result<(), RuntimeError> {
        self.calls.push(call);
        if self.fail.remove(&op) {
            return Err(RuntimeError::Unavailable(format!(
                "injected failure of {op:?}"
            )));
        }
        Ok(())
    }

    /// The result of an applied call, or the injected after-effect failure.
    fn leave<T>(&mut self, op: Op, value: T) -> Result<T, RuntimeError> {
        if self.fail_after.remove(&op) {
            return Err(RuntimeError::Unavailable(format!(
                "injected failure after {op:?}"
            )));
        }
        Ok(value)
    }

    fn fresh(&mut self) -> Sim {
        self.next_generation += 1;
        Sim {
            generation: self.next_generation,
            starting: self.hold_starting,
            exited: None,
        }
    }

    /// Brings `branch` to `replicas` at `class`: a class change replaces
    /// every member, an exited member is replaced, extra members go.
    fn reconcile(&mut self, branch: &BranchId, class: Class, replicas: u32) {
        let Some(pool) = self.pools.get(branch) else {
            return;
        };
        let restart_all = pool.class != class;
        let keep: Vec<(u32, Sim)> = pool
            .members
            .iter()
            .filter(|(i, m)| **i < replicas && !restart_all && m.exited.is_none())
            .map(|(i, m)| (*i, *m))
            .collect();
        let mut members: BTreeMap<u32, Sim> = keep.into_iter().collect();
        for index in 0..replicas {
            members.entry(index).or_insert_with(|| self.fresh());
        }
        if let Some(pool) = self.pools.get_mut(branch) {
            pool.class = class;
            pool.replicas = replicas;
            pool.members = members;
        }
    }

    fn status(&self, branch: &BranchId) -> Option<PoolStatus> {
        let pool = self.pools.get(branch)?;
        let members = pool
            .members
            .iter()
            .map(|(&index, sim)| {
                let port = 24_000 + u16::try_from(index).unwrap_or(u16::MAX - 24_000);
                let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
                Member {
                    index,
                    name: format!("fake-{branch}-{index}-g{}", sim.generation),
                    mysql_addr: SocketAddr::new(ip, port),
                    status_addr: SocketAddr::new(ip, port + 1_000),
                    state: match (sim.exited, sim.starting) {
                        (Some(code), _) => MemberState::Exited { code },
                        (None, true) => MemberState::Starting,
                        (None, false) => MemberState::Ready,
                    },
                }
            })
            .collect();
        Some(PoolStatus {
            branch: branch.clone(),
            class: pool.class,
            replicas: pool.replicas,
            members,
        })
    }
}

#[async_trait]
impl SqlRuntime for FakeRuntime {
    async fn ensure_pool(
        &self,
        branch: &BranchId,
        class: Class,
        replicas: u32,
    ) -> Result<PoolStatus, RuntimeError> {
        let mut st = self.lock();
        st.enter(
            Op::EnsurePool,
            Call::EnsurePool(branch.clone(), class, replicas),
        )?;
        st.pools.entry(branch.clone()).or_insert(Pool {
            class,
            replicas: 0,
            members: BTreeMap::new(),
        });
        st.reconcile(branch, class, replicas);
        let status = st
            .status(branch)
            .ok_or_else(|| RuntimeError::NoPool(branch.clone()))?;
        st.leave(Op::EnsurePool, status)
    }

    async fn scale(&self, branch: &BranchId, replicas: u32) -> Result<PoolStatus, RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::Scale, Call::Scale(branch.clone(), replicas))?;
        let class = st
            .pools
            .get(branch)
            .map(|p| p.class)
            .ok_or_else(|| RuntimeError::NoPool(branch.clone()))?;
        st.reconcile(branch, class, replicas);
        let status = st
            .status(branch)
            .ok_or_else(|| RuntimeError::NoPool(branch.clone()))?;
        st.leave(Op::Scale, status)
    }

    async fn pool_status(&self, branch: &BranchId) -> Result<Option<PoolStatus>, RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::PoolStatus, Call::PoolStatus(branch.clone()))?;
        let status = st.status(branch);
        st.leave(Op::PoolStatus, status)
    }

    async fn delete_pool(&self, branch: &BranchId) -> Result<(), RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::DeletePool, Call::DeletePool(branch.clone()))?;
        st.pools.remove(branch);
        st.leave(Op::DeletePool, ())
    }

    async fn run_job(&self, spec: &JobSpec) -> Result<JobOutcome, RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::RunJob, Call::RunJob(spec.name.clone()))?;
        let outcome = JobOutcome {
            exit_code: st.job_exit_code,
            log_tail: String::new(),
        };
        st.leave(Op::RunJob, outcome)
    }
}
