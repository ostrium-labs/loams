//! Job runs (§85 §11): a run is queued by a client, claimed by one runner
//! under a lease, and committed into its journal as it goes: log segments
//! (idempotent by offset, D2441), artifact references and the final state.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use loams_common::NamespaceId;
use loams_common::meta::Fence;
use loams_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::checkpoint::{CheckpointError, Checkpoints};
use crate::error::{Error, Result};
use crate::journal::{Head, Journal, Loaded};
use crate::keys;

/// The most bytes in one log append.
pub const MAX_LOG_APPEND: usize = 4 << 20;
const MAX_PARAMS: usize = 32;

/// How a run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl RunOutcome {
    fn as_str(self) -> &'static str {
        match self {
            RunOutcome::Succeeded => "succeeded",
            RunOutcome::Failed => "failed",
            RunOutcome::Cancelled => "cancelled",
            RunOutcome::TimedOut => "timed_out",
        }
    }
}

/// One committed fact in a run's journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RunEntry {
    Claimed {
        runner: String,
        at_ms: u64,
    },
    Log {
        start: u64,
        len: u64,
        sha: String,
    },
    Artifact {
        name: String,
        tid: String,
        size: u64,
    },
    Finished {
        outcome: RunOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        at_ms: u64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Spec {
    job: String,
    params: BTreeMap<String, String>,
    created_ms: u64,
}

/// An artifact a run produced: a completed transfer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactView {
    pub name: String,
    pub tid: String,
    pub size: u64,
}

/// What `GET /runs/{rid}` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunView {
    pub rid: String,
    pub job: String,
    pub params: BTreeMap<String, String>,
    /// `queued`, `running`, `succeeded`, `failed`, `cancelled` or `timed_out`.
    pub state: String,
    pub log_bytes: u64,
    pub artifacts: Vec<ArtifactView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    pub cancel_requested: bool,
    pub created_ms: u64,
}

impl RunView {
    /// Whether the run has ended.
    pub fn done(&self) -> bool {
        is_final(&self.state)
    }
}

fn is_final(st: &str) -> bool {
    matches!(st, "succeeded" | "failed" | "cancelled" | "timed_out")
}

/// A run a runner has claimed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedRun {
    pub rid: String,
    pub job: String,
    pub params: BTreeMap<String, String>,
    pub epoch: u64,
    /// Committed log bytes: where the runner's next append starts.
    pub log_bytes: u64,
}

/// A log read: the committed length and state when it was taken, and the
/// bytes from the requested offset up to that length.
pub struct LogRead {
    pub committed: u64,
    pub state: String,
    pub done: bool,
    pub bytes: BoxStream<'static, Result<Bytes>>,
}

impl std::fmt::Debug for LogRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogRead")
            .field("committed", &self.committed)
            .field("state", &self.state)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

/// The run service over a bucket and the metastore.
#[derive(Debug, Clone)]
pub struct Runs {
    store: Store,
    ck: Arc<dyn Checkpoints>,
    lease_ttl: Duration,
}

fn check_job(job: &str) -> Result<()> {
    let ok = !job.is_empty()
        && job.len() <= 64
        && job.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
        });
    if ok {
        Ok(())
    } else {
        Err(Error::BadRequest(
            "job must be 1..=64 of [a-z0-9._-]".into(),
        ))
    }
}

fn check_params(params: &BTreeMap<String, String>) -> Result<()> {
    if params.len() > MAX_PARAMS {
        return Err(Error::BadRequest(format!("at most {MAX_PARAMS} params")));
    }
    for (k, v) in params {
        let key_ok = !k.is_empty()
            && k.len() <= 64
            && k.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !key_ok || v.len() > 4096 || v.contains('\0') {
            return Err(Error::BadRequest(format!(
                "param {k:?}: names are 1..=64 of [A-Z0-9_], values at most 4096 bytes without NUL"
            )));
        }
    }
    Ok(())
}

impl Runs {
    pub fn new(store: Store, ck: Arc<dyn Checkpoints>, lease_ttl: Duration) -> Self {
        Runs {
            store,
            ck,
            lease_ttl,
        }
    }

    fn journal(&self, ns: NamespaceId, rid: &str) -> Journal {
        Journal::new(
            self.store.clone(),
            Arc::clone(&self.ck),
            ns,
            keys::run_pointer(rid),
            keys::run_journal(ns, rid),
        )
    }

    fn fence(ns: NamespaceId, rid: &str, epoch: u64) -> Fence {
        Fence {
            lease: keys::run_lease(ns, rid),
            epoch,
        }
    }

