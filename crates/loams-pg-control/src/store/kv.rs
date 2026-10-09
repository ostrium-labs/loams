//! [`KvControlStore`]: the one implementation of [`PgControlStore`], on
//! `loams-kv`'s store seam, so the embedded (redb) and the TiKV backend run
//! the same code (ruling R3.1).
//!
//! Every call is one optimistic transaction of `loams-kv`'s runner, which
//! retries conflicts. A compare-and-set reads the record's version and
//! answers its own refusal (`Conflict`, `NotFound`, `Fenced`) without
//! writing. Writes carry a commit token unless [`StoreOptions`] says not.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use loams_kv::{Store, Txn, TxnError, TxnOptions};
use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_PAGE_SIZE, Fence, LEASE_SCOPE, MAX_LEASE_TTL, MAX_PAGE_SIZE, Page, PgControlStore,
    StoreError, StoreEvent, Versioned,
};
use crate::model::{FORMAT, Record, TAGS, project_lease};

/// How often a watch rescans its prefix when this handle has not written.
pub const DEFAULT_POLL: Duration = Duration::from_millis(250);

pub(crate) const OP_GET: &str = "pg.get";
pub(crate) const OP_PUT: &str = "pg.put";
pub(crate) const OP_DELETE: &str = "pg.delete";
pub(crate) const OP_LIST: &str = "pg.list";
pub(crate) const OP_LEASE: &str = "pg.lease";
pub(crate) const OP_WATCH: &str = "pg.watch";

/// Keys a watch scans per request.
const WATCH_SCAN: usize = 1000;

/// How a [`KvControlStore`] runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreOptions {
    /// How often a watch rescans its prefix (it also rescans at once after
    /// this handle's own writes). Default [`DEFAULT_POLL`].
    pub poll: Duration,
    /// Write a commit token with every write, so a lost acknowledgement is
    /// resolved instead of surfacing as `Undetermined` (default `true`; the
    /// conformance suite turns it off to surface one).
    pub commit_tokens: bool,
}

impl Default for StoreOptions {
    fn default() -> Self {
        StoreOptions {
            poll: DEFAULT_POLL,
            commit_tokens: true,
        }
    }
}

/// `PgControlStore` on a `loams-kv` store. Cheap to clone; clones share the
/// change signal that wakes this handle's watches.
#[derive(Debug, Clone)]
pub struct KvControlStore {
    store: Store,
    options: StoreOptions,
    changes: Arc<tokio::sync::watch::Sender<u64>>,
}

/// Unfenced writes for the API service, which holds no project lease (Task 5
/// writes a project or branch in state `creating`, and the reconciler acts
/// on it under its lease). Every write is still a compare-and-set. Only
/// [`KvControlStore::api_writer`] makes one (R3.10).
#[derive(Debug, Clone)]
pub struct ApiWriter {
    store: KvControlStore,
}

impl ApiWriter {
    /// `put` with no fence.
    ///
    /// # Errors
    ///
    /// As [`PgControlStore::put`], but never `Fenced`.
    pub async fn put<R: Record>(&self, rec: &R, expected: Option<u64>) -> Result<u64, StoreError> {
        self.store.put_record(rec, expected, None).await
    }

    /// `delete` with no fence.
    ///
    /// # Errors
    ///
    /// As [`PgControlStore::delete`], but never `Fenced`.
    pub async fn delete<R: Record>(&self, key: &R::Key, expected: u64) -> Result<(), StoreError> {
        self.store.delete_record::<R>(key, expected, None).await
    }

    /// The store, for reads.
    pub fn store(&self) -> &KvControlStore {
        &self.store
    }
}

/// The lease record: the metastore's `Lease` field for field, so its
/// encoding (`FORMAT ‖ postcard`) is the one `loams-meta-tikv` writes at
/// `e/m/` (R3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LeaseRec {
    epoch: u64,
    /// `None` once released.
    owner: Option<String>,
    deadline_ms: u64,
}

impl KvControlStore {
    /// The control store on `store` (a fresh handle: its own change signal).
    pub fn new(store: Store, options: StoreOptions) -> Self {
        let (changes, _) = tokio::sync::watch::channel(0);
        KvControlStore {
            store,
            options,
            changes: Arc::new(changes),
        }
    }

