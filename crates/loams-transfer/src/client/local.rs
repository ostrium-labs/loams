//! The client's local side (§85 §7): streamed file I/O in 1 MiB blocks, the
//! local state of uploads, the download checkpoint, and the install lock.
//!
//! Every read and write here moves at most one block at a time, so memory
//! does not grow with the file (D2432).

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::net::ByteStream;

/// The I/O block: what one read or write moves.
pub const BLOCK: usize = 1 << 20;

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, off)
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, off)
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::write_at(f, buf, off)
}

#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], off: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_write(f, buf, off)
}

/// Reads exactly `buf.len()` bytes at `off`.
pub fn read_exact_at(f: &File, mut buf: &mut [u8], mut off: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = read_at(f, buf, off)?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the file is shorter than expected (did it change?)",
            ));
        }
        buf = &mut buf[n..];
        off += n as u64;
    }
    Ok(())
}

/// Writes all of `buf` at `off`.
pub fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = write_at(f, buf, off)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "write returned 0"));
        }
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

/// SHA-256 of `[off, off + len)`, read in blocks on a blocking thread.
pub async fn hash_range(f: Arc<File>, off: u64, len: u64) -> io::Result<String> {
    tokio::task::spawn_blocking(move || {
        let mut h = Sha256::new();
        let mut buf = vec![0u8; BLOCK];
        let mut pos = 0u64;
        while pos < len {
            let n = BLOCK.min((len - pos) as usize);
            read_exact_at(&f, &mut buf[..n], off + pos)?;
            h.update(&buf[..n]);
            pos += n as u64;
        }
        Ok(hex::encode(h.finalize()))
    })
    .await
    .map_err(io::Error::other)?
}

/// SHA-256 of a whole file, read sequentially in blocks; `on_block` sees
/// each block's length (for "verifying" progress).
pub async fn hash_file(
    path: PathBuf,
    on_block: impl Fn(u64) + Send + 'static,
) -> io::Result<String> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut f = File::open(&path)?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; BLOCK];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
            on_block(n as u64);
        }
        Ok(hex::encode(h.finalize()))
    })
    .await
    .map_err(io::Error::other)?
}

/// `[off, off + len)` as a body, read one block at a time as it is sent.
pub fn range_stream(f: Arc<File>, off: u64, len: u64) -> ByteStream {
    futures::stream::unfold(0u64, move |pos| {
        let f = Arc::clone(&f);
        async move {
            if pos >= len {
                return None;
            }
            let n = BLOCK.min((len - pos) as usize);
            let r = tokio::task::spawn_blocking(move || {
                let mut buf = vec![0u8; n];
                read_exact_at(&f, &mut buf, off + pos)?;
                Ok::<_, io::Error>(Bytes::from(buf))
            })
            .await
            .map_err(io::Error::other)
            .and_then(|r| r);
            let next = if r.is_ok() { pos + n as u64 } else { len };
            Some((r, next))
        }
    })
    .boxed()
}

/// Writes a streamed chunk at `off` while hashing it; returns its length
/// and SHA-256. With `skip_zeros`, all-zero blocks are not written, so a
/// sparse part file stays sparse where the source was: the part file starts
/// as zeros. A retry passes `false`, since a failed attempt may have left
/// other bytes there.
pub async fn write_stream(
    f: Arc<File>,
    off: u64,
    mut body: ByteStream,
    skip_zeros: bool,
) -> io::Result<(u64, String)> {
    let mut h = Sha256::new();
    let mut pos = 0u64;
    while let Some(b) = body.next().await {
        let b = b?;
        h.update(&b);
        if !skip_zeros || b.iter().any(|x| *x != 0) {
            let f = Arc::clone(&f);
            let at = off + pos;
            let data = b.clone();
            tokio::task::spawn_blocking(move || write_all_at(&f, &data, at))
                .await
                .map_err(io::Error::other)??;
        }
        pos += b.len() as u64;
    }
    Ok((pos, hex::encode(h.finalize())))
}

/// Writes `data` to `path` through a temporary file and a rename, after
/// syncing it: a crash leaves the old or the new content, never a mix.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let tmp = path.with_extension("tmp");
    {
        let mut f = File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_data()?;
    }
    std::fs::rename(&tmp, path)
}

/// An upload's local state (§85 §7.1): written before the transfer is
/// created, removed after it completes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadState {
    pub tid: String,
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    /// The source's modification time, in ns since the epoch.
    pub mtime_ns: u128,
    pub chunk_size: u64,
    pub server: String,
    pub ns: String,
}

/// The local state directory of one install.
#[derive(Clone, Debug)]
pub struct StateDir {
    root: PathBuf,
}

