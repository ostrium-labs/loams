//! [`MemStore`]: the in-process store, for tests and the dev mode.
//!
//! One mutex guards everything, and no lock is held across an await, so
//! every operation is atomic. A commit checks everything before it changes
//! anything. It refuses what Postgres would refuse (NUL in a string), so the
//! two stores answer the same requests the same way.

use std::collections::{BTreeMap, HashMap};
#[cfg(feature = "faults")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokio::sync::watch;

use crate::{
    Commit, CurrentRun, EventRow, FIRST_EVENT_ID, Loaded, NamespaceRecord, OutboxRow, RunKey,
    STATUS_RUNNING, StartExecution, StartOutcome, Store, StoreError, VisibilityRow, check_commit,
    check_shards, shard_of,
};

/// The in-process store.
#[derive(Debug)]
pub struct MemStore {
    shards: u32,
    inner: Mutex<Inner>,
    wake: watch::Sender<u64>,
    #[cfg(feature = "faults")]
    fault: AtomicBool,
}

#[derive(Debug, Default)]
struct Inner {
    namespaces: BTreeMap<String, NamespaceRecord>,
    executions: HashMap<RunKey, Loaded>,
    history: HashMap<RunKey, Vec<EventRow>>,
    /// (namespace_id, workflow_id) → the current run.
    current: HashMap<(String, String), CurrentRun>,
    outbox: BTreeMap<i64, Outbox>,
    next_outbox_id: i64,
    visibility: HashMap<RunKey, VisibilityRow>,
}

#[derive(Debug)]
struct Outbox {
    row: OutboxRow,
    dispatched: bool,
    claimed_until: Option<Instant>,
}

impl Default for MemStore {
    fn default() -> Self {
        Self::new(crate::DEFAULT_SHARDS).unwrap_or_else(|_| unreachable!("64 shards is valid"))
    }
}

impl MemStore {
    /// A store with `shards` shards (1..=[`crate::MAX_SHARDS`]).
    pub fn new(shards: u32) -> Result<Self, StoreError> {
        check_shards(shards)?;
        Ok(Self {
            shards,
            inner: Mutex::new(Inner {
                next_outbox_id: 1,
                ..Inner::default()
            }),
            wake: watch::Sender::new(0),
            #[cfg(feature = "faults")]
            fault: AtomicBool::new(false),
        })
    }

    pub fn shards(&self) -> u32 {
        self.shards
    }