    /// The `loams-kv` store underneath.
    pub fn kv(&self) -> &Store {
        &self.store
    }

    /// The API service's writer: compare-and-set writes with no lease
    /// (R3.10). An inherent method, not on [`PgControlStore`], so code
    /// generic over the trait (the reconcilers) cannot reach it.
    pub fn api_writer(&self) -> ApiWriter {
        ApiWriter {
            store: self.clone(),
        }
    }

    /// The options this handle runs with.
    pub fn options(&self) -> &StoreOptions {
        &self.options
    }

    /// Runs a read-only body.
    async fn read<T: Send, F>(&self, op: &'static str, body: F) -> Result<T, StoreError>
    where
        F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<Result<T, StoreError>, TxnError>>,
    {
        self.store
            .run(TxnOptions::new(op), body)
            .await
            .map_err(txn_error)?
            .value
    }

    /// Runs a writing body (with a commit token unless the options say
    /// not), and wakes this handle's watches after a write that succeeded.
    async fn write<T: Send, F>(&self, op: &'static str, body: F) -> Result<T, StoreError>
    where
        F: for<'t> FnMut(&'t mut Txn) -> BoxFuture<'t, Result<Result<T, StoreError>, TxnError>>,
    {
        let mut opts = TxnOptions::new(op);
        opts.commit_token = self.options.commit_tokens;
        let out = self.store.run(opts, body).await.map_err(txn_error)?.value;
        if out.is_ok() {
            self.changes.send_modify(|n| *n = n.wrapping_add(1));
        }
        out
    }

    /// `put`, fenced by `fence` or (only for [`ApiWriter`]) by nothing.
    async fn put_record<R: Record>(
        &self,
        rec: &R,
        expected: Option<u64>,
        fence: Option<&Fence>,
    ) -> Result<u64, StoreError> {
        let key = R::encode_key(&rec.key())?;
        let body = postcard::to_stdvec(rec)
            .map_err(|e| StoreError::InvalidArgument(format!("{} encodes: {e}", R::KIND)))?;
        if let Some(fence) = fence {
            check_scope(fence.scope())?;
        }
        check_project(fence, rec)?;
        let fence = fence.cloned();
        self.write(OP_PUT, move |txn| {
            let (key, body, fence) = (key.clone(), body.clone(), fence.clone());
            Box::pin(async move {
                if let Err(e) = check_fence(txn, fence.as_ref()).await? {
                    return Ok(Err(e));
                }
                let current = match txn.get(&key).await? {
                    Some(value) => match decode_record::<R>(&value) {
                        Ok(stored) => {
                            if let Err(e) = check_project(fence.as_ref(), &stored.record) {
                                return Ok(Err(e));
                            }
                            Some(stored.version)
                        }
                        Err(e) => return Ok(Err(e)),
                    },
                    None => None,
                };
                if current != expected {
                    return Ok(Err(StoreError::Conflict { current }));
                }
                // The start timestamp is above every committed write of the
                // key under snapshot isolation; `max` only guards that.
                let version = txn
                    .start_ts()
                    .0
                    .max(current.map_or(0, |c| c.saturating_add(1)));
                txn.put(&key, encode_value(version, &body)).await?;
                Ok(Ok(version))
            })
        })
        .await
    }

    /// `delete`, fenced by `fence` or (only for [`ApiWriter`]) by nothing.
    async fn delete_record<R: Record>(
        &self,
        key: &R::Key,
        expected: u64,
        fence: Option<&Fence>,
    ) -> Result<(), StoreError> {
        let key = R::encode_key(key)?;
        if let Some(fence) = fence {
            check_scope(fence.scope())?;
        }
        let fence = fence.cloned();
        self.write(OP_DELETE, move |txn| {
            let (key, fence) = (key.clone(), fence.clone());
            Box::pin(async move {
                if let Err(e) = check_fence(txn, fence.as_ref()).await? {
                    return Ok(Err(e));
                }
                let Some(value) = txn.get(&key).await? else {
                    return Ok(Err(StoreError::NotFound));
                };
                let current = match decode_record::<R>(&value) {
                    Ok(stored) => {
                        if let Err(e) = check_project(fence.as_ref(), &stored.record) {
                            return Ok(Err(e));
                        }
                        stored.version
                    }
                    Err(e) => return Ok(Err(e)),
                };
                if current != expected {
                    return Ok(Err(StoreError::Conflict {
                        current: Some(current),
                    }));
                }
                txn.delete(&key).await?;
                Ok(Ok(()))
            })
        })
        .await
    }

    /// The version of every record under `prefix`, in one snapshot.
    /// Undecodable values are returned apart, with why, for the watch to
    /// report once.
    async fn versions(&self, prefix: &[u8]) -> Result<Scan, StoreError> {
        let prefix = prefix.to_vec();
        self.read(OP_WATCH, move |txn| {
            let prefix = prefix.clone();
            Box::pin(async move {
                let end = prefix_end(&prefix);
                let mut out = Scan::default();
                let mut start = prefix.clone();
                loop {
                    let pairs = txn.scan(&start, end.as_deref(), WATCH_SCAN).await?;
                    let full = pairs.len() == WATCH_SCAN;
                    if let Some((last, _)) = pairs.last() {
                        start = after(last);
                    }
                    for (key, value) in pairs {
                        match decode_version(&value) {
                            Ok((version, _)) => {
                                out.versions.insert(key, version);
                            }
                            Err(e) => {
                                out.corrupt.insert(key, e.to_string());
                            }
                        }
                    }
                    if !full {
                        return Ok(Ok(out));
                    }
                }
            })
        })
        .await
    }
}

impl PgControlStore for KvControlStore {
    async fn get<R: Record>(&self, key: &R::Key) -> Result<Option<Versioned<R>>, StoreError> {
        let key = R::encode_key(key)?;
        self.read(OP_GET, move |txn| {
            let key = key.clone();
            Box::pin(async move {
                Ok(match txn.get(&key).await? {
                    Some(value) => decode_record::<R>(&value).map(Some),
                    None => Ok(None),
                })
            })
        })
        .await
    }