impl StateDir {
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("uploads"))?;
        Ok(StateDir { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn upload_path(&self, tid: &str) -> PathBuf {
        self.root.join("uploads").join(format!("{tid}.json"))
    }

    pub fn save_upload(&self, s: &UploadState) -> io::Result<()> {
        write_atomic(&self.upload_path(&s.tid), &serde_json::to_vec_pretty(s)?)
    }

    pub fn load_upload(&self, tid: &str) -> io::Result<Option<UploadState>> {
        match std::fs::read(self.upload_path(tid)) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn remove_upload(&self, tid: &str) -> io::Result<()> {
        match std::fs::remove_file(self.upload_path(tid)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Every unfinished upload.
    pub fn uploads(&self) -> io::Result<Vec<UploadState>> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(self.root.join("uploads"))? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Ok(b) = std::fs::read(&p)
                && let Ok(s) = serde_json::from_slice(&b)
            {
                out.push(s);
            }
        }
        out.sort_by(|a: &UploadState, b| a.tid.cmp(&b.tid));
        Ok(out)
    }

    /// The install id: made once, then stable. It is the lease holder
    /// (D2428).
    pub fn install_id(&self) -> io::Result<String> {
        let p = self.root.join("install-id");
        match std::fs::read_to_string(&p) {
            Ok(s) if !s.trim().is_empty() => Ok(s.trim().to_string()),
            _ => {
                let id = format!("install-{}", crate::keys::new_id());
                write_atomic(&p, id.as_bytes())?;
                Ok(id)
            }
        }
    }

    /// Takes the install's exclusive lock (the local half of the single
    /// writer, D2428). The lock lasts as long as the returned file is open;
    /// the OS releases it when the process dies.
    pub fn lock(&self) -> io::Result<File> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.root.join("agent.lock"))?;
        match f.try_lock() {
            Ok(()) => Ok(f),
            Err(std::fs::TryLockError::WouldBlock) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "another loams-transfer agent holds this state directory",
            )),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }

    /// The local mirror of run `rid`'s log (§85 §11).
    pub fn run_log(&self, rid: &str) -> PathBuf {
        let dir = self.root.join("runs");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(format!("{rid}.log"))
    }

    /// The transport memory file (§85 §10).
    pub fn transport_memory(&self) -> PathBuf {
        self.root.join("transport.json")
    }
}

/// A download's local checkpoint (§85 §7.2), beside the part file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadCheckpoint {
    pub tid: String,
    pub size: u64,
    pub chunk_size: u64,
    pub file_sha256: String,
    /// Verified, durable chunk indexes.
    pub verified: Vec<u32>,
}

/// The part file and checkpoint paths for a destination.
pub fn part_paths(dest: &Path) -> (PathBuf, PathBuf) {
    let mut part = dest.as_os_str().to_owned();
    part.push(".loams-part");
    let part = PathBuf::from(part);
    let mut ck = part.as_os_str().to_owned();
    ck.push(".json");
    (part, PathBuf::from(ck))
}

/// The modification time of `meta`, in ns since the epoch.
pub fn mtime_ns(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().expect("dir");
        let s = StateDir::new(dir.path()).expect("state");
        let held = s.lock().expect("first");
        assert!(s.lock().is_err());
        drop(held);
        s.lock().expect("after release");
    }

    #[test]
    fn install_id_is_stable() {
        let dir = tempfile::tempdir().expect("dir");
        let s = StateDir::new(dir.path()).expect("state");
        assert_eq!(s.install_id().expect("a"), s.install_id().expect("b"));
    }

    #[tokio::test]
    async fn sparse_write_skips_zero_blocks_and_hashes_all() {
        let dir = tempfile::tempdir().expect("dir");
        let p = dir.path().join("f");
        let f = File::create(&p).expect("create");
        f.set_len(4 * BLOCK as u64).expect("len");
        let f = Arc::new(f);
        let mut data = vec![0u8; 2 * BLOCK];
        data[BLOCK + 5] = 9;
        let body = futures::stream::iter(
            data.chunks(BLOCK)
                .map(|c| Ok(Bytes::copy_from_slice(c)))
                .collect::<Vec<_>>(),
        )
        .boxed();
        let (n, sha) = write_stream(Arc::clone(&f), BLOCK as u64, body, true)
            .await
            .expect("write");
        assert_eq!(n, 2 * BLOCK as u64);
        assert_eq!(sha, hex::encode(Sha256::digest(&data)));
        let back = std::fs::read(&p).expect("read");
        assert_eq!(&back[BLOCK..3 * BLOCK], &data[..]);
    }
}
