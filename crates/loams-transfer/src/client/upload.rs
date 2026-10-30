//! The upload pipeline (§85 §7.1).
//!
//! The local state is written before the transfer is created; the server's
//! checkpoint says which chunks to skip; up to `window` chunks are in flight,
//! each hashed from disk and then streamed from disk (D2431, D2432); a
//! sequential reader hashes the whole file beside them; the lease is renewed
//! at a third of its life; `complete` carries the whole-file hash.

use std::collections::BTreeSet;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use super::local::{self, StateDir, UploadState};
use super::progress::{Direction, Phase, Tracker};
use super::{TransferError, backoff};
use crate::api::{Client, ClientError};
use crate::transfer::{CreateTransfer, TransferState, TransferView};

/// Upload settings.
#[derive(Clone, Debug)]
pub struct UploadOpts {
    /// Chunks in flight (D2431: 8).
    pub window: usize,
    /// A fixed chunk size; `None` picks one (§85 §6).
    pub chunk_size: Option<u64>,
    /// The smallest size [`pick_chunk_size`] may pick.
    pub min_chunk: u64,
    /// How often the lease is renewed (a third of its 30 s TTL).
    pub renew_every: Duration,
    /// Attempts per chunk before the upload fails.
    pub max_attempts: u32,
    /// A pause before each chunk request (tests, and a crude rate limit).
    pub pace: Option<Duration>,
}

impl Default for UploadOpts {
    fn default() -> Self {
        UploadOpts {
            window: 8,
            chunk_size: None,
            min_chunk: 16 << 20,
            renew_every: Duration::from_secs(10),
            max_attempts: 12,
            pace: None,
        }
    }
}

/// The smallest power-of-two chunk of at least `min` that keeps the file
/// within 16,384 chunks, up to 64 MiB (D2423).
pub fn pick_chunk_size(size: u64, min: u64) -> u64 {
    let mut c = min.next_power_of_two().max(1);
    while size.div_ceil(c) > 16_384 && c < (64 << 20) {
        c *= 2;
    }
    c
}

/// Starts (or, with `resume`, continues) an upload of `path`.
///
/// Returns the completed transfer's view.
pub async fn upload(
    client: &Client,
    state: &StateDir,
    path: &Path,
    resume: Option<&str>,
    opts: &UploadOpts,
    tracker_out: impl FnOnce(Tracker),
) -> Result<TransferView, TransferError> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(TransferError::Local(format!(
            "{} is not a file",
            path.display()
        )));
    }
    let size = meta.len();
    let mtime = local::mtime_ns(&meta);
    let holder = state.install_id()?;
    let st = match resume {
        Some(tid) => {
            let st = state
                .load_upload(tid)?
                .ok_or_else(|| TransferError::Local(format!("no local state for {tid}")))?;
            if st.size != size
                || st.mtime_ns != mtime
                || st.path != path
                || st.server != client.origin()
                || st.ns != client.ns()
            {
                return Err(TransferError::SourceChanged(format!(
                    "{} changed since the upload started; start a new transfer",
                    path.display()
                )));
            }
            st
        }
        None => {
            let st = UploadState {
                tid: crate::keys::new_id(),
                path: path.to_path_buf(),
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into()),
                size,
                mtime_ns: mtime,
                chunk_size: opts
                    .chunk_size
                    .unwrap_or_else(|| pick_chunk_size(size, opts.min_chunk)),
                server: client.origin().to_string(),
                ns: client.ns().to_string(),
            };
            // The local state first: a crash after the create still resumes.
            state.save_upload(&st)?;
            st
        }
    };
    let tid = st.tid.clone();
    let req = CreateTransfer {
        name: st.name.clone(),
        size,
        chunk_size: st.chunk_size,
        client_id: holder.clone(),
    };
    let view = retry(opts, || client.create(&tid, &req)).await?;
    let tracker = Tracker::new(&tid, Direction::Upload, size, view.committed_bytes);
    tracker_out(tracker.clone());
    if view.state == TransferState::Complete {
        state.remove_upload(&tid)?;
        tracker.set_phase(Phase::Done);
        return Ok(view);
    }
    if view.state == TransferState::Abandoned {
        return Err(TransferError::Local(format!(
            "transfer {tid} was abandoned"
        )));
    }
    if let Ok(t) = client.transport().await {
        tracker.set_transport(t);
    }
    let lease = retry(opts, || client.lease(&tid, &holder)).await?;
    let epoch = Arc::new(AtomicU64::new(lease.epoch));

    // The lease renewer: every third of the TTL (D2428).
    let renewer = {
        let (client, tid, holder, epoch) = (
            client.clone(),
            tid.clone(),
            holder.clone(),
            Arc::clone(&epoch),
        );
        let every = opts.renew_every;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(every).await;
                match client.lease(&tid, &holder).await {
                    Ok(g) => epoch.store(g.epoch, Ordering::SeqCst),
                    Err(e) if e.code() == Some("lease_held") => return Err::<(), _>(e),
                    Err(_) => {}
                }
            }
        })
    };
    let result = run_pipeline(client, &st, &view, &epoch, &holder, opts, &tracker).await;
    renewer.abort();
    let file_sha = result?;
    tracker.set_phase(Phase::Verifying);
    let done = retry(opts, || {
        client.complete(&tid, epoch.load(Ordering::SeqCst), &file_sha)
    })
    .await?;
    state.remove_upload(&tid)?;
    tracker.set_done(done.committed_bytes);
    tracker.set_phase(Phase::Done);
    Ok(done)
}