    async fn spec(&self, ns: NamespaceId, rid: &str) -> Result<Spec> {
        keys::check_id("run", rid)?;
        match self.store.get(&keys::run_spec(ns, rid)).await {
            Ok((data, _)) => Ok(serde_json::from_slice(&data)?),
            Err(StoreError::NotFound { .. }) => Err(Error::NotFound(format!("run {rid}"))),
            Err(e) => Err(e.into()),
        }
    }

    async fn loaded(&self, ns: NamespaceId, rid: &str) -> Result<Loaded<RunEntry>> {
        self.journal(ns, rid)
            .load()
            .await?
            .ok_or_else(|| Error::NotFound(format!("run {rid}")))
    }

    /// Commits under `fence` whatever `f` decides from the current journal:
    /// `Some((entries, head))` to append, `None` for nothing to do. Reloads
    /// and asks again when the pointer moved.
    async fn update<F>(
        &self,
        ns: NamespaceId,
        rid: &str,
        fence: Option<Fence>,
        mut f: F,
    ) -> Result<Loaded<RunEntry>>
    where
        F: FnMut(&Loaded<RunEntry>) -> Result<Option<(Vec<RunEntry>, Head)>>,
    {
        let journal = self.journal(ns, rid);
        for _ in 0..16 {
            let cur = self.loaded(ns, rid).await?;
            let Some((entries, head)) = f(&cur)? else {
                return Ok(cur);
            };
            if let Some(l) = journal.append(&cur, entries, head, fence.clone()).await? {
                return Ok(l);
            }
        }
        Err(Error::Meta(format!(
            "run {rid}: too many concurrent updates"
        )))
    }

    /// Queues run `rid` (a client-chosen ULID) of `job`. Idempotent for the
    /// same job and params.
    pub async fn create(
        &self,
        ns: NamespaceId,
        rid: &str,
        job: &str,
        params: BTreeMap<String, String>,
    ) -> Result<RunView> {
        keys::check_id("run", rid)?;
        check_job(job)?;
        check_params(&params)?;
        let spec = Spec {
            job: job.to_string(),
            params: params.clone(),
            created_ms: self.ck.now_ms(),
        };
        match self
            .store
            .put_if_absent(
                &keys::run_spec(ns, rid),
                Bytes::from(serde_json::to_vec(&spec)?),
            )
            .await
        {
            Ok(_) => {}
            Err(StoreError::AlreadyExists { .. }) => {
                let have = self.spec(ns, rid).await?;
                if have.job != job || have.params != params {
                    return Err(Error::conflict(
                        "run_exists",
                        format!("run {rid} exists with another job or params"),
                    ));
                }
            }
            Err(e) => return Err(e.into()),
        }
        let mut head = Head::new("queued");
        head.x.insert("job".into(), job.into());
        self.journal(ns, rid).create(&head).await?;
        self.view(ns, rid).await
    }

    /// The run's state, log length and artifacts.
    pub async fn view(&self, ns: NamespaceId, rid: &str) -> Result<RunView> {
        let spec = self.spec(ns, rid).await?;
        let cur = self.loaded(ns, rid).await?;
        let mut v = RunView {
            rid: rid.to_string(),
            job: spec.job,
            params: spec.params,
            state: cur.head.st.clone(),
            log_bytes: cur.head.bytes,
            artifacts: Vec::new(),
            exit_code: None,
            message: None,
            runner: None,
            cancel_requested: cur
                .head
                .x
                .get("cx")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            created_ms: spec.created_ms,
        };
        for e in cur.entries {
            match e {
                RunEntry::Claimed { runner, .. } => v.runner = Some(runner),
                RunEntry::Artifact { name, tid, size } => {
                    v.artifacts.push(ArtifactView { name, tid, size });
                }
                RunEntry::Finished {
                    exit_code, message, ..
                } => {
                    v.exit_code = exit_code;
                    v.message = message;
                }
                RunEntry::Log { .. } => {}
            }
        }
        Ok(v)
    }

    /// The namespace's runs, oldest first, as `(rid, state, job)`.
    pub async fn list(&self, ns: NamespaceId) -> Result<Vec<(String, String, String)>> {
        let mut out = Vec::new();
        for (key, p) in self
            .ck
            .pointers_with_prefix(ns, keys::RUN_POINTER_PREFIX)
            .await?
        {
            let head: Head = serde_json::from_str(&p.value)?;
            let job = head
                .x
                .get("job")
                .and_then(|j| j.as_str())
                .unwrap_or_default()
                .to_string();
            let rid = key
                .strip_prefix(keys::RUN_POINTER_PREFIX)
                .unwrap_or(&key)
                .to_string();
            out.push((rid, head.st, job));
        }
        Ok(out)
    }

