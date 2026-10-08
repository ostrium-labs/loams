//! An in-memory [`SqlRuntime`] for saga tests: records every call, injects
//! one-shot failures, and can hold new members in `Starting`.

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

#[derive(Debug)]
struct Pool {
    class: Class,
    replicas: u32,
    /// Member indexes still `Starting`.
    starting: BTreeSet<u32>,
}

#[derive(Debug, Default)]
struct State {
    pools: BTreeMap<BranchId, Pool>,
    calls: Vec<Call>,
    fail: BTreeSet<Op>,
    hold_starting: bool,
    job_exit_code: i32,
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

    /// While on, members created by `ensure_pool` or `scale` stay
    /// `Starting`; turning it off makes every member ready.
    pub fn hold_starting(&self, on: bool) {
        let mut st = self.lock();
        st.hold_starting = on;
        if !on {
            for pool in st.pools.values_mut() {
                pool.starting.clear();
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

    fn resize(&mut self, branch: &BranchId, replicas: u32) {
        let hold = self.hold_starting;
        if let Some(pool) = self.pools.get_mut(branch) {
            if hold {
                pool.starting.extend(pool.replicas..replicas);
            }
            pool.starting.retain(|&i| i < replicas);
            pool.replicas = replicas;
        }
    }

    fn status(&self, branch: &BranchId) -> Option<PoolStatus> {
        let pool = self.pools.get(branch)?;
        let members = (0..pool.replicas)
            .map(|index| {
                let port = 24_000 + u16::try_from(index).unwrap_or(u16::MAX - 24_000);
                let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
                Member {
                    index,
                    name: format!("fake-{branch}-{index}"),
                    mysql_addr: SocketAddr::new(ip, port),
                    status_addr: SocketAddr::new(ip, port + 1_000),
                    state: if pool.starting.contains(&index) {
                        MemberState::Starting
                    } else {
                        MemberState::Ready
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
        st.pools
            .entry(branch.clone())
            .or_insert(Pool {
                class,
                replicas: 0,
                starting: BTreeSet::new(),
            })
            .class = class;
        st.resize(branch, replicas);
        st.status(branch)
            .ok_or_else(|| RuntimeError::NoPool(branch.clone()))
    }

    async fn scale(&self, branch: &BranchId, replicas: u32) -> Result<PoolStatus, RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::Scale, Call::Scale(branch.clone(), replicas))?;
        if !st.pools.contains_key(branch) {
            return Err(RuntimeError::NoPool(branch.clone()));
        }
        st.resize(branch, replicas);
        st.status(branch)
            .ok_or_else(|| RuntimeError::NoPool(branch.clone()))
    }

    async fn pool_status(&self, branch: &BranchId) -> Result<Option<PoolStatus>, RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::PoolStatus, Call::PoolStatus(branch.clone()))?;
        Ok(st.status(branch))
    }

    async fn delete_pool(&self, branch: &BranchId) -> Result<(), RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::DeletePool, Call::DeletePool(branch.clone()))?;
        st.pools.remove(branch);
        Ok(())
    }

    async fn run_job(&self, spec: &JobSpec) -> Result<JobOutcome, RuntimeError> {
        let mut st = self.lock();
        st.enter(Op::RunJob, Call::RunJob(spec.name.clone()))?;
        Ok(JobOutcome {
            exit_code: st.job_exit_code,
            log_tail: String::new(),
        })
    }
}
