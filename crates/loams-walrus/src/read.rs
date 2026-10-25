//! Reads from aggregators (docs.wal.app "Reading blobs", "Quilt HTTP
//! APIs"; the aggregator's OpenAPI):
//!
//! - `GET /v1/blobs/{blob_id}`, with `Range: bytes=a-b` for range reads
//!   (`206`; only the fetched slivers are consistency-checked);
//! - `GET /v1/blobs/by-quilt-id/{quilt_id}/{identifier}`;
//! - `GET /v1/blobs/by-quilt-patch-id/{patch_id}`;
//! - `GET /v1/quilts/{quilt_id}/patches`.

use std::future::Future;
use std::ops::Range;
use std::path::{Path, PathBuf};

use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use reqwest::StatusCode;
use sha2::{Digest as _, Sha256};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::error::{AttemptFailure, WalrusError};
use crate::hedge::{collapse, hedged};
use crate::types::{BlobId, ContentDigest, PatchEntry, QuiltId, QuiltPatchId, Unverified};
use crate::{Inner, RETRY_MAX, RETRY_START, Walrus};

impl Walrus {
    /// The blob `id`, whose bytes must hash to `sha256`. Served from the
    /// cache when it holds a verified copy; a fetched blob is cached.
    pub async fn read(&self, id: &BlobId, sha256: &ContentDigest) -> Result<Bytes, WalrusError> {
        let inner = self.inner()?;
        inner
            .read_verified(id.as_str(), id.as_str(), sha256, |agg| blob_url(agg, id))
            .await
    }

    /// The file stored as `identifier` in quilt `quilt`, whose bytes must
    /// hash to `sha256`. Cached like a blob.
    pub async fn read_quilt_patch(
        &self,
        quilt: &QuiltId,
        identifier: &str,
        sha256: &ContentDigest,
    ) -> Result<Bytes, WalrusError> {
        let inner = self.inner()?;
        let label = format!("{quilt}/{identifier}");
        inner
            .read_verified(&label, quilt.as_str(), sha256, |agg| {
                url_with(
                    agg,
                    &["v1", "blobs", "by-quilt-id", quilt.as_str(), identifier],
                )
            })
            .await
    }

    /// The quilt patch `patch`, whose bytes must hash to `sha256`. Cached
    /// like a blob.
    pub async fn read_patch(
        &self,
        patch: &QuiltPatchId,
        sha256: &ContentDigest,
    ) -> Result<Bytes, WalrusError> {
        let inner = self.inner()?;
        inner
            .read_verified(patch.as_str(), patch.as_str(), sha256, |agg| {
                url_with(agg, &["v1", "blobs", "by-quilt-patch-id", patch.as_str()])
            })
            .await
    }

    /// Streams the blob `id` to `dest` without holding it in memory. Each
    /// attempt writes a temp file beside `dest`; the winner's is synced and
    /// renamed onto `dest` only after its sha256 matched, so `dest` never
    /// holds unverified bytes. Returns the blob's length. The cache is not
    /// consulted.
    pub async fn read_to_path(
        &self,
        id: &BlobId,
        sha256: &ContentDigest,
        dest: &Path,
    ) -> Result<u64, WalrusError> {
        let inner = self.inner()?;
        let dir = match dest.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let (len, tmp) = inner
            .after_write(id.as_str(), || {
                hedged(
                    &inner.aggregators,
                    inner.hedge_after,
                    id.as_str(),
                    Some(sha256),
                    |agg| {
                        let url = blob_url(&agg, id);
                        let dir = dir.clone();
                        async move { inner.fetch_to_file(url, sha256, &dir).await }
                    },
                )
            })
            .await?;
        tmp.persist(dest).map_err(|e| WalrusError::Io(e.error))?;
        Ok(len)
    }

    /// UNVERIFIED: bytes `range` of blob `id`, through the `Range` header.
    /// Walrus checks only the consistency of the slivers a range read
    /// fetches, and a whole-blob sha256 cannot vet a slice, so the bytes
    /// come back as [`Unverified`]: the caller must check them against its
    /// own record. Aggregators are tried in order, without hedging; a range
    /// past the end is cut short.
    pub async fn read_range(
        &self,
        id: &BlobId,
        range: Range<u64>,
    ) -> Result<Unverified<Bytes>, WalrusError> {
        let inner = self.inner()?;
        if range.start >= range.end {
            return Ok(Unverified::new(Bytes::new()));
        }
        let want = range.end - range.start;
        if want > inner.max_blob_bytes {
            return Err(WalrusError::TooLarge {
                what: "range",
                size: want,
                max: inner.max_blob_bytes,
            });
        }
        let mut attempts = Vec::new();
        for agg in &inner.aggregators {
            match inner.fetch_range(blob_url(agg, id), range.clone()).await {
                Ok(bytes) => return Ok(Unverified::new(bytes)),
                Err(why) => attempts.push((agg.clone(), why)),
            }
        }
        Err(collapse(id.as_str(), None, attempts))
    }

