//! The download pipeline (§85 §7.2).
//!
//! Chunks stream into a sparse part file at their offsets, hashed as they
//! arrive. A chunk counts only once the part file is synced and the local
//! checkpoint, rewritten through a rename, lists it (D2434). The end hashes
//! the whole part file against the transfer's file hash, then renames it.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use tokio::sync::Mutex;

use super::local::{self, DownloadCheckpoint};
use super::progress::{Direction, Phase, Tracker};
use super::upload::{UploadOpts, retry};
use super::{TransferError, backoff};
use crate::api::Client;
use crate::transfer::TransferState;

/// Downloads transfer `tid` to `dest`, resuming from `dest`'s part file if
/// one exists for the same transfer. Returns `dest`.
pub async fn download(
    client: &Client,
    tid: &str,
    dest: &Path,
    opts: &UploadOpts,
    tracker_out: impl FnOnce(Tracker),
) -> Result<PathBuf, TransferError> {
    let view = retry(opts, || client.view(tid)).await?;
    if view.state != TransferState::Complete {
        return Err(TransferError::Local(format!(
            "transfer {tid} is {:?}, not complete",
            view.state
        )));
    }
    let file_sha = view
        .file_sha256
        .clone()
        .ok_or_else(|| TransferError::Local("the transfer has no file hash".into()))?;
    let (part, ck_path) = local::part_paths(dest);
    let mut ck = match std::fs::read(&ck_path) {
        Ok(b) => serde_json::from_slice::<DownloadCheckpoint>(&b).ok(),
        Err(_) => None,
    }
    .filter(|c| {
        c.tid == tid
            && c.size == view.size
            && c.chunk_size == view.chunk_size
            && c.file_sha256 == file_sha
    })
    .filter(|_| part.exists());
    let file = if ck.is_some() {
        OpenOptions::new().write(true).open(&part)?
    } else {
        // A fresh, sparse part file at the full size.
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&part)?;
        f.set_len(view.size)?;
        f.sync_all()?;
        let c = DownloadCheckpoint {
            tid: tid.to_string(),
            size: view.size,
            chunk_size: view.chunk_size,
            file_sha256: file_sha.clone(),
            verified: Vec::new(),
        };
        local::write_atomic(&ck_path, &serde_json::to_vec(&c)?)?;
        ck = Some(c);
        f
    };
    let ck = ck.ok_or_else(|| TransferError::Local("checkpoint".into()))?;
    let verified: BTreeSet<u32> = ck.verified.iter().copied().collect();
    let chunk_len = |i: u32| {
        let off = u64::from(i) * view.chunk_size;
        view.chunk_size.min(view.size - off)
    };
    let done: u64 = verified.iter().map(|i| chunk_len(*i)).sum();
    let tracker = Tracker::new(tid, Direction::Download, view.size, done);
    tracker_out(tracker.clone());
    if let Ok(t) = client.transport().await {
        tracker.set_transport(t);
    }
    tracker.set_phase(Phase::Transferring);
    let chunks = u32::try_from(view.chunks).map_err(|_| TransferError::Local("chunks".into()))?;
    let file = Arc::new(file);
    let state = Arc::new(Mutex::new(ck));
    let mut pending = (0..chunks).filter(|i| !verified.contains(i));
    let mut inflight = FuturesUnordered::new();
    loop {
        while inflight.len() < opts.window.max(1) {
            let Some(i) = pending.next() else { break };
            let (client, file, tracker, tid) = (
                client.clone(),
                Arc::clone(&file),
                tracker.clone(),
                tid.to_string(),
            );
            let off = u64::from(i) * view.chunk_size;
            let len = chunk_len(i);
            let (max, pace) = (opts.max_attempts, opts.pace);
            tracker.inflight(1);
            inflight.push(async move {
                if let Some(p) = pace {
                    tokio::time::sleep(p).await;
                }
                let r = fetch_chunk(&client, &tid, &file, i, off, len, &tracker, max).await;
                tracker.inflight(-1);
                r.map(|()| (i, len))
            });
        }
        let Some(r) = inflight.next().await else {
            break;
        };
        let (i, len) = r?;
        // The local commit: data durable first, then the checkpoint says so.
        let mut c = state.lock().await;
        let f = Arc::clone(&file);
        tokio::task::spawn_blocking(move || f.sync_data())
            .await
            .map_err(|e| TransferError::Local(e.to_string()))??;
        c.verified.push(i);
        c.verified.sort_unstable();
        local::write_atomic(&ck_path, &serde_json::to_vec(&*c)?)?;
        tracker.add_done(len);
    }
    drop(file);
    tracker.set_phase(Phase::Verifying);
    let got = local::hash_file(part.clone(), |_| {}).await?;
    if got != file_sha {
        // Something on disk changed under us: start over next time.
        let _ = std::fs::remove_file(&ck_path);
        return Err(TransferError::Integrity(format!(
            "the downloaded file's SHA-256 {got} does not match {file_sha}"
        )));
    }
    std::fs::rename(&part, dest)?;
    let _ = std::fs::remove_file(&ck_path);
    tracker.set_phase(Phase::Done);
    Ok(dest.to_path_buf())
}

#[allow(clippy::too_many_arguments)]
async fn fetch_chunk(
    client: &Client,
    tid: &str,
    file: &Arc<File>,
    i: u32,
    off: u64,
    len: u64,
    tracker: &Tracker,
    max_attempts: u32,
) -> Result<(), TransferError> {
    let mut attempt = 0u32;
    loop {
        let r = match client.get_chunk(tid, i).await {
            Ok((sha, body)) => {
                match local::write_stream(Arc::clone(file), off, body, attempt == 0).await {
                    Ok((n, got)) if n == len && got == sha => return Ok(()),
                    Ok((n, _)) => Err(TransferError::Integrity(format!(
                        "chunk {i}: {n} bytes or its hash did not match"
                    ))),
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) if !e.is_transient() => return Err(e.into()),
            Err(e) => Err(e.into()),
        };
        attempt += 1;
        tracker.retry();
        if attempt >= max_attempts {
            return r;
        }
        client.reset().await;
        tokio::time::sleep(backoff(attempt)).await;
    }
}
