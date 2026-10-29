//! Adopting a preview.2 stack (design §54 §5.3, D926; UP1 Task 5, Q820's
//! default): preview.2 writes no stack record and no ledger, so 0.1.0's
//! `migrate` recognises it from what is on disk and records it as
//! `0.1.0-preview.2`.
//!
//! Adoption needs a **positive marker** (ruling T0-6): the release the
//! operator declares (`--declared-release 0.1.0-preview.2`, which the CLI
//! fills from the stack directory's `LOAMS_VERSION`, the desktop from
//! `.loams-stack-version`). Decoding alone never suffices, because preview.1
//! writes every format at the same version. A legacy graph stream
//! (`graph.<gr>`), which only preview.1 wrote, refuses adoption even when the
//! declared release says preview.2.
//!
//! What must hold besides the marker:
//! - every metastore record kind lists and decodes (namespaces, streams,
//!   links, collections, pointers), at preview.2's versions;
//! - `_system`, if present, holds only the auth directory's
//!   `principal-directory` pointer and `upgrade/*` pointers;
//! - every collection manifest a pointer names opens as an `OPCM` envelope at
//!   a version this release reads; every graph catalog document parses;
//! - the Durable SQLite store, if present, lists exactly Resonate's
//!   `1 initial` migration;
//! - the House data directory, if present, has `catalog.sqlite` with the
//!   `namespaces` and `tables` tables and no `FORMAT` file.

use std::path::Path;

use loams_common::meta::{
    COLLECTION_POINTER_PREFIX, Consistency, GRAPH_STREAM_PREFIX, SYSTEM_NAMESPACE, SYSTEM_STREAMS,
};

use crate::formats::{self, STAMP_FILE};
use crate::migration::Cx;

/// The release adoption records.
pub const PREVIEW_2: &str = "0.1.0-preview.2";

/// The auth directory's pointer in `_system` (`crates/loams/src/auth/directory.rs`).
const DIRECTORY_POINTER: &str = "principal-directory";
/// The graph catalog's pointer key.
const GRAPH_CATALOG_POINTER: &str = "graph-catalog";
/// A collection manifest's magic (`crates/loams-collection/src/manifest.rs`).
const MANIFEST_MAGIC: &[u8; 4] = b"OPCM";

/// A recognised preview.2 stack: what was looked at.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preview2 {
    pub namespaces: usize,
    pub streams: usize,
    pub collections: usize,
    pub manifests_opened: usize,
    /// Conditional data markers found (`_graph.<gr>` streams, the auth
    /// directory): they strengthen the declared release, never replace it.
    pub data_markers: Vec<String>,
    pub durable_checked: bool,
    pub house_checked: bool,
}

/// Why a store is not a recognised preview.2 stack (`stack_unrecognised`).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("stack_unrecognised: {store}: {detail}")]
pub struct Mismatch {
    pub store: &'static str,
    pub detail: String,
}

fn mismatch(store: &'static str, detail: impl Into<String>) -> Mismatch {
    Mismatch {
        store,
        detail: detail.into(),
    }
}

/// The preview.2 fingerprint.
#[derive(Debug)]
pub struct Fingerprint;

impl Fingerprint {
    /// Recognises a preview.2 stack, or says why not.
    pub async fn check(cx: &Cx, declared_release: Option<&str>) -> Result<Preview2, Mismatch> {
        if declared_release != Some(PREVIEW_2) {
            return Err(mismatch(
                "host",
                format!(
                    "adoption needs the stack's declared release to be {PREVIEW_2} (found {}); \
                     start a fresh stack, or pass --declared-release from the stack's LOAMS_VERSION",
                    declared_release.unwrap_or("none")
                ),
            ));
        }
        let mut found = Preview2::default();
        check_meta(cx, &mut found).await?;
        if let Some(dir) = &cx.data_dir {
            found.durable_checked = check_durable(&dir.join("durable").join("default.db"))?;
        }
        if let Some(dir) = &cx.house_dir {
            found.house_checked = check_house(dir)?;
        }
        Ok(found)
    }
}