    /// The patches of quilt `quilt` (`GET /v1/quilts/{id}/patches`). The
    /// listing itself is not verified: use its patch ids with
    /// [`Walrus::read_patch`] and a sha256 from a signed record.
    pub async fn list_patches(&self, quilt: &QuiltId) -> Result<Vec<PatchEntry>, WalrusError> {
        let inner = self.inner()?;
        inner
            .after_write(quilt.as_str(), || {
                hedged(
                    &inner.aggregators,
                    inner.hedge_after,
                    quilt.as_str(),
                    None,
                    |agg| {
                        let url = url_with(&agg, &["v1", "quilts", quilt.as_str(), "patches"]);
                        async move {
                            let body = inner.fetch_bounded(url).await?;
                            serde_json::from_slice::<Vec<PatchEntry>>(&body)
                                .map_err(|e| AttemptFailure::Other(format!("patch list: {e}")))
                        }
                    },
                )
            })
            .await
    }
}

impl Inner {
    /// A verified read through the cache. `written_key` is the id a write
    /// by this client would have recorded.
    async fn read_verified(
        &self,
        label: &str,
        written_key: &str,
        sha256: &ContentDigest,
        url: impl Fn(&Url) -> Url,
    ) -> Result<Bytes, WalrusError> {
        if let Some(cache) = &self.cache {
            match cache.get(sha256).await {
                Ok(Some(bytes)) => return Ok(bytes),
                Ok(None) => {}
                Err(e) => tracing::warn!(%sha256, error = %e, "walrus: cache read failed"),
            }
        }
        let bytes = self
            .after_write(written_key, || {
                hedged(
                    &self.aggregators,
                    self.hedge_after,
                    label,
                    Some(sha256),
                    |agg| {
                        let url = url(&agg);
                        async move { self.fetch_verified(url, sha256).await }
                    },
                )
            })
            .await?;
        if let Some(cache) = &self.cache
            && let Err(e) = cache.put(Some(label), sha256, &bytes).await
        {
            tracing::warn!(%sha256, error = %e, "walrus: cache write failed");
        }
        Ok(bytes)
    }

    /// Runs `op`; while it answers `NotFound` for an id this client wrote
    /// less than `read_after_write_ms` ago (a CDN can cache a 404 just after
    /// certification, and Testnet aggregators can lag), retries it with
    /// exponential backoff capped at 10 s.
    pub(crate) async fn after_write<T, F, Fut>(&self, key: &str, op: F) -> Result<T, WalrusError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, WalrusError>>,
    {
        let mut delay = RETRY_START;
        loop {
            match op().await {
                Err(WalrusError::NotFound(what)) => {
                    let Some(at) = self.written_at(key) else {
                        return Err(WalrusError::NotFound(what));
                    };
                    if at.elapsed() + delay >= self.write_window {
                        return Err(WalrusError::NotFound(what));
                    }
                    tracing::debug!(
                        blob = key,
                        ?delay,
                        "walrus: 404 just after a write, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(RETRY_MAX);
                }
                other => return other,
            }
        }
    }

    async fn get(&self, url: Url) -> Result<reqwest::Response, AttemptFailure> {
        let resp = self
            .http
            .get(url)
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| AttemptFailure::Other(format!("request: {e}")))?;
        check_status(resp)
    }

    fn check_length(&self, len: Option<u64>) -> Result<(), AttemptFailure> {
        match len {
            Some(size) if size > self.max_blob_bytes => Err(AttemptFailure::TooLarge {
                size,
                max: self.max_blob_bytes,
            }),
            _ => Ok(()),
        }
    }

    /// The whole body of `url`, bounded by `max_blob_bytes` while
    /// streaming, unverified (for listings).
    async fn fetch_bounded(&self, url: Url) -> Result<Bytes, AttemptFailure> {
        let resp = self.get(url).await?;
        self.body_bounded(resp).await
    }