async fn run_pipeline(
    client: &Client,
    st: &UploadState,
    view: &TransferView,
    epoch: &Arc<AtomicU64>,
    holder: &str,
    opts: &UploadOpts,
    tracker: &Tracker,
) -> Result<String, TransferError> {
    let file = Arc::new(File::open(&st.path)?);
    let committed: BTreeSet<u32> = view.committed.iter().flat_map(|[a, b]| *a..*b).collect();
    let chunks = u32::try_from(view.chunks).map_err(|_| TransferError::Local("chunks".into()))?;
    let todo: Vec<u32> = (0..chunks).filter(|i| !committed.contains(i)).collect();
    tracker.set_phase(Phase::Transferring);

    // The whole-file hash, sequentially, beside the chunks (§85 §7.1 step 5).
    let whole = tokio::spawn(local::hash_file(st.path.clone(), |_| {}));

    let mut pending = todo.into_iter();
    let mut inflight = FuturesUnordered::new();
    loop {
        while inflight.len() < opts.window.max(1) {
            let Some(i) = pending.next() else { break };
            let off = u64::from(i) * st.chunk_size;
            let len = st.chunk_size.min(st.size - off);
            let (client, tid, file, epoch, holder, tracker) = (
                client.clone(),
                st.tid.clone(),
                Arc::clone(&file),
                Arc::clone(epoch),
                holder.to_string(),
                tracker.clone(),
            );
            let (max, pace) = (opts.max_attempts, opts.pace);
            tracker.inflight(1);
            inflight.push(async move {
                if let Some(p) = pace {
                    tokio::time::sleep(p).await;
                }
                let r = send_chunk(
                    &client, &tid, &holder, &file, i, off, len, &epoch, &tracker, max,
                )
                .await;
                tracker.inflight(-1);
                r
            });
        }
        let Some(r) = inflight.next().await else {
            break;
        };
        let put = r?;
        tracker.raise_done(put);
    }
    let sha = whole
        .await
        .map_err(|e| TransferError::Local(e.to_string()))??;
    Ok(sha)
}

/// Sends one chunk until it is committed; returns the committed bytes the
/// server reported.
#[allow(clippy::too_many_arguments)]
async fn send_chunk(
    client: &Client,
    tid: &str,
    holder: &str,
    file: &Arc<File>,
    i: u32,
    off: u64,
    len: u64,
    epoch: &AtomicU64,
    tracker: &Tracker,
    max_attempts: u32,
) -> Result<u64, TransferError> {
    let mut attempt = 0u32;
    loop {
        // Pass 1: hash from disk. Pass 2: send from disk (D2432).
        let sha = local::hash_range(Arc::clone(file), off, len).await?;
        let body = local::range_stream(Arc::clone(file), off, len);
        let r = client
            .put_chunk(tid, i, &sha, epoch.load(Ordering::SeqCst), body)
            .await;
        let err = match r {
            Ok(p) => return Ok(p.committed_bytes),
            Err(e) => e,
        };
        attempt += 1;
        tracker.retry();
        match err.code() {
            // Our epoch is stale (a renewal raced, or the lease lapsed while
            // we were away): take it again as the same holder and retry.
            Some("lease_lost") => {
                if let Ok(g) = client.lease(tid, holder).await {
                    epoch.store(g.epoch, Ordering::SeqCst);
                }
            }
            Some("lease_held") => return Err(err.into()),
            // The bytes changed between the two passes, or on the wire.
            Some("integrity") if attempt < 3 => {}
            Some("chunk_conflict") | Some("integrity") => {
                return Err(TransferError::SourceChanged(format!(
                    "chunk {i} no longer matches what was committed: {err}"
                )));
            }
            _ if err.is_transient() => {}
            _ => return Err(err.into()),
        }
        if attempt >= max_attempts {
            return Err(err.into());
        }
        if matches!(err, ClientError::Io(_)) {
            client.reset().await;
        }
        tokio::time::sleep(backoff(attempt)).await;
    }
}

/// Retries a request while its error is transient.
pub(super) async fn retry<T, F, Fut>(opts: &UploadOpts, mut f: F) -> Result<T, TransferError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, ClientError>>,
{
    let mut attempt = 0u32;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if e.is_transient() && attempt + 1 < opts.max_attempts => {
                attempt += 1;
                tokio::time::sleep(backoff(attempt)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_size_grows_to_stay_within_the_chunk_limit() {
        let mib = 1u64 << 20;
        assert_eq!(pick_chunk_size(10 << 30, 16 * mib), 16 * mib);
        assert_eq!(pick_chunk_size(300 << 30, 16 * mib), 32 * mib);
        assert_eq!(pick_chunk_size(1 << 40, 16 * mib), 64 * mib);
        assert_eq!(pick_chunk_size(1000, 64 << 10), 64 << 10);
    }
}