    /// Claims the oldest queued run of one of `jobs` for `runner`, or
    /// returns `None`. A running run whose runner's lease lapsed is finished
    /// as failed ("runner lost") on the way.
    pub async fn claim(
        &self,
        ns: NamespaceId,
        runner: &str,
        jobs: &[String],
    ) -> Result<Option<ClaimedRun>> {
        for (rid, st, job) in self.list(ns).await? {
            if !jobs.contains(&job) {
                continue;
            }
            let lease_key = keys::run_lease(ns, &rid);
            if st == "running" {
                let lapsed = match self.ck.lease(&lease_key).await? {
                    Some(l) => !l.is_held_at(self.ck.now_ms()),
                    None => true,
                };
                if lapsed {
                    self.reap(ns, &rid, runner).await?;
                }
                continue;
            }
            if st != "queued" {
                continue;
            }
            let grant = match self
                .ck
                .acquire_lease(&lease_key, runner, self.lease_ttl)
                .await
            {
                Ok(g) => g,
                Err(CheckpointError::LeaseHeld { .. }) => continue,
                Err(e) => return Err(e.into()),
            };
            let now = self.ck.now_ms();
            let claimed = self
                .update(ns, &rid, Some(Self::fence(ns, &rid, grant.epoch)), |cur| {
                    if cur.head.st != "queued" {
                        return Ok(None);
                    }
                    let mut head = cur.head.clone();
                    head.st = "running".into();
                    Ok(Some((
                        vec![RunEntry::Claimed {
                            runner: runner.to_string(),
                            at_ms: now,
                        }],
                        head,
                    )))
                })
                .await?;
            let mine = claimed.head.st == "running"
                && claimed.entries.iter().rev().find_map(|e| match e {
                    RunEntry::Claimed { runner: r, .. } => Some(r.as_str()),
                    _ => None,
                }) == Some(runner);
            if !mine {
                continue;
            }
            let spec = self.spec(ns, &rid).await?;
            return Ok(Some(ClaimedRun {
                rid,
                job: spec.job,
                params: spec.params,
                epoch: grant.epoch,
                log_bytes: claimed.head.bytes,
            }));
        }
        Ok(None)
    }

