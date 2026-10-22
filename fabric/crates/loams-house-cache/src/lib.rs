//! `house-cache`: the House front's S3 read proxy (HS1 Task 9, design §49 §7,
//! D766).
//!
//! A sealed worker reaches the bucket only through its forwarder (HS1 R6.2):
//! every connection chDB opens to `127.0.0.1:9180` inside the worker's network
//! namespace arrives here as one end of a socket pair, handed over by the front's
//! forwarder thread for that worker ([`loams_house::ForwardTarget`]). So each
//! connection is known to be a given worker's before a byte of it is read, and a
//! worker cannot claim to be another: there is nothing in a request that names
//! the worker.
//!
//! What a worker may read is its [`Grant`], set when the pool binds it to a
//! namespace: keys under `ns/<id>/` of the namespace's store (plus, from Task 10,
//! the external tables' locations the catalog vouches for). Anything else is
//! `403 AccessDenied`, counted per namespace. `ListObjectsV2` is refused unless
//! the grant allows it (HS1 R1.2: pinned Iceberg reads never list).
//!
//! Reads go through `loams-cache`'s [`loams_cache::RangeCache`]: fixed blocks,
//! crc32c checked on every hit (a block that fails is evicted, re-fetched and
//! never served), RAM then optionally NVMe. Objects are immutable, so nothing is
//! invalidated, and a block one worker fetched serves the next worker of any
//! namespace that may read it. Upstream requests are signed by the store with the
//! front's credentials ([`StoreOptions`] or the front's environment), which never
//! reach a worker: a worker's requests carry chDB's dummy keys, which are never
//! checked, and nothing it sends is forwarded upstream.

mod s3;
mod upstream;

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::os::unix::net::UnixStream as StdUnixStream;
use std::sync::{Arc, Mutex, MutexGuard};

use hyper_util::rt::TokioIo;
use loams_cache::CacheStats;
use loams_house::{ForwardSlot, ForwardTarget};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

pub use upstream::{
    MAX_REMEMBERED_HEADS, OneStore, StoreOptions, StoreResolver, Upstream, UpstreamError,
    bucket_and_prefix,
};

/// The most request heads [`HouseCache::record_requests`] keeps.
pub const MAX_RECORDED_REQUESTS: usize = 1024;

/// What one worker may read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    /// The namespace the worker is bound to.
    pub namespace: String,
    /// Store-relative key prefixes it may read, each ending in `/`.
    pub prefixes: Vec<String>,
    /// Whether it may list (`ListObjectsV2`) under those prefixes.
    pub list: bool,
}

impl Grant {
    /// A bound worker's grant: its namespace's own prefix, `ns/<id>/`, no listing.
    pub fn for_namespace(namespace: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            prefixes: vec![format!("ns/{namespace}/")],
            list: false,
        }
    }

    /// Whether the store-relative `key` is under one of the prefixes.
    pub fn allows(&self, key: &str) -> bool {
        self.prefixes
            .iter()
            .any(|prefix| key.starts_with(prefix.as_str()))
    }
}

/// Counters for one namespace (§49 §7's metrics; Task 23 exports them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NamespaceMetrics {
    /// Requests answered.
    pub requests: u64,
    /// Requests refused for the key, bucket or method (`403`).
    pub refused: u64,
    /// Body bytes served.
    pub bytes_served: u64,
    /// Requests for objects that do not exist (`404`).
    pub not_found: u64,
    /// Requests the store or the cache failed (`500`/`503`).
    pub upstream_errors: u64,
}

/// A snapshot of the proxy's counters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    /// Per namespace.
    pub namespaces: HashMap<String, NamespaceMetrics>,
    /// Requests from a worker with no grant (not bound yet, or gone).
    pub refused_unbound: u64,
}

/// One request head as a worker sent it ([`HouseCache::record_requests`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenRequest {
    /// The worker it came from.
    pub worker: String,
    /// `GET`, `HEAD`, …
    pub method: String,
    /// Path and query.
    pub target: String,
    /// Every header, in order.
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Default)]
struct WorkerEntry {
    grant: Option<Grant>,
    /// The private endpoint of a worker without a forwarder.
    endpoint: Option<JoinHandle<()>>,
}

#[derive(Debug)]
struct Inner {
    resolver: Arc<dyn StoreResolver>,
    runtime: Handle,
    workers: Mutex<HashMap<String, WorkerEntry>>,
    metrics: Mutex<Metrics>,
    recorded: Mutex<Option<Vec<SeenRequest>>>,
    /// Prefixes the catalog vouched for, per namespace (external tables'
    /// locations, HS1 Task 10, R9.5): added to every grant of that namespace.
    vouched: Mutex<HashMap<String, Vec<String>>>,
    /// The front's tracer: each read is a span under its worker's (HS1 Task
    /// 23).
    tracer: Mutex<Arc<loams_house::trace::Tracer>>,
}

