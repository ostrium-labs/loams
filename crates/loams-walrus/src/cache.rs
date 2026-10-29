//! `VerifiedDiskCache`: verified blobs on local disk, keyed by sha256.
//!
//! - Layout: `<dir>/sha256/<first two hex digits>/<64 hex digits>`, plus
//!   `<dir>/index`, which maps blob ids and patch ids to sha256 digests
//!   (one `<name> <hex>` line each). The key is the sha256, never a blob
//!   id: the cache only holds bytes whose hash it checked.
//! - A write goes to a temp file in the same directory, is synced, then
//!   renamed into place: a reader never sees a partial file.
//! - The cache is bounded by size; the least recently read blobs are
//!   evicted first. Recency is kept in memory and seeded from file mtimes
//!   when the cache is opened.
//! - A read hits only when the caller names the expected sha256, and every
//!   read re-hashes the file (so the first read after a restart does too).
//!   A file that no longer matches is deleted and reported as a miss, and
//!   the caller fetches the blob again.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use tokio::io::AsyncWriteExt;

use crate::types::ContentDigest;

const TMP_PREFIX: &str = ".tmp-";
const BLOBS: &str = "sha256";
const INDEX: &str = "index";

#[derive(Debug, Default)]
struct Index {
    /// Size and last-use tick of each cached blob.
    entries: HashMap<ContentDigest, (u64, u64)>,
    /// Blob or patch id to digest.
    names: BTreeMap<String, ContentDigest>,
    total: u64,
    tick: u64,
}

impl Index {
    fn touch(&mut self, d: &ContentDigest) {
        self.tick += 1;
        let tick = self.tick;
        if let Some(e) = self.entries.get_mut(d) {
            e.1 = tick;
        }
    }

    fn insert(&mut self, d: ContentDigest, size: u64) {
        self.tick += 1;
        if let Some((old, _)) = self.entries.insert(d, (size, self.tick)) {
            self.total -= old;
        }
        self.total += size;
    }

    fn remove(&mut self, d: &ContentDigest) {
        if let Some((size, _)) = self.entries.remove(d) {
            self.total -= size;
        }
    }

    /// The least recently used entries to drop until `total <= max`,
    /// never `keep`.
    fn victims(&mut self, max: u64, keep: Option<&ContentDigest>) -> Vec<ContentDigest> {
        let mut order: Vec<_> = self
            .entries
            .iter()
            .filter(|(d, _)| Some(*d) != keep)
            .map(|(d, (size, tick))| (*tick, *d, *size))
            .collect();
        order.sort_unstable();
        let mut out = Vec::new();
        let mut total = self.total;
        for (_, d, size) in order {
            if total <= max {
                break;
            }
            total -= size;
            out.push(d);
        }
        for d in &out {
            self.remove(d);
        }
        if !out.is_empty() {
            self.names.retain(|_, d| !out.contains(d));
        }
        out
    }

    fn render_names(&self) -> String {
        let mut s = String::new();
        for (name, d) in &self.names {
            s.push_str(name);
            s.push(' ');
            s.push_str(&d.to_hex());
            s.push('\n');
        }
        s
    }
}

/// Verified blobs on local disk, keyed by sha256, bounded by size with LRU
/// eviction, re-verified on every read.
#[derive(Debug)]
pub struct VerifiedDiskCache {
    dir: PathBuf,
    max_bytes: u64,
    index: Mutex<Index>,
    /// Serializes index saves, so the last rename carries the latest names.
    save: tokio::sync::Mutex<()>,
    seq: AtomicU64,
}

