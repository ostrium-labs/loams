//! Writes through a publisher (docs.wal.app "Client daemon mode & HTTP
//! API", "Quilt HTTP APIs"; the publisher's OpenAPI):
//!
//! - `PUT /v1/blobs?epochs=N[&permanent=true]` with the blob as the body.
//!   Blobs are deletable by default since Walrus v1.33; the old
//!   `deletable` flag is deprecated and never sent.
//! - `PUT /v1/quilts?epochs=N[&permanent=true]`, `multipart/form-data`
//!   with one file field per identifier.
//!
//! A blob store answers one of:
//!
//! ```json
//! {"newlyCreated": {"blobObject": {"id": "0x…", "blobId": "…", "size": 17,
//!   "storage": {"endEpoch": 35, …}, …}, "resourceOperation": {…}, "cost": 0}}
//! {"alreadyCertified": {"blobId": "…", "event": {…}, "endEpoch": 35}}
//! ```
//!
//! and a quilt store `{"blobStoreResult": <one of those>,
//! "storedQuiltBlobs": [{"identifier": "…", "quiltPatchId": "…"}]}`. The
//! publisher's `markedInvalid` and `error` answers are
//! [`WalrusError::Publisher`].
//!
//! Every write is read back through an aggregator, and its sha256 checked,
//! before it answers `Ok`.

use std::collections::BTreeSet;

use bytes::{BufMut, Bytes, BytesMut};
use serde::Deserialize;
use url::Url;

use crate::error::WalrusError;
use crate::read::url_with;
use crate::types::{BlobId, ContentDigest, QuiltPatchId, StoreOpts, Stored, StoredQuilt};
use crate::{Inner, Walrus};

impl Walrus {
    /// Stores `bytes` and reads it back verified. Refused before upload
    /// when over `max_blob_bytes` ([`WalrusError::TooLarge`]) or when no
    /// publisher is configured ([`WalrusError::NoPublisher`]). Storing the
    /// same bytes again answers `alreadyCertified` (`newly_created: false`)
    /// while the existing blob lives long enough.
    pub async fn store(&self, bytes: Bytes, opts: StoreOpts) -> Result<Stored, WalrusError> {
        let inner = self.inner()?;
        let publisher = inner.publisher.as_ref().ok_or(WalrusError::NoPublisher)?;
        let epochs = inner.epochs_of(opts)?;
        inner.check_size("blob", bytes.len() as u64)?;
        let sha256 = ContentDigest::of(&bytes);
        let size = bytes.len() as u64;
        let url = store_url(publisher, "blobs", epochs, opts.permanent);
        let body = inner.put(url, bytes, None).await?;
        let parsed: BlobStoreResult = serde_json::from_slice(&body)
            .map_err(|e| WalrusError::Publisher(format!("unexpected response: {e}")))?;
        let stored = parsed.into_stored(size)?;
        inner.note_write(stored.blob_id.as_str());
        self.read(&stored.blob_id, &sha256).await?;
        Ok(stored)
    }