/// The proxy. Cheap to clone; every clone is the same proxy.
#[derive(Clone, Debug)]
pub struct HouseCache {
    inner: Arc<Inner>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl HouseCache {
    /// A proxy serving each namespace from `resolver`'s upstream. Must be called
    /// on a tokio runtime, which serves every connection.
    pub fn new(resolver: Arc<dyn StoreResolver>) -> Self {
        Self {
            inner: Arc::new(Inner {
                resolver,
                runtime: Handle::current(),
                workers: Mutex::new(HashMap::new()),
                metrics: Mutex::new(Metrics::default()),
                recorded: Mutex::new(None),
                vouched: Mutex::new(HashMap::new()),
                tracer: Mutex::new(loams_house::trace::Tracer::off()),
            }),
        }
    }

    /// Puts each worker's reads under the span of what it runs, in `tracer`'s
    /// traces (the front's, HS1 Task 23).
    pub fn set_tracer(&self, tracer: Arc<loams_house::trace::Tracer>) {
        *lock(&self.inner.tracer) = tracer;
    }

    /// Its tracer.
    pub fn tracer(&self) -> Arc<loams_house::trace::Tracer> {
        Arc::clone(&lock(&self.inner.tracer))
    }

    /// Sets what `worker` may read (the pool's `Bind`, or a test).
    pub fn grant(&self, worker: &str, grant: Grant) {
        lock(&self.inner.workers)
            .entry(worker.to_string())
            .or_default()
            .grant = Some(grant);
    }

    /// Forgets `worker`: its grant and its private endpoint. Its open
    /// connections are refused from their next request.
    pub fn revoke(&self, worker: &str) {
        if let Some(entry) = lock(&self.inner.workers).remove(worker)
            && let Some(listener) = entry.endpoint
        {
            listener.abort();
        }
    }

    /// Lets namespace `namespace`'s workers read under `prefix` too (store-relative,
    /// ending in `/`): what the catalog vouches for an external table's location
    /// (HS1 Task 10, R9.5). Workers bound now get it at once; workers bound later
    /// get it with their namespace's own prefix. A prefix that is not a whole
    /// directory (`/`, empty, `..`), or lies in the namespaces' `ns/` tree, is
    /// ignored.
    pub fn vouch(&self, namespace: &str, prefix: &str) {
        // `ns/…` is the namespaces' own tree: a namespace gets its part of it only
        // through `Grant::for_namespace`, never by a vouch (PR #448 review).
        let sound = prefix.ends_with('/')
            && prefix.len() > 1
            && !prefix.starts_with('/')
            && !prefix.starts_with("ns/")
            && prefix
                .trim_end_matches('/')
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if !sound {
            return;
        }
        {
            let mut vouched = lock(&self.inner.vouched);
            let prefixes = vouched.entry(namespace.to_string()).or_default();
            if prefixes.iter().any(|p| p == prefix) {
                return;
            }
            prefixes.push(prefix.to_string());
        }
        for entry in lock(&self.inner.workers).values_mut() {
            if let Some(grant) = entry.grant.as_mut()
                && grant.namespace == namespace
                && !grant.prefixes.iter().any(|p| p == prefix)
            {
                grant.prefixes.push(prefix.to_string());
            }
        }
    }

    /// A newly bound worker's grant: its namespace's prefix and what was vouched.
    fn grant_for(&self, namespace: &str) -> Grant {
        let mut grant = Grant::for_namespace(namespace);
        if let Some(extra) = lock(&self.inner.vouched).get(namespace) {
            grant.prefixes.extend(extra.iter().cloned());
        }
        grant
    }

    /// The grant `worker` has now.
    pub fn grant_of(&self, worker: &str) -> Option<Grant> {
        lock(&self.inner.workers)
            .get(worker)
            .and_then(|entry| entry.grant.clone())
    }

    /// The upstream serving `namespace`, if any.
    pub fn upstream(&self, namespace: &str) -> Option<Arc<Upstream>> {
        self.inner.resolver.upstream(namespace)
    }

    /// Serves one connection of `worker` until it closes.
    pub async fn serve_stream<S>(&self, worker: &str, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let proxy = self.clone();
        let worker = Arc::<str>::from(worker);
        let service = hyper::service::service_fn(move |request| {
            let proxy = proxy.clone();
            let worker = Arc::clone(&worker);
            async move { Ok::<_, std::convert::Infallible>(s3::handle(&proxy, &worker, request).await) }
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .keep_alive(true)
            .serve_connection(TokioIo::new(stream), service)
            .await;
    }

    /// Serves every connection to `listener` as `worker`'s, until the task is
    /// aborted: the endpoint of a worker that has no forwarder.
    pub async fn serve(&self, listener: TcpListener, worker: &str) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                // Out of descriptors, most likely: back off rather than spin.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            };
            let proxy = self.clone();
            let worker = worker.to_string();
            tokio::spawn(async move { proxy.serve_stream(&worker, stream).await });
        }
    }

    /// The counters so far.
    pub fn metrics(&self) -> Metrics {
        lock(&self.inner.metrics).clone()
    }

    /// The cache counters of `namespace`'s upstream.
    pub fn cache_stats(&self, namespace: &str) -> Option<CacheStats> {
        self.upstream(namespace).map(|up| up.cache().stats())
    }

    /// Starts (`true`) or stops keeping the heads of the requests workers send,
    /// at most [`MAX_RECORDED_REQUESTS`]: what a test inspects to show that no
    /// credential reaches or comes from a worker.
    pub fn record_requests(&self, on: bool) {
        *lock(&self.inner.recorded) = on.then(Vec::new);
    }

    /// The request heads recorded so far.
    pub fn requests_seen(&self) -> Vec<SeenRequest> {
        lock(&self.inner.recorded).clone().unwrap_or_default()
    }

    /// Records the request `seen` builds, building it only when recording is on
    /// and below its cap.
    fn record_with(&self, seen: impl FnOnce() -> SeenRequest) {
        if let Some(recorded) = lock(&self.inner.recorded).as_mut()
            && recorded.len() < MAX_RECORDED_REQUESTS
        {
            recorded.push(seen());
        }
    }

    fn count(&self, namespace: Option<&str>, update: impl FnOnce(&mut NamespaceMetrics)) {
        let mut metrics = lock(&self.inner.metrics);
        match namespace {
            Some(ns) => update(metrics.namespaces.entry(ns.to_string()).or_default()),
            None => metrics.refused_unbound += 1,
        }
    }
}

/// Its counters for the front's `/metrics` (HS1 Task 23): requests, refusals
/// and bytes over every namespace, and the blocks its caches hit and missed
/// (each distinct upstream counted once).
impl loams_house::metrics::CacheMetrics for HouseCache {
    fn cache_counters(&self) -> loams_house::metrics::CacheCounters {
        let metrics = self.metrics();
        let mut out = loams_house::metrics::CacheCounters::default();
        let mut seen: Vec<*const Upstream> = Vec::new();
        for (namespace, m) in &metrics.namespaces {
            out.requests += m.requests;
            out.refused += m.refused;
            out.bytes += m.bytes_served;
            if let Some(upstream) = self.upstream(namespace) {
                let at = Arc::as_ptr(&upstream);
                if !seen.contains(&at) {
                    seen.push(at);
                    let stats = upstream.cache().stats();
                    out.hits += stats.hits;
                    out.misses += stats.misses;
                }
            }
        }
        out.refused += metrics.refused_unbound;
        out
    }
}

impl ForwardTarget for HouseCache {
    fn accept(&self, worker: &str, stream: StdUnixStream, slot: ForwardSlot) {
        let proxy = self.clone();
        let worker = worker.to_string();
        self.inner.runtime.spawn(async move {
            let _slot = slot;
            if stream.set_nonblocking(true).is_err() {
                return;
            }
            if let Ok(stream) = tokio::net::UnixStream::from_std(stream) {
                proxy.serve_stream(&worker, stream).await;
            }
        });
    }