impl VerifiedDiskCache {
    /// Opens (creating it if needed) the cache at `dir`, bounded to
    /// `max_bytes`. Leftover temp files are removed; files whose names are
    /// not digests are left alone and not counted.
    pub async fn open(dir: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let dir = dir.into();
        let blobs = dir.join(BLOBS);
        tokio::fs::create_dir_all(&blobs).await?;
        let mut found = Vec::new();
        let mut shards = tokio::fs::read_dir(&blobs).await?;
        while let Some(shard) = shards.next_entry().await? {
            if !shard.file_type().await?.is_dir() {
                continue;
            }
            let mut files = tokio::fs::read_dir(shard.path()).await?;
            while let Some(f) = files.next_entry().await? {
                let name = f.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with(TMP_PREFIX) {
                    let _ = tokio::fs::remove_file(f.path()).await;
                    continue;
                }
                let Ok(d) = ContentDigest::parse_hex(name) else {
                    continue;
                };
                let meta = f.metadata().await?;
                found.push((meta.modified().ok(), d, meta.len()));
            }
        }
        found.sort_by_key(|(mtime, _, _)| *mtime);
        let mut index = Index::default();
        for (_, d, size) in found {
            index.insert(d, size);
        }
        match tokio::fs::read_to_string(dir.join(INDEX)).await {
            Ok(text) => {
                for line in text.lines() {
                    if let Some((name, hex)) = line.split_once(' ')
                        && let Ok(d) = ContentDigest::parse_hex(hex)
                        && index.entries.contains_key(&d)
                    {
                        index.names.insert(name.to_string(), d);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let cache = Self {
            dir,
            max_bytes,
            index: Mutex::new(index),
            save: tokio::sync::Mutex::new(()),
            seq: AtomicU64::new(0),
        };
        cache.evict(None).await;
        Ok(cache)
    }

    /// The cache's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The bytes cached under `digest`, re-verified. A file that does not
    /// hash to `digest` is deleted and reported as a miss.
    pub async fn get(&self, digest: &ContentDigest) -> io::Result<Option<Bytes>> {
        let path = self.path_of(digest);
        let data = match tokio::fs::read(&path).await {
            Ok(data) => data,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.lock().remove(digest);
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        if ContentDigest::of(&data) != *digest {
            tracing::warn!(%digest, path = %path.display(), "walrus cache: corrupt file removed");
            let _ = tokio::fs::remove_file(&path).await;
            self.lock().remove(digest);
            return Ok(None);
        }
        {
            let mut index = self.lock();
            if index.entries.contains_key(digest) {
                index.touch(digest);
            } else {
                index.insert(*digest, data.len() as u64);
            }
        }
        Ok(Some(Bytes::from(data)))
    }

    /// The digest recorded for a blob or patch id, if it is still cached.
    pub fn lookup(&self, name: &str) -> Option<ContentDigest> {
        self.lock().names.get(name).copied()
    }

    /// Whether `digest` is indexed (no read, no verification).
    pub fn contains(&self, digest: &ContentDigest) -> bool {
        self.lock().entries.contains_key(digest)
    }

    /// The bytes the index counts.
    pub fn total_bytes(&self) -> u64 {
        self.lock().total
    }

    /// Caches `data` under `digest`, recording `name` (a blob or patch id)
    /// for it when given. `data` must hash to `digest`
    /// ([`io::ErrorKind::InvalidData`] otherwise): unverified bytes are
    /// never stored. A blob larger than the whole cache is not stored.
    pub async fn put(
        &self,
        name: Option<&str>,
        digest: &ContentDigest,
        data: &[u8],
    ) -> io::Result<()> {
        if ContentDigest::of(data) != *digest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("walrus cache: data does not hash to {digest}"),
            ));
        }
        let size = data.len() as u64;
        if size > self.max_bytes {
            return Ok(());
        }
        let path = self.path_of(digest);
        let shard = path.parent().unwrap_or(&self.dir);
        tokio::fs::create_dir_all(shard).await?;
        let tmp = self.tmp_in(shard);
        write_atomic(&tmp, &path, data).await?;
        let names_changed = {
            let mut index = self.lock();
            index.insert(*digest, size);
            match name {
                Some(n) if !n.contains(char::is_whitespace) => {
                    index.names.insert(n.to_string(), *digest) != Some(*digest)
                }
                _ => false,
            }
        };
        let evicted = self.evict(Some(digest)).await;
        if names_changed || evicted {
            self.save_index().await?;
        }
        Ok(())
    }

    /// Evicts down to the bound; whether anything went.
    async fn evict(&self, keep: Option<&ContentDigest>) -> bool {
        let victims = self.lock().victims(self.max_bytes, keep);
        for d in &victims {
            if let Err(e) = tokio::fs::remove_file(self.path_of(d)).await
                && e.kind() != io::ErrorKind::NotFound
            {
                tracing::warn!(digest = %d, error = %e, "walrus cache: eviction failed");
            }
        }
        !victims.is_empty()
    }

    async fn save_index(&self) -> io::Result<()> {
        let _saving = self.save.lock().await;
        let text = self.lock().render_names();
        let tmp = self.tmp_in(&self.dir);
        write_atomic(&tmp, &self.dir.join(INDEX), text.as_bytes()).await
    }

    fn tmp_in(&self, dir: &Path) -> PathBuf {
        dir.join(format!(
            "{TMP_PREFIX}{}-{}",
            std::process::id(),
            self.seq.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn path_of(&self, d: &ContentDigest) -> PathBuf {
        let hex = d.to_hex();
        self.dir.join(BLOBS).join(&hex[..2]).join(hex)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        // Every update leaves the index consistent, so a poisoned lock is
        // reused.
        self.index.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Writes `data` to `tmp`, syncs it and renames it onto `path`.
async fn write_atomic(tmp: &Path, path: &Path, data: &[u8]) -> io::Result<()> {
    let written = async {
        let mut f = tokio::fs::File::create(tmp).await?;
        f.write_all(data).await?;
        f.sync_all().await?;
        drop(f);
        tokio::fs::rename(tmp, path).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(tmp).await;
    }
    written
}