    async fn put<R: Record>(
        &self,
        rec: &R,
        expected: Option<u64>,
        fence: &Fence,
    ) -> Result<u64, StoreError> {
        self.put_record(rec, expected, Some(fence)).await
    }

    async fn delete<R: Record>(
        &self,
        key: &R::Key,
        expected: u64,
        fence: &Fence,
    ) -> Result<(), StoreError> {
        self.delete_record::<R>(key, expected, Some(fence)).await
    }

    async fn list<R: Record>(
        &self,
        prefix: &R::Prefix,
        page: Page,
    ) -> Result<(Vec<Versioned<R>>, Option<String>), StoreError> {
        let prefix = R::encode_prefix(prefix)?;
        let start = match &page.token {
            None => prefix.clone(),
            Some(token) => {
                let last = hex::decode(token)
                    .map_err(|_| StoreError::InvalidArgument("unreadable page token".into()))?;
                if !last.starts_with(&prefix) || last.len() == prefix.len() {
                    return Err(StoreError::InvalidArgument(
                        "the page token belongs to another listing".into(),
                    ));
                }
                after(&last)
            }
        };
        let size = match page.size {
            0 => DEFAULT_PAGE_SIZE,
            n => n.min(MAX_PAGE_SIZE),
        } as usize;
        let end = prefix_end(&prefix);
        self.read(OP_LIST, move |txn| {
            let (start, end) = (start.clone(), end.clone());
            Box::pin(async move {
                let mut pairs = txn.scan(&start, end.as_deref(), size + 1).await?;
                let more = pairs.len() > size;
                pairs.truncate(size);
                let next = more
                    .then(|| pairs.last().map(|(k, _)| hex::encode(k)))
                    .flatten();
                let records: Result<Vec<_>, _> =
                    pairs.iter().map(|(_, v)| decode_record::<R>(v)).collect();
                Ok(records.map(|r| (r, next)))
            })
        })
        .await
    }