    /// Makes the next [`Store::commit`] fail after its checks pass, with
    /// [`StoreError::Unavailable`], leaving nothing written.
    #[cfg(feature = "faults")]
    pub fn fail_next_commit(&self) {
        self.fault.store(true, Ordering::SeqCst);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves Inner consistent: every
        // mutation happens after its checks.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn bump(&self) {
        self.wake.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Writes a checked commit. Called with the lock held.
    fn apply(&self, inner: &mut Inner, key: &RunKey, version: u64, c: Commit) {
        let shard_id = shard_of(&key.namespace_id, &key.workflow_id, self.shards);
        inner
            .history
            .entry(key.clone())
            .or_default()
            .extend(c.events);
        inner.executions.insert(
            key.clone(),
            Loaded {
                state_blob: c.state_blob,
                version,
                next_event_id: c.next_event_id,
                status: c.status,
            },
        );
        if let Some(cur) = inner
            .current
            .get_mut(&(key.namespace_id.clone(), key.workflow_id.clone()))
            && cur.run_id == key.run_id
        {
            cur.status = c.status;
        }
        for o in c.outbox {
            let id = inner.next_outbox_id;
            inner.next_outbox_id += 1;
            inner.outbox.insert(
                id,
                Outbox {
                    row: OutboxRow {
                        id,
                        shard_id,
                        key: key.clone(),
                        kind: o.kind,
                        resonate_id: o.resonate_id,
                        request: o.request,
                        attempts: 0,
                    },
                    dispatched: false,
                    claimed_until: None,
                },
            );
        }
        if let Some(v) = c.visibility {
            inner.visibility.insert(key.clone(), v);
        }
    }
}

fn text(field: &str, s: &str) -> Result<(), StoreError> {
    if s.contains('\0') {
        return Err(StoreError::InvalidArgument(format!("{field} contains NUL")));
    }
    Ok(())
}

fn json(field: &str, v: &serde_json::Value) -> Result<(), StoreError> {
    use serde_json::Value;
    match v {
        Value::String(s) => text(field, s),
        Value::Array(a) => a.iter().try_for_each(|v| json(field, v)),
        Value::Object(o) => o.iter().try_for_each(|(k, v)| {
            text(field, k)?;
            json(field, v)
        }),
        _ => Ok(()),
    }
}

fn check_key(key: &RunKey) -> Result<(), StoreError> {
    text("namespace_id", &key.namespace_id)?;
    text("workflow_id", &key.workflow_id)?;
    text("run_id", &key.run_id)
}

fn check_strings(c: &Commit) -> Result<(), StoreError> {
    for o in &c.outbox {
        text("outbox kind", &o.kind)?;
        text("outbox resonate_id", &o.resonate_id)?;
        json("outbox request", &o.request)?;
    }
    if let Some(v) = &c.visibility {
        text("workflow_type", &v.workflow_type)?;
        text("task_queue", &v.task_queue)?;
        json("search_attrs", &v.search_attrs)?;
    }
    Ok(())
}

#[async_trait]
impl Store for MemStore {
    async fn create_namespace(&self, ns: NamespaceRecord) -> Result<(), StoreError> {
        text("namespace_id", &ns.namespace_id)?;
        text("loams_namespace_id", &ns.loams_namespace_id)?;
        text("name", &ns.name)?;
        json("config", &ns.config)?;
        let mut inner = self.lock();
        let taken = inner.namespaces.contains_key(&ns.namespace_id)
            || inner
                .namespaces
                .values()
                .any(|n| n.loams_namespace_id == ns.loams_namespace_id && n.name == ns.name);
        if taken {
            return Err(StoreError::NamespaceExists);
        }
        inner.namespaces.insert(ns.namespace_id.clone(), ns);
        Ok(())
    }

    async fn update_namespace(&self, ns: NamespaceRecord) -> Result<(), StoreError> {
        json("config", &ns.config)?;
        let mut inner = self.lock();
        let cur = inner
            .namespaces
            .get_mut(&ns.namespace_id)
            .ok_or(StoreError::NotFound)?;
        cur.retention_secs = ns.retention_secs;
        cur.state = ns.state;
        cur.config = ns.config;
        Ok(())
    }

    async fn namespace_by_name(
        &self,
        loams_namespace_id: &str,
        name: &str,
    ) -> Result<Option<NamespaceRecord>, StoreError> {
        Ok(self
            .lock()
            .namespaces
            .values()
            .find(|n| n.loams_namespace_id == loams_namespace_id && n.name == name)
            .cloned())
    }

    async fn namespace_by_id(
        &self,
        namespace_id: &str,
    ) -> Result<Option<NamespaceRecord>, StoreError> {
        Ok(self.lock().namespaces.get(namespace_id).cloned())
    }