async fn check_meta(cx: &Cx, found: &mut Preview2) -> Result<(), Mismatch> {
    let meta = &*cx.meta;
    let l = Consistency::Linearizable;
    let failed = |what: &str, e: &dyn std::fmt::Display| mismatch("meta", format!("{what}: {e}"));
    let namespaces = meta
        .namespaces(l)
        .await
        .map_err(|e| failed("namespaces", &e))?;
    let streams = meta
        .streams(l, None)
        .await
        .map_err(|e| failed("streams", &e))?;
    let links = meta.links(l, None).await.map_err(|e| failed("links", &e))?;
    let collections = meta
        .collections(l, None)
        .await
        .map_err(|e| failed("collections", &e))?;
    if let Some(legacy) = streams.iter().find(|s| s.name.starts_with("graph.gr_")) {
        return Err(mismatch(
            "meta",
            format!(
                "stream {:?} has preview.1's graph stream name; preview.1 stacks are not \
                 adopted (D923): start a fresh stack",
                legacy.name
            ),
        ));
    }
    for stream in streams
        .iter()
        .filter(|s| s.name.starts_with(GRAPH_STREAM_PREFIX))
    {
        found.data_markers.push(format!("stream {}", stream.name));
    }
    for ns in &namespaces {
        let pointers = meta
            .pointers_with_prefix(l, ns.id, "")
            .await
            .map_err(|e| failed("pointers", &e))?;
        if ns.name == SYSTEM_NAMESPACE {
            // The reserved system streams (`_system/_meter`, made by every
            // `migrate run` since 0.1.3) are Loams' own, not foreign data.
            let owns_objects = streams.iter().any(|s| {
                s.namespace == ns.id && !SYSTEM_STREAMS.iter().any(|(name, _)| *name == s.name)
            }) || links.iter().any(|k| k.namespace == ns.id)
                || collections.iter().any(|c| c.namespace == ns.id);
            let foreign = pointers
                .iter()
                .find(|(key, _)| key != DIRECTORY_POINTER && !key.starts_with("upgrade/"));
            if owns_objects || foreign.is_some() {
                return Err(mismatch(
                    "meta",
                    format!(
                        "namespace {SYSTEM_NAMESPACE} holds data Loams did not write there ({}); \
                         rename it with preview.2 first",
                        foreign.map_or("streams, links or collections", |(k, _)| k.as_str())
                    ),
                ));
            }
            if pointers.iter().any(|(key, _)| key == DIRECTORY_POINTER) {
                found.data_markers.push("auth directory".to_string());
            }
            continue;
        }
        for (key, pointer) in &pointers {
            if key.starts_with(COLLECTION_POINTER_PREFIX) {
                if let Some(bucket) = &cx.bucket {
                    open_manifest(bucket, &pointer.value).await?;
                    found.manifests_opened += 1;
                }
            } else if key == GRAPH_CATALOG_POINTER
                && let Some(bucket) = &cx.bucket
            {
                let (bytes, _) = bucket
                    .get(&pointer.value)
                    .await
                    .map_err(|e| mismatch("bucket", format!("{}: {e}", pointer.value)))?;
                let doc: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|e| mismatch("bucket", format!("{}: {e}", pointer.value)))?;
                if !doc.get("graphs").is_some_and(serde_json::Value::is_object) {
                    return Err(mismatch(
                        "bucket",
                        format!("{} is not a graph catalog document", pointer.value),
                    ));
                }
            }
        }
    }
    found.namespaces = namespaces
        .iter()
        .filter(|n| n.name != SYSTEM_NAMESPACE)
        .count();
    found.streams = streams.len();
    found.collections = collections.len();
    Ok(())
}

/// Opens a collection manifest's envelope (`OPCM ‖ u16 version ‖ body ‖
/// crc32c`) without decoding its body.
pub(crate) async fn open_manifest(bucket: &loams_store::Store, path: &str) -> Result<(), Mismatch> {
    let bad = |detail: String| mismatch("bucket", format!("{path}: {detail}"));
    let (bytes, _) = bucket.get(path).await.map_err(|e| bad(e.to_string()))?;
    if bytes.len() < 10 || &bytes[..4] != MANIFEST_MAGIC {
        return Err(bad("not a collection manifest".to_string()));
    }
    let (framed, crc) = bytes.split_at(bytes.len() - 4);
    if crc32c::crc32c(framed).to_le_bytes() != crc {
        return Err(bad("crc mismatch".to_string()));
    }
    let version = u16::from_le_bytes([framed[4], framed[5]]);
    formats::check(formats::COLLECTION_MANIFEST, version).map_err(|e| bad(e.to_string()))
}

/// The Durable store, when it exists, has exactly Resonate's `1 initial`.
fn check_durable(path: &Path) -> Result<bool, Mismatch> {
    if !path.exists() {
        return Ok(false);
    }
    let bad = |detail: String| mismatch("durable", format!("{}: {detail}", path.display()));
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| bad(e.to_string()))?;
    let mut stmt = conn
        .prepare("SELECT version, description, success FROM _sqlx_migrations ORDER BY version")
        .map_err(|e| bad(e.to_string()))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })
        .map_err(|e| bad(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| bad(e.to_string()))?;
    if rows != [(1, "initial".to_string(), true)] {
        return Err(bad(format!(
            "Resonate's migrations are {rows:?}, not preview.2's [1 initial]"
        )));
    }
    Ok(true)
}

/// The House directory, when it has a catalog: preview.2's layout.
fn check_house(dir: &Path) -> Result<bool, Mismatch> {
    let catalog = dir.join("catalog.sqlite");
    if !catalog.exists() {
        return Ok(false);
    }
    let bad = |detail: String| mismatch("house", format!("{}: {detail}", dir.display()));
    if dir.join(STAMP_FILE).exists() {
        return Err(bad(format!(
            "has a {STAMP_FILE} file, which preview.2 never writes"
        )));
    }
    let conn =
        rusqlite::Connection::open_with_flags(&catalog, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| bad(e.to_string()))?;
    for table in ["namespaces", "tables"] {
        let found: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|e| bad(e.to_string()))?;
        if found != 1 {
            return Err(bad(format!("catalog.sqlite has no {table} table")));
        }
    }
    Ok(true)
}