    async fn body_bounded(&self, resp: reqwest::Response) -> Result<Bytes, AttemptFailure> {
        self.check_length(resp.content_length())?;
        let mut body = resp.bytes_stream();
        let mut buf = BytesMut::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| AttemptFailure::Other(format!("body: {e}")))?;
            self.check_length(Some((buf.len() + chunk.len()) as u64))?;
            buf.extend_from_slice(&chunk);
        }
        Ok(buf.freeze())
    }

    async fn fetch_verified(
        &self,
        url: Url,
        sha256: &ContentDigest,
    ) -> Result<Bytes, AttemptFailure> {
        let resp = self.get(url).await?;
        self.check_length(resp.content_length())?;
        let mut body = resp.bytes_stream();
        let mut hasher = Sha256::new();
        let mut buf = BytesMut::new();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| AttemptFailure::Other(format!("body: {e}")))?;
            self.check_length(Some((buf.len() + chunk.len()) as u64))?;
            hasher.update(&chunk);
            buf.extend_from_slice(&chunk);
        }
        let actual = ContentDigest::from_hasher(hasher);
        if actual != *sha256 {
            return Err(AttemptFailure::Integrity { actual });
        }
        Ok(buf.freeze())
    }

    async fn fetch_to_file(
        &self,
        url: Url,
        sha256: &ContentDigest,
        dir: &Path,
    ) -> Result<(u64, TempPath), AttemptFailure> {
        let io = |e: std::io::Error| AttemptFailure::Other(format!("local file: {e}"));
        let resp = self.get(url).await?;
        self.check_length(resp.content_length())?;
        let (file, path) = tempfile::Builder::new()
            .prefix(".walrus-")
            .tempfile_in(dir)
            .map_err(io)?
            .into_parts();
        let mut file = tokio::fs::File::from_std(file);
        let mut body = resp.bytes_stream();
        let mut hasher = Sha256::new();
        let mut len = 0u64;
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| AttemptFailure::Other(format!("body: {e}")))?;
            len += chunk.len() as u64;
            self.check_length(Some(len))?;
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(io)?;
        }
        let actual = ContentDigest::from_hasher(hasher);
        if actual != *sha256 {
            return Err(AttemptFailure::Integrity { actual });
        }
        file.sync_all().await.map_err(io)?;
        Ok((len, path))
    }

    async fn fetch_range(&self, url: Url, range: Range<u64>) -> Result<Bytes, AttemptFailure> {
        let want = range.end - range.start;
        let resp = self
            .http
            .get(url)
            .header(
                reqwest::header::RANGE,
                format!("bytes={}-{}", range.start, range.end - 1),
            )
            .timeout(self.timeout)
            .send()
            .await
            .map_err(|e| AttemptFailure::Other(format!("request: {e}")))?;
        let resp = check_status(resp)?;
        if resp.status() == StatusCode::PARTIAL_CONTENT {
            self.check_length(resp.content_length())?;
            let body = resp
                .bytes()
                .await
                .map_err(|e| AttemptFailure::Other(format!("body: {e}")))?;
            if body.len() as u64 > want {
                return Err(AttemptFailure::Other(format!(
                    "206 with {} bytes for a {want}-byte range",
                    body.len()
                )));
            }
            return Ok(body);
        }
        // A 200 is the whole blob: bounded like any read, then sliced.
        let body = self.body_bounded(resp).await?;
        let len = body.len() as u64;
        let start = range.start.min(len) as usize;
        let end = range.end.min(len) as usize;
        Ok(body.slice(start..end))
    }
}

fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, AttemptFailure> {
    let status = resp.status();
    if status == StatusCode::NOT_FOUND {
        Err(AttemptFailure::NotFound)
    } else if status.is_success() {
        Ok(resp)
    } else {
        Err(AttemptFailure::Other(format!("HTTP {status}")))
    }
}

/// `base` with `segments` appended to its path, each percent-encoded as one
/// segment.
pub(crate) fn url_with(base: &Url, segments: &[&str]) -> Url {
    let mut url = base.clone();
    if let Ok(mut path) = url.path_segments_mut() {
        path.pop_if_empty().extend(segments);
    }
    url
}

fn blob_url(agg: &Url, id: &BlobId) -> Url {
    url_with(agg, &["v1", "blobs", id.as_str()])
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_documented_routes() {
        let id = BlobId::from_bytes([7; 32]);
        let agg = Url::parse("https://agg.example/").unwrap();
        assert_eq!(
            blob_url(&agg, &id).as_str(),
            format!("https://agg.example/v1/blobs/{id}")
        );
        let sub = Url::parse("https://agg.example/walrus").unwrap();
        assert_eq!(
            blob_url(&sub, &id).as_str(),
            format!("https://agg.example/walrus/v1/blobs/{id}")
        );
        assert_eq!(
            url_with(
                &agg,
                &["v1", "blobs", "by-quilt-id", id.as_str(), "dir/a b.json"]
            )
            .as_str(),
            format!("https://agg.example/v1/blobs/by-quilt-id/{id}/dir%2Fa%20b.json")
        );
    }
}