    async fn acquire_lease(
        &self,
        scope: &str,
        holder: &str,
        ttl: Duration,
    ) -> Result<Fence, StoreError> {
        check_scope(scope)?;
        if holder.is_empty() {
            return Err(StoreError::InvalidArgument(
                "a lease holder is empty".into(),
            ));
        }
        let ttl_ms = check_ttl(ttl)?;
        let (scope, holder) = (scope.to_string(), holder.to_string());
        self.write(OP_LEASE, move |txn| {
            let (scope, holder) = (scope.clone(), holder.clone());
            Box::pin(async move {
                // Deadlines are judged against the transaction's start
                // timestamp: the store's clock (PD's TSO on TiKV).
                let now = txn.start_ts().physical_ms();
                let current = match txn.get(scope.as_bytes()).await? {
                    Some(v) => match decode_lease(&v) {
                        Ok(lease) => Some(lease),
                        Err(e) => return Ok(Err(e)),
                    },
                    None => None,
                };
                let epoch = match current {
                    None => 1,
                    Some(lease) if lease.owner.is_some() && now < lease.deadline_ms => {
                        if lease.owner.as_deref() != Some(holder.as_str()) {
                            return Ok(Err(StoreError::Held {
                                holder: lease.owner.unwrap_or_default(),
                                deadline_ms: lease.deadline_ms,
                            }));
                        }
                        lease.epoch
                    }
                    Some(lease) => lease.epoch + 1,
                };
                let holder_out = holder.clone();
                let lease = LeaseRec {
                    epoch,
                    owner: Some(holder),
                    deadline_ms: now.saturating_add(ttl_ms),
                };
                txn.put(scope.as_bytes(), encode_lease(&lease)).await?;
                Ok(Ok(Fence::new(scope, holder_out, epoch)))
            })
        })
        .await
    }

    async fn renew_lease(&self, fence: &Fence, ttl: Duration) -> Result<Fence, StoreError> {
        check_scope(fence.scope())?;
        let ttl_ms = check_ttl(ttl)?;
        let fence = fence.clone();
        self.write(OP_LEASE, move |txn| {
            let fence = fence.clone();
            Box::pin(async move {
                let now = txn.start_ts().physical_ms();
                let key = fence.scope().as_bytes();
                let current = match txn.get(key).await? {
                    Some(v) => match decode_lease(&v) {
                        Ok(lease) => lease,
                        Err(e) => return Ok(Err(e)),
                    },
                    None => return Ok(Err(StoreError::LeaseLost)),
                };
                if current.epoch != fence.epoch()
                    || current.owner.as_deref() != Some(fence.holder())
                    || now >= current.deadline_ms
                {
                    return Ok(Err(StoreError::LeaseLost));
                }
                let lease = LeaseRec {
                    deadline_ms: now.saturating_add(ttl_ms),
                    ..current
                };
                txn.put(key, encode_lease(&lease)).await?;
                Ok(Ok(fence))
            })
        })
        .await
    }

    fn watch(&self, prefix: &[u8]) -> Result<BoxStream<'static, StoreEvent>, StoreError> {
        check_watch_prefix(prefix)?;
        let mut changes = self.changes.subscribe();
        changes.mark_unchanged();
        let state = Watch {
            store: self.clone(),
            prefix: prefix.to_vec(),
            known: None,
            queue: VecDeque::new(),
            changes,
            warned: BTreeSet::new(),
        };
        Ok(Box::pin(futures::stream::unfold(
            state,
            |mut w| async move {
                let event = w.next().await;
                Some((event, w))
            },
        )))
    }
}

/// A watch's state: what it last saw, and the events not yet sent.
struct Watch {
    store: KvControlStore,
    prefix: Vec<u8>,
    known: Option<BTreeMap<Vec<u8>, u64>>,
    queue: VecDeque<StoreEvent>,
    changes: tokio::sync::watch::Receiver<u64>,
    /// The undecodable keys already reported, so a rescan does not report
    /// them again.
    warned: BTreeSet<Vec<u8>>,
}

/// One scan of a watch's prefix.
#[derive(Debug, Default)]
struct Scan {
    versions: BTreeMap<Vec<u8>, u64>,
    corrupt: BTreeMap<Vec<u8>, String>,
}