    /// Stores `files` as one quilt, each under its identifier, and reads
    /// every patch back verified. Identifiers must be unique, non-empty,
    /// free of `"`, `\`, control characters, and not `_metadata`. Refused
    /// before upload when the files together are over `max_blob_bytes`.
    pub async fn store_quilt(
        &self,
        files: Vec<(String, Bytes)>,
        opts: StoreOpts,
    ) -> Result<StoredQuilt, WalrusError> {
        let inner = self.inner()?;
        let publisher = inner.publisher.as_ref().ok_or(WalrusError::NoPublisher)?;
        let epochs = inner.epochs_of(opts)?;
        if files.is_empty() {
            return Err(WalrusError::Config(
                "a quilt needs at least one file".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for (id, _) in &files {
            check_identifier(id)?;
            if !seen.insert(id.as_str()) {
                return Err(WalrusError::Config(format!("duplicate identifier {id:?}")));
            }
        }
        let total: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
        inner.check_size("quilt", total)?;
        let (content_type, body) = multipart(&files);
        let url = store_url(publisher, "quilts", epochs, opts.permanent);
        let resp = inner.put(url, body, Some(content_type)).await?;
        let parsed: QuiltStoreResult = serde_json::from_slice(&resp)
            .map_err(|e| WalrusError::Publisher(format!("unexpected response: {e}")))?;
        let blob = parsed.blob_store_result.into_stored(total)?;
        inner.note_write(blob.blob_id.as_str());
        let mut patches = Vec::with_capacity(parsed.stored_quilt_blobs.len());
        for p in parsed.stored_quilt_blobs {
            inner.note_write(p.quilt_patch_id.as_str());
            patches.push((p.identifier, p.quilt_patch_id));
        }
        for (id, data) in &files {
            let patch = patches
                .iter()
                .find(|(i, _)| i == id)
                .map(|(_, p)| p)
                .ok_or_else(|| {
                    WalrusError::Publisher(format!("quilt answer has no patch for {id:?}"))
                })?;
            self.read_patch(patch, &ContentDigest::of(data)).await?;
        }
        Ok(StoredQuilt { blob, patches })
    }
}

impl Inner {
    fn epochs_of(&self, opts: StoreOpts) -> Result<u32, WalrusError> {
        match opts.epochs.unwrap_or(self.epochs) {
            0 => Err(WalrusError::Config("epochs must be at least 1".into())),
            n => Ok(n),
        }
    }

    fn check_size(&self, what: &'static str, size: u64) -> Result<(), WalrusError> {
        if size > self.max_blob_bytes {
            return Err(WalrusError::TooLarge {
                what,
                size,
                max: self.max_blob_bytes,
            });
        }
        Ok(())
    }

    async fn put(
        &self,
        url: Url,
        body: Bytes,
        content_type: Option<String>,
    ) -> Result<Bytes, WalrusError> {
        let mut req = self.http.put(url).timeout(self.timeout).body(body);
        if let Some(ct) = content_type {
            req = req.header(reqwest::header::CONTENT_TYPE, ct);
        }
        if let Some(token) = &self.auth {
            req = req.bearer_auth(token.expose());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| WalrusError::Publisher(format!("request: {e}")))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| WalrusError::Publisher(format!("body: {e}")))?;
        if !status.is_success() {
            let text = String::from_utf8_lossy(&body[..body.len().min(512)]).into_owned();
            return Err(WalrusError::Publisher(format!("HTTP {status}: {text}")));
        }
        Ok(body)
    }
}

fn store_url(publisher: &Url, what: &str, epochs: u32, permanent: bool) -> Url {
    let mut url = url_with(publisher, &["v1", what]);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("epochs", &epochs.to_string());
        if permanent {
            q.append_pair("permanent", "true");
        }
    }
    url
}

fn check_identifier(id: &str) -> Result<(), WalrusError> {
    let bad = id.is_empty()
        || id == "_metadata"
        || id.chars().any(|c| c == '"' || c == '\\' || c.is_control());
    if bad {
        return Err(WalrusError::Config(format!(
            "quilt identifier {id:?} is empty, reserved or has a quote, backslash or control character"
        )));
    }
    Ok(())
}

/// A `multipart/form-data` body with one file field per identifier. The
/// boundary is derived from the content's sha256, so it cannot occur in it
/// short of a hash collision.
fn multipart(files: &[(String, Bytes)]) -> (String, Bytes) {
    let mut all = sha2::Sha256::default();
    for (id, data) in files {
        sha2::Digest::update(&mut all, id.as_bytes());
        sha2::Digest::update(&mut all, data);
    }
    let boundary = format!("loams-walrus-{}", ContentDigest::from_hasher(all).to_hex());
    let mut body = BytesMut::new();
    for (id, data) in files {
        body.put_slice(format!("--{boundary}\r\n").as_bytes());
        body.put_slice(
            format!(
                "Content-Disposition: form-data; name=\"{id}\"; filename=\"{id}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.put_slice(data);
        body.put_slice(b"\r\n");
    }
    body.put_slice(format!("--{boundary}--\r\n").as_bytes());
    (
        format!("multipart/form-data; boundary={boundary}"),
        body.freeze(),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum BlobStoreResult {
    #[serde(rename_all = "camelCase")]
    NewlyCreated { blob_object: BlobObject },
    #[serde(rename_all = "camelCase")]
    AlreadyCertified { blob_id: String, end_epoch: u64 },
    #[serde(rename_all = "camelCase")]
    MarkedInvalid { blob_id: String },
    #[serde(rename_all = "camelCase")]
    Error {
        #[serde(default)]
        blob_id: Option<String>,
        #[serde(default)]
        error_msg: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BlobObject {
    id: String,
    blob_id: String,
    #[serde(default)]
    size: Option<u64>,
    storage: Storage,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Storage {
    end_epoch: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuiltStoreResult {
    blob_store_result: BlobStoreResult,
    stored_quilt_blobs: Vec<StoredQuiltPatch>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredQuiltPatch {
    identifier: String,
    quilt_patch_id: QuiltPatchId,
}

impl BlobStoreResult {
    /// `size` is what was sent, used when the answer does not say.
    fn into_stored(self, size: u64) -> Result<Stored, WalrusError> {
        match self {
            Self::NewlyCreated { blob_object } => Ok(Stored {
                blob_id: BlobId::parse(&blob_object.blob_id)?,
                end_epoch: blob_object.storage.end_epoch,
                newly_created: true,
                size: blob_object.size.unwrap_or(size),
                sui_object_id: Some(blob_object.id),
            }),
            Self::AlreadyCertified { blob_id, end_epoch } => Ok(Stored {
                blob_id: BlobId::parse(&blob_id)?,
                end_epoch,
                newly_created: false,
                size,
                sui_object_id: None,
            }),
            Self::MarkedInvalid { blob_id } => Err(WalrusError::Publisher(format!(
                "blob {blob_id} is marked invalid"
            ))),
            Self::Error { blob_id, error_msg } => Err(WalrusError::Publisher(format!(
                "store of {} failed: {}",
                blob_id.as_deref().unwrap_or("?"),
                error_msg.as_deref().unwrap_or("no message")
            ))),
        }
    }
}