    async fn list_namespaces(
        &self,
        loams_namespace_id: &str,
    ) -> Result<Vec<NamespaceRecord>, StoreError> {
        let mut out: Vec<_> = self
            .lock()
            .namespaces
            .values()
            .filter(|n| n.loams_namespace_id == loams_namespace_id)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn start(&self, s: StartExecution) -> Result<StartOutcome, StoreError> {
        check_key(&s.key)?;
        text("request_id", &s.request_id)?;
        check_commit(&s.commit, FIRST_EVENT_ID)?;
        check_strings(&s.commit)?;
        let wf = (s.key.namespace_id.clone(), s.key.workflow_id.clone());
        let has_outbox = !s.commit.outbox.is_empty();
        {
            let mut inner = self.lock();
            if let Some(cur) = inner.current.get(&wf) {
                let running = cur.status == STATUS_RUNNING;
                if running || cur.request_id == s.request_id {
                    return Ok(StartOutcome::AlreadyStarted {
                        run_id: cur.run_id.clone(),
                        request_id: cur.request_id.clone(),
                        running,
                    });
                }
            }
            if inner.executions.contains_key(&s.key) {
                return Err(StoreError::RunExists);
            }
            let status = s.commit.status;
            self.apply(&mut inner, &s.key, 1, s.commit);
            inner.current.insert(
                wf,
                CurrentRun {
                    run_id: s.key.run_id.clone(),
                    status,
                    request_id: s.request_id,
                },
            );
        }
        if has_outbox {
            self.bump();
        }
        Ok(StartOutcome::Started { version: 1 })
    }

    async fn current_run(
        &self,
        namespace_id: &str,
        workflow_id: &str,
    ) -> Result<Option<CurrentRun>, StoreError> {
        Ok(self
            .lock()
            .current
            .get(&(namespace_id.to_owned(), workflow_id.to_owned()))
            .cloned())
    }

    async fn load(&self, key: &RunKey) -> Result<Option<Loaded>, StoreError> {
        Ok(self.lock().executions.get(key).cloned())
    }

    async fn commit(
        &self,
        key: &RunKey,
        expected_version: u64,
        c: Commit,
    ) -> Result<u64, StoreError> {
        let has_outbox = !c.outbox.is_empty();
        let version = {
            let mut inner = self.lock();
            let Some(run) = inner.executions.get(key) else {
                return Err(StoreError::NotFound);
            };
            if run.version != expected_version {
                return Err(StoreError::Conflict {
                    expected: expected_version,
                    actual: run.version,
                });
            }
            check_commit(&c, run.next_event_id)?;
            check_strings(&c)?;
            #[cfg(feature = "faults")]
            if self.fault.swap(false, Ordering::SeqCst) {
                return Err(StoreError::Unavailable("injected fault".into()));
            }
            let version = expected_version + 1;
            self.apply(&mut inner, key, version, c);
            version
        };
        if has_outbox {
            self.bump();
        }
        Ok(version)
    }

    async fn history(
        &self,
        key: &RunKey,
        from_event_id: i64,
        max_events: usize,
    ) -> Result<Vec<EventRow>, StoreError> {
        let inner = self.lock();
        if !inner.executions.contains_key(key) {
            return Err(StoreError::NotFound);
        }
        let events = inner.history.get(key).map(Vec::as_slice).unwrap_or(&[]);
        // Events are stored contiguously from FIRST_EVENT_ID.
        let start = usize::try_from(from_event_id.max(FIRST_EVENT_ID) - FIRST_EVENT_ID)
            .unwrap_or(usize::MAX)
            .min(events.len());
        Ok(events[start..].iter().take(max_events).cloned().collect())
    }

    async fn claim_outbox(
        &self,
        shards: &[u32],
        limit: usize,
        lease: Duration,
        owner: &str,
    ) -> Result<Vec<OutboxRow>, StoreError> {
        text("owner", owner)?;
        let now = Instant::now();
        let until = now + lease;
        let mut inner = self.lock();
        let mut out = Vec::new();
        for o in inner.outbox.values_mut() {
            if out.len() >= limit {
                break;
            }
            let free = o.claimed_until.is_none_or(|t| t < now);
            if !o.dispatched && free && shards.contains(&o.row.shard_id) {
                o.claimed_until = Some(until);
                o.row.attempts += 1;
                out.push(o.row.clone());
            }
        }
        Ok(out)
    }

    async fn mark_dispatched(&self, ids: &[i64]) -> Result<(), StoreError> {
        let mut inner = self.lock();
        for id in ids {
            if let Some(o) = inner.outbox.get_mut(id) {
                o.dispatched = true;
            }
        }
        Ok(())
    }

    fn outbox_wakeups(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }

    async fn visibility(&self, key: &RunKey) -> Result<Option<VisibilityRow>, StoreError> {
        Ok(self.lock().visibility.get(key).cloned())
    }
}