impl Watch {
    async fn next(&mut self) -> StoreEvent {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return event;
            }
            if self.known.is_some() {
                self.wait().await;
            }
            match self.store.versions(&self.prefix).await {
                Ok(scan) => {
                    self.report(&scan.corrupt);
                    self.diff(scan.versions);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "pg-control watch rescan failed");
                    if self.known.is_none() {
                        self.wait().await;
                    }
                }
            }
        }
    }

    async fn wait(&mut self) {
        let poll = self.store.options.poll;
        tokio::select! {
            changed = self.changes.changed() => {
                if changed.is_err() {
                    tokio::time::sleep(poll).await;
                }
            }
            () = tokio::time::sleep(poll) => {}
        }
    }

    /// Warns once per undecodable key (again only after it decoded or went
    /// away in between).
    fn report(&mut self, corrupt: &BTreeMap<Vec<u8>, String>) {
        self.warned.retain(|key| corrupt.contains_key(key));
        for (key, why) in corrupt {
            if self.warned.insert(key.clone()) {
                tracing::warn!(
                    key = %String::from_utf8_lossy(key),
                    error = %why,
                    "pg-control watch skips an undecodable record"
                );
            }
        }
    }

    fn diff(&mut self, now: BTreeMap<Vec<u8>, u64>) {
        let first = self.known.is_none();
        let known = self.known.take().unwrap_or_default();
        for (key, &version) in &now {
            if known.get(key) != Some(&version) {
                self.queue.push_back(StoreEvent::Put {
                    key: key.clone(),
                    version,
                });
            }
        }
        for key in known.keys() {
            if !now.contains_key(key) {
                self.queue
                    .push_back(StoreEvent::Delete { key: key.clone() });
            }
        }
        if first {
            self.queue.push_back(StoreEvent::Synced);
        }
        self.known = Some(now);
    }
}

/// Inside `txn`: the fence's lease is still at its epoch and held, and the
/// lease record is locked, so a takeover committed meanwhile conflicts with
/// this write. `loams-meta-tikv`'s `check_fence`, on `loams-kv` (R3.2):
/// `get` and `lock_keys` are an optimistic `get_for_update`.
async fn check_fence(
    txn: &mut Txn,
    fence: Option<&Fence>,
) -> Result<Result<(), StoreError>, TxnError> {
    let Some(fence) = fence else {
        return Ok(Ok(()));
    };
    let key = fence.scope().as_bytes();
    let lease = txn.get(key).await?;
    txn.lock_keys([key]).await?;
    Ok(match lease.as_deref().map(decode_lease).transpose() {
        Err(e) => Err(e),
        Ok(Some(lease)) if lease.epoch == fence.epoch() && lease.owner.is_some() => Ok(()),
        Ok(_) => Err(StoreError::Fenced),
    })
}

/// A watch scans only `pg-control`'s own keys: `<tag>/…` for a tag of
/// [`TAGS`], never the metastore's records beside them (R3.13).
fn check_watch_prefix(prefix: &[u8]) -> Result<(), StoreError> {
    match prefix {
        [tag, b'/', ..] if TAGS.contains(tag) => Ok(()),
        _ => Err(StoreError::InvalidArgument(
            "a watch prefix starts with one of pg-control's tags (x/ X/ E/ C/ R/ D/)".into(),
        )),
    }
}

/// A TTL in milliseconds: above zero, at most [`MAX_LEASE_TTL`].
fn check_ttl(ttl: Duration) -> Result<u64, StoreError> {
    if ttl.is_zero() || ttl > MAX_LEASE_TTL {
        return Err(StoreError::InvalidArgument(format!(
            "a lease ttl is above zero and at most {} s",
            MAX_LEASE_TTL.as_secs()
        )));
    }
    Ok(u64::try_from(ttl.as_millis()).unwrap_or(u64::MAX))
}

/// A fenced write of `rec` needs its project's lease (R3.11): a fence for
/// prj-A never writes a record of prj-B.
fn check_project<R: Record>(fence: Option<&Fence>, rec: &R) -> Result<(), StoreError> {
    match (fence, rec.project()) {
        (Some(fence), Some(project)) if fence.scope() != project_lease(project) => {
            Err(StoreError::InvalidArgument(format!(
                "the fence {} does not cover a {} of {project}",
                fence.scope(),
                R::KIND
            )))
        }
        _ => Ok(()),
    }
}

/// `e/pg/<id>`, with one part after the scope.
fn check_scope(scope: &str) -> Result<(), StoreError> {
    match scope.strip_prefix(LEASE_SCOPE) {
        Some(rest) if !rest.is_empty() && !rest.contains('/') => Ok(()),
        _ => Err(StoreError::InvalidArgument(format!(
            "a lease scope is {LEASE_SCOPE}<id>"
        ))),
    }
}