    async fn reap(&self, ns: NamespaceId, rid: &str, by: &str) -> Result<()> {
        let grant = match self
            .ck
            .acquire_lease(&keys::run_lease(ns, rid), by, self.lease_ttl)
            .await
        {
            Ok(g) => g,
            Err(CheckpointError::LeaseHeld { .. }) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        self.finish(
            ns,
            rid,
            grant.epoch,
            RunOutcome::Failed,
            None,
            Some("runner lost: its lease lapsed".into()),
        )
        .await?;
        Ok(())
    }

    /// Renews the runner's lease. Returns whether a cancel was requested.
    pub async fn renew(
        &self,
        ns: NamespaceId,
        rid: &str,
        runner: &str,
        epoch: u64,
    ) -> Result<bool> {
        let grant = self
            .ck
            .acquire_lease(&keys::run_lease(ns, rid), runner, self.lease_ttl)
            .await?;
        if grant.epoch != epoch {
            return Err(Error::LeaseLost);
        }
        let (_, head) = self
            .journal(ns, rid)
            .head()
            .await?
            .ok_or_else(|| Error::NotFound(format!("run {rid}")))?;
        Ok(head.x.get("cx").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    /// Appends `data` to the run's log at `offset` (D2441). The offset must
    /// be the committed length; a resend of committed bytes at the same
    /// offset is a no-op. Returns the committed length.
    pub async fn append_log(
        &self,
        ns: NamespaceId,
        rid: &str,
        epoch: u64,
        offset: u64,
        data: Bytes,
    ) -> Result<u64> {
        if data.is_empty() || data.len() > MAX_LOG_APPEND {
            return Err(Error::BadRequest(format!(
                "a log append is 1..={MAX_LOG_APPEND} bytes"
            )));
        }
        let len = data.len() as u64;
        let sha = hex::encode(Sha256::digest(&data));
        let duplicate = |cur: &Loaded<RunEntry>| {
            cur.entries.iter().any(|e| {
                matches!(e, RunEntry::Log { start, len: l, sha: s } if *start == offset && *l == len && *s == sha)
            })
        };
        let cur = self.loaded(ns, rid).await?;
        if offset < cur.head.bytes && duplicate(&cur) {
            return Ok(cur.head.bytes);
        }
        if offset != cur.head.bytes {
            return Err(Error::Offset {
                committed: cur.head.bytes,
            });
        }
        if cur.head.st != "running" {
            return Err(Error::State(format!("run {rid} is {}", cur.head.st)));
        }
        self.store
            .put(&keys::run_log(ns, rid, offset, &sha), data)
            .await?;
        let done = self
            .update(ns, rid, Some(Self::fence(ns, rid, epoch)), |cur| {
                if offset < cur.head.bytes && duplicate(cur) {
                    return Ok(None);
                }
                if offset != cur.head.bytes {
                    return Err(Error::Offset {
                        committed: cur.head.bytes,
                    });
                }
                if cur.head.st != "running" {
                    return Err(Error::State(format!("run {rid} is {}", cur.head.st)));
                }
                let mut head = cur.head.clone();
                head.bytes += len;
                Ok(Some((
                    vec![RunEntry::Log {
                        start: offset,
                        len,
                        sha: sha.clone(),
                    }],
                    head,
                )))
            })
            .await?;
        Ok(done.head.bytes)
    }

    /// Records an artifact (a transfer the runner uploaded).
    pub async fn add_artifact(
        &self,
        ns: NamespaceId,
        rid: &str,
        epoch: u64,
        name: &str,
        tid: &str,
        size: u64,
    ) -> Result<()> {
        keys::check_id("transfer", tid)?;
        if name.is_empty() || name.len() > 1024 || name.chars().any(char::is_control) {
            return Err(Error::BadRequest(
                "artifact name must be 1..=1024 bytes".into(),
            ));
        }
        self.update(ns, rid, Some(Self::fence(ns, rid, epoch)), |cur| {
            if cur.head.st != "running" {
                return Err(Error::State(format!("run {rid} is {}", cur.head.st)));
            }
            let dup = cur.entries.iter().any(
                |e| matches!(e, RunEntry::Artifact { name: n, tid: t, .. } if n == name && t == tid),
            );
            if dup {
                return Ok(None);
            }
            Ok(Some((
                vec![RunEntry::Artifact {
                    name: name.to_string(),
                    tid: tid.to_string(),
                    size,
                }],
                cur.head.clone(),
            )))
        })
        .await?;
        Ok(())
    }

    /// Records how the run ended. Idempotent for the same outcome.
    pub async fn finish(
        &self,
        ns: NamespaceId,
        rid: &str,
        epoch: u64,
        outcome: RunOutcome,
        exit_code: Option<i32>,
        message: Option<String>,
    ) -> Result<RunView> {
        let now = self.ck.now_ms();
        self.update(ns, rid, Some(Self::fence(ns, rid, epoch)), |cur| {
            if cur.head.st == outcome.as_str() {
                return Ok(None);
            }
            if cur.head.st != "running" {
                return Err(Error::State(format!("run {rid} is {}", cur.head.st)));
            }
            let mut head = cur.head.clone();
            head.st = outcome.as_str().into();
            if let Some(c) = exit_code {
                head.x.insert("exit".into(), c.into());
            }
            Ok(Some((
                vec![RunEntry::Finished {
                    outcome,
                    exit_code,
                    message: message.clone(),
                    at_ms: now,
                }],
                head,
            )))
        })
        .await?;
        self.view(ns, rid).await
    }

    /// Cancels a run: a queued run ends at once; a running run gets the
    /// cancel flag, which its runner reads when it renews.
    pub async fn cancel(&self, ns: NamespaceId, rid: &str) -> Result<RunView> {
        let journal = self.journal(ns, rid);
        for _ in 0..16 {
            let Some((version, mut head)) = journal.head().await? else {
                return Err(Error::NotFound(format!("run {rid}")));
            };
            if is_final(&head.st) {
                break;
            }
            if head.st == "queued" {
                head.st = RunOutcome::Cancelled.as_str().into();
            } else {
                head.x.insert("cx".into(), true.into());
            }
            if journal.set_head(version, &head, None).await?.is_some() {
                break;
            }
        }
        self.view(ns, rid).await
    }

    /// The log from `from` up to the committed length at the time of the
    /// call, streamed segment by segment from the bucket.
    pub async fn read_log(&self, ns: NamespaceId, rid: &str, from: u64) -> Result<LogRead> {
        let cur = self.loaded(ns, rid).await?;
        if from > cur.head.bytes {
            return Err(Error::Offset {
                committed: cur.head.bytes,
            });
        }
        let mut parts = Vec::new();
        for e in &cur.entries {
            if let RunEntry::Log { start, len, sha } = e
                && start + len > from
            {
                let skip = from.saturating_sub(*start);
                parts.push((keys::run_log(ns, rid, *start, sha), skip, *len));
            }
        }
        let store = self.store.clone();
        let bytes = stream::iter(parts)
            .then(move |(key, skip, len)| {
                let store = store.clone();
                async move { Ok(store.get_range(&key, skip..len).await?) }
            })
            .boxed();
        Ok(LogRead {
            committed: cur.head.bytes,
            done: is_final(&cur.head.st),
            state: cur.head.st,
            bytes,
        })
    }
}