    fn endpoint(&self, worker: &str) -> io::Result<SocketAddr> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let proxy = self.clone();
        let name = worker.to_string();
        let task = self.inner.runtime.spawn(async move {
            if let Ok(listener) = TcpListener::from_std(listener) {
                proxy.serve(listener, &name).await;
            }
        });
        let mut workers = lock(&self.inner.workers);
        let entry = workers.entry(worker.to_string()).or_default();
        if let Some(old) = entry.endpoint.replace(task) {
            old.abort();
        }
        Ok(addr)
    }

    fn bound(&self, worker: &str, namespace: &str) {
        self.grant(worker, self.grant_for(namespace));
    }

    fn gone(&self, worker: &str) {
        self.revoke(worker);
    }
}

impl loams_house::catalog::PrefixGrants for HouseCache {
    fn vouch(&self, namespace: &str, prefix: &str) {
        HouseCache::vouch(self, namespace, prefix);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_namespace_grant_is_its_own_prefix() {
        let grant = Grant::for_namespace("7");
        assert!(grant.allows("ns/7/house/t/data/a.parquet"));
        assert!(!grant.allows("ns/70/house/t"));
        assert!(!grant.allows("ns/8/x"));
        assert!(!grant.allows("ns/7"));
        assert!(!grant.list);
    }
}