fn txn_error(e: TxnError) -> StoreError {
    match e {
        TxnError::Undetermined { .. } => StoreError::Undetermined,
        other => StoreError::Unavailable(other.to_string()),
    }
}

/// `FORMAT ‖ varint(version) ‖ body`.
fn encode_value(version: u64, body: &[u8]) -> Vec<u8> {
    let mut out = match postcard::to_extend(&version, vec![FORMAT]) {
        Ok(out) => out,
        Err(e) => unreachable!("postcard refused a u64: {e}"),
    };
    out.extend_from_slice(body);
    out
}

/// The version and the record bytes of a stored value.
fn decode_version(value: &[u8]) -> Result<(u64, &[u8]), StoreError> {
    match value.split_first() {
        Some((&FORMAT, rest)) => postcard::take_from_bytes::<u64>(rest)
            .map_err(|e| StoreError::Corrupt(format!("a version does not decode: {e}"))),
        Some((format, _)) => Err(StoreError::Corrupt(format!("unknown format {format}"))),
        None => Err(StoreError::Corrupt("empty value".into())),
    }
}

fn decode_record<R: Record>(value: &[u8]) -> Result<Versioned<R>, StoreError> {
    let (version, body) = decode_version(value)?;
    let record = postcard::from_bytes(body)
        .map_err(|e| StoreError::Corrupt(format!("a {} does not decode: {e}", R::KIND)))?;
    Ok(Versioned { record, version })
}

fn encode_lease(lease: &LeaseRec) -> Vec<u8> {
    match postcard::to_extend(lease, vec![FORMAT]) {
        Ok(out) => out,
        Err(e) => unreachable!("postcard refused a lease: {e}"),
    }
}

fn decode_lease(value: &[u8]) -> Result<LeaseRec, StoreError> {
    match value.split_first() {
        Some((&FORMAT, rest)) => postcard::from_bytes(rest)
            .map_err(|e| StoreError::Corrupt(format!("a lease does not decode: {e}"))),
        _ => Err(StoreError::Corrupt("a lease has an unknown format".into())),
    }
}

/// The smallest key after `key`.
fn after(key: &[u8]) -> Vec<u8> {
    let mut next = key.to_vec();
    next.push(0);
    next
}

/// The first key past every key starting with `prefix`, or `None`.
fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < u8::MAX {
            end.push(last + 1);
            return Some(end);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_carry_their_version() {
        let v = encode_value(300, b"body");
        assert_eq!(v[0], FORMAT);
        assert_eq!(decode_version(&v), Ok((300, b"body".as_slice())));
        assert!(matches!(
            decode_version(&[9, 1]),
            Err(StoreError::Corrupt(_))
        ));
        assert!(matches!(decode_version(&[]), Err(StoreError::Corrupt(_))));
    }

    /// The lease record is the metastore's `Lease`, byte for byte (its
    /// fields, in order: epoch, owner, deadline_ms).
    #[test]
    fn lease_encoding_is_the_metastores() {
        let lease = LeaseRec {
            epoch: 2,
            owner: Some("a".into()),
            deadline_ms: 5,
        };
        assert_eq!(encode_lease(&lease), [FORMAT, 2, 1, 1, b'a', 5]);
        assert_eq!(decode_lease(&encode_lease(&lease)), Ok(lease));
    }

    #[test]
    fn prefix_end_skips_ff() {
        assert_eq!(prefix_end(b"X/a"), Some(b"X/b".to_vec()));
        assert_eq!(prefix_end(&[1, 0xff]), Some(vec![2]));
        assert_eq!(prefix_end(&[0xff]), None);
    }

    #[test]
    fn watch_prefixes_are_pg_controls_own() {
        for ok in [
            b"x/".as_slice(),
            b"X/prj-1/",
            b"E/",
            b"C/",
            b"R/br-1/",
            b"D/",
        ] {
            assert!(check_watch_prefix(ok).is_ok(), "{ok:?}");
        }
        for bad in [b"".as_slice(), b"X", b"N/", b"e/pg/", b"t/", b"x"] {
            assert!(check_watch_prefix(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn scopes_are_checked() {
        assert!(check_scope("e/pg/prj-1").is_ok());
        for bad in ["e/pg/", "e/m/x", "e/pg/a/b", ""] {
            assert!(check_scope(bad).is_err(), "{bad}");
        }
    }
}
