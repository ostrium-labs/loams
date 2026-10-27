//! Adopting preview.2 (UP1 Task 5): the fingerprint and
//! `0000-adopt-preview-2`.
//!
//! The hermetic stack mirrors `tests/fixtures/preview2/` on the embedded Raft
//! metastore: the same namespace, collection, stream and graph, with the
//! fixture's collection manifest, graph catalog document and Durable store.
//! `preview2_tikv_dump_is_recognised` loads the TiKV dump itself (it needs
//! `LOAMS_TEST_PD`).

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use loams_common::meta::{Consistency, MetaStore, PointerCas, collection_pointer_key};
use loams_common::schema::{CollectionSchema, DynamicMapping, FieldKind, FieldSpec};
use loams_migrate::adopt::{Fingerprint, PREVIEW_2};
use loams_migrate::migrations::{AdoptPreview2, MIGRATIONS};
use loams_migrate::{Cx, Migration, RunOptions, Runner, UpgradePath};
use loams_store::Store as ObjectStore;
use support::Stack;
use tempfile::TempDir;

const V0_1_0: &str = "0.1.0";

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/preview2")
        .join(rel)
}

const MANIFEST: &str =
    "ns/1/collections/1/manifests/00000000000000000001-01M4Y13Q3TPM4CX1TNDC6F7FR7.pb";
const CATALOG: &str = "graphs/1/catalog/01M4Y143GPAFSNQGHQZ0XDV8JS.json";

/// A preview.2-shaped stack: metastore records, bucket objects, Durable.
struct Preview2 {
    stack: Stack,
    bucket_dir: TempDir,
    bucket: ObjectStore,
}

impl Preview2 {
    async fn new() -> Preview2 {
        let stack = Stack::new().await;
        let bucket_dir = TempDir::new().expect("bucket");
        let url = url_of(bucket_dir.path());
        let bucket = ObjectStore::from_url(&url, Vec::<(String, String)>::new()).expect("store");
        for path in [MANIFEST, CATALOG] {
            let bytes = std::fs::read(fixture(&format!("bucket/{path}"))).expect("fixture");
            bucket.put(path, Bytes::from(bytes)).await.expect("put");
        }
        let meta = &*stack.meta;
        let ns = meta.create_namespace("up1").await.expect("namespace");
        let schema = CollectionSchema::new(
            vec![FieldSpec {
                name: "tenant".to_string(),
                source_path: "tenant".to_string(),
                kind: FieldKind::Keyword,
                indexed: true,
                fast: true,
                ignore_malformed: false,
            }],
            Vec::new(),
            DynamicMapping::Ignore,
        );
        let (cid, _, _) = meta
            .create_collection(ns, "docs", schema, 4)
            .await
            .expect("collection");
        cas(meta, ns, &collection_pointer_key(cid), MANIFEST).await;
        cas(meta, ns, "graph-catalog", CATALOG).await;
        meta.ensure_graph_stream(ns, "gr_01M4Y143G5T5EV2ZJDG3T46DDH")
            .await
            .expect("graph stream");
        let durable = stack.data.path().join("durable");
        std::fs::create_dir_all(&durable).expect("mkdir");
        std::fs::copy(fixture("durable/default.db"), durable.join("default.db")).expect("copy db");
        Preview2 {
            stack,
            bucket_dir,
            bucket,
        }
    }

    fn cx(&self, declared: Option<&str>) -> Cx {
        let mut cx = self.stack.cx();
        cx.bucket = Some(self.bucket.clone());
        cx.declared_release = declared.map(str::to_string);
        cx
    }
}

fn url_of(dir: &Path) -> String {
    format!(
        "file://{}/",
        dir.canonicalize().expect("canonical").display()
    )
}

async fn cas(meta: &dyn MetaStore, ns: loams_common::NamespaceId, key: &str, value: &str) {
    meta.cas_pointer(PointerCas {
        namespace: ns,
        key: key.to_string(),
        expected: None,
        value: value.to_string(),
        fence: None,
        fresh: None,
    })
    .await
    .into_result()
    .expect("pointer");
}

#[tokio::test]
async fn preview2_fixture_is_recognised() {
    let p2 = Preview2::new().await;
    let found = Fingerprint::check(&p2.cx(None), Some(PREVIEW_2))
        .await
        .expect("recognised");
    assert_eq!(found.namespaces, 1);
    assert_eq!(found.collections, 1);
    assert_eq!(found.manifests_opened, 1);
    assert!(found.durable_checked);
    assert!(
        found
            .data_markers
            .iter()
            .any(|m| m.starts_with("stream _graph.")),
        "{:?}",
        found.data_markers
    );
}

#[tokio::test]
async fn preview1_fixture_is_refused() {
    // preview.1's shape: a graph stream under its legacy name `graph.<gr>`.
    let p1 = Preview2::new().await;
    let ns = p1
        .stack
        .meta
        .namespace_by_name(Consistency::Linearizable, "up1")
        .await
        .expect("read")
        .expect("ns");
    p1.stack
        .meta
        .create_stream(
            ns.id,
            "graph.gr_01M4Y0000000000000000000AA",
            1,
            loams_common::meta::WalClass::Standard,
            loams_common::meta::Retention::default(),
        )
        .await
        .expect("preview.1 named graph streams this way");
    let err = Fingerprint::check(&p1.cx(None), Some(PREVIEW_2))
        .await
        .unwrap_err();
    assert_eq!(err.store, "meta");
    assert!(err.detail.contains("preview.1"), "{err}");
    // Declared as preview.1: never adopted.
    let err = Fingerprint::check(&p1.cx(None), Some("0.1.0-preview.1"))
        .await
        .unwrap_err();
    assert_eq!(err.store, "host");
}

#[tokio::test]
async fn decodable_store_without_marker_is_refused() {
    let p2 = Preview2::new().await;
    let err = Fingerprint::check(&p2.cx(None), None).await.unwrap_err();
    assert_eq!(err.store, "host", "{err}");
    assert!(err.to_string().starts_with("stack_unrecognised"), "{err}");
}

#[tokio::test]
async fn declared_preview2_with_decodable_data_is_adopted() {
    let p2 = Preview2::new().await;
    let migrations: [&dyn Migration; 1] = [&AdoptPreview2];
    let train = UpgradePath::compiled();
    let report = Runner::new(&migrations, train)
        .run(
            &mut p2.cx(Some(PREVIEW_2)),
            V0_1_0,
            &RunOptions::new("migrate-adopt"),
        )
        .await
        .expect("adopted and migrated");
    assert_eq!(report.from, None);
    assert_eq!(report.applied, ["0000-adopt-preview-2"]);
    let ledger = p2.stack.ledger().await;
    assert_eq!(
        ledger
            .stack()
            .await
            .expect("stack")
            .expect("stamped")
            .release,
        V0_1_0
    );
    let entries = ledger.entries().await.expect("entries");
    assert_eq!(entries[0].id, "0000-adopt-preview-2");
    assert_eq!(entries[0].release_from, PREVIEW_2);
    // The compiled list starts with the adoption.
    assert_eq!(MIGRATIONS[0].id(), "0000-adopt-preview-2");
    // Without a declared release the run refuses and marks it failed.
    let other = Preview2::new().await;
    let err = Runner::new(&migrations, train)
        .run(
            &mut other.cx(None),
            V0_1_0,
            &RunOptions::new("migrate-adopt"),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), "migration_failed", "{err}");
    assert!(err.to_string().contains("stack_unrecognised"), "{err}");
}

#[tokio::test]
async fn corrupt_pointer_envelope_is_refused() {
    let p2 = Preview2::new().await;
    let mut bytes = std::fs::read(fixture(&format!("bucket/{MANIFEST}"))).expect("fixture");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    p2.bucket
        .put(MANIFEST, Bytes::from(bytes))
        .await
        .expect("overwrite");
    let err = Fingerprint::check(&p2.cx(None), Some(PREVIEW_2))
        .await
        .unwrap_err();
    assert_eq!(err.store, "bucket");
    assert!(err.detail.contains("crc mismatch"), "{err}");
}

#[tokio::test]
async fn adoption_is_idempotent() {
    let p2 = Preview2::new().await;
    let migrations: [&dyn Migration; 1] = [&AdoptPreview2];
    let runner = Runner::new(&migrations, UpgradePath::compiled());
    let opts = RunOptions::new("migrate-adopt");
    runner
        .run(&mut p2.cx(Some(PREVIEW_2)), V0_1_0, &opts)
        .await
        .expect("first");
    let entries = p2.stack.ledger().await.entries().await.expect("entries");
    let again = runner
        .run(&mut p2.cx(Some(PREVIEW_2)), V0_1_0, &opts)
        .await
        .expect("again");
    assert!(again.applied.is_empty() && !again.stack_written);
    assert_eq!(
        p2.stack.ledger().await.entries().await.expect("entries"),
        entries
    );
    // The fingerprint itself is read-only: checking twice agrees.
    let a = Fingerprint::check(&p2.cx(None), Some(PREVIEW_2)).await;
    let b = Fingerprint::check(&p2.cx(None), Some(PREVIEW_2)).await;
    assert_eq!(a, b);
}

/// Every pointer of every namespace, by namespace name.
async fn all_pointers(meta: &dyn MetaStore) -> Vec<(String, String, u64, String)> {
    let mut out = Vec::new();
    for ns in meta
        .namespaces(Consistency::Linearizable)
        .await
        .expect("namespaces")
    {
        for (key, p) in meta
            .pointers_with_prefix(Consistency::Linearizable, ns.id, "")
            .await
            .expect("pointers")
        {
            out.push((ns.name.clone(), key, p.version, p.value));
        }
    }
    out
}

#[tokio::test]
async fn adoption_writes_only_system_upgrade_keys() {
    let p2 = Preview2::new().await;
    let meta = &*p2.stack.meta;
    let before = all_pointers(meta).await;
    let streams = meta
        .streams(Consistency::Linearizable, None)
        .await
        .expect("streams");
    let collections = meta
        .collections(Consistency::Linearizable, None)
        .await
        .expect("collections");
    let files = support::read_dir(p2.bucket_dir.path());
    let migrations: [&dyn Migration; 1] = [&AdoptPreview2];
    Runner::new(&migrations, UpgradePath::compiled())
        .run(
            &mut p2.cx(Some(PREVIEW_2)),
            V0_1_0,
            &RunOptions::new("migrate-adopt"),
        )
        .await
        .expect("adopt");
    let after = all_pointers(meta).await;
    let added: Vec<_> = after.iter().filter(|p| !before.contains(p)).collect();
    assert!(before.iter().all(|p| after.contains(p)), "nothing changed");
    assert!(
        added
            .iter()
            .all(|(ns, key, _, _)| ns == "_system" && key.starts_with("upgrade/")),
        "{added:?}"
    );
    // The run's only new stream is Loams Meter's `_system/_meter` (ME1 Task 4).
    let (meter, others): (Vec<_>, Vec<_>) = meta
        .streams(Consistency::Linearizable, None)
        .await
        .expect("streams")
        .into_iter()
        .partition(|s| s.name == loams_common::meta::METER_STREAM);
    assert_eq!(others, streams);
    assert_eq!(meter.len(), 1, "{meter:?}");
    assert_eq!(
        meta.collections(Consistency::Linearizable, None)
            .await
            .expect("collections"),
        collections
    );
    assert_eq!(support::read_dir(p2.bucket_dir.path()), files);
}

#[tokio::test]
async fn preview2_reads_after_adoption() {
    // What preview.2's decoders read back is unchanged: the pointers it
    // knows, and the manifest and catalog objects they name, byte for byte.
    let p2 = Preview2::new().await;
    let meta: Arc<dyn MetaStore> = p2.stack.meta.clone();
    let ns = meta
        .namespace_by_name(Consistency::Linearizable, "up1")
        .await
        .expect("read")
        .expect("ns");
    let before = meta
        .pointers_with_prefix(Consistency::Linearizable, ns.id, "")
        .await
        .expect("pointers");
    let migrations: [&dyn Migration; 1] = [&AdoptPreview2];
    Runner::new(&migrations, UpgradePath::compiled())
        .run(
            &mut p2.cx(Some(PREVIEW_2)),
            V0_1_0,
            &RunOptions::new("migrate-adopt"),
        )
        .await
        .expect("adopt");
    let after = meta
        .pointers_with_prefix(Consistency::Linearizable, ns.id, "")
        .await
        .expect("pointers");
    assert_eq!(after, before);
    for (_, pointer) in &after {
        let (bytes, _) = p2.bucket.get(&pointer.value).await.expect("object");
        let original =
            std::fs::read(fixture(&format!("bucket/{}", pointer.value))).expect("fixture");
        assert_eq!(bytes.as_ref(), original.as_slice(), "{}", pointer.value);
    }
    // And the fingerprint still recognises the stack it adopted.
    Fingerprint::check(&p2.cx(None), Some(PREVIEW_2))
        .await
        .expect("still preview.2-shaped");
}

#[tokio::test]
async fn system_namespace_with_user_data_is_refused() {
    let p2 = Preview2::new().await;
    let sys = p2
        .stack
        .meta
        .create_namespace("_system")
        .await
        .expect("_system");
    cas(
        &*p2.stack.meta,
        sys,
        "principal-directory",
        "system/directory/x.json",
    )
    .await;
    Fingerprint::check(&p2.cx(None), Some(PREVIEW_2))
        .await
        .expect("the auth directory is preview.2's own");
    cas(&*p2.stack.meta, sys, "mine", "x").await;
    let err = Fingerprint::check(&p2.cx(None), Some(PREVIEW_2))
        .await
        .unwrap_err();
    assert!(err.detail.contains("mine"), "{err}");
}

/// The real preview.2 dump (`tests/fixtures/preview2/loams_meta.jsonl`),
/// written into a TiKV test keyspace under a fresh root and read through
/// `loams-meta-tikv`, with the fixture's bucket objects and Durable store.
/// Needs a cluster (`LOAMS_TEST_PD`); skipped otherwise.
#[tokio::test]
async fn preview2_tikv_dump_is_recognised() {
    use loams_tikv::{TxnOptions, testing};
    let Some(cluster) = testing::cluster().await else {
        return;
    };
    let config = cluster.config(testing::TEST_META);
    let tikv = loams_tikv::Tikv::connect(config.clone())
        .await
        .expect("connect");
    let dump = std::fs::read_to_string(fixture("loams_meta.jsonl")).expect("dump");
    let pairs: Vec<(Vec<u8>, Vec<u8>)> = dump
        .lines()
        .map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).expect("json line");
            let hex = |field: &str| {
                let text = v[field].as_str().expect("hex");
                (0..text.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex digit"))
                    .collect::<Vec<u8>>()
            };
            (hex("k"), hex("v"))
        })
        .collect();
    for chunk in pairs.chunks(50) {
        let chunk = chunk.to_vec();
        tikv.run(TxnOptions::new("test.load_dump"), |txn| {
            let chunk = chunk.clone();
            Box::pin(async move {
                for (k, v) in chunk {
                    txn.put(&k, v).await?;
                }
                Ok(())
            })
        })
        .await
        .expect("load");
    }
    let meta: Arc<dyn MetaStore> = Arc::new(
        loams_meta_tikv::TikvMeta::open(loams_meta_tikv::TikvMetaConfig::new(config))
            .await
            .expect("open"),
    );
    let bucket_dir = TempDir::new().expect("bucket");
    let bucket = ObjectStore::from_url(&url_of(bucket_dir.path()), Vec::<(String, String)>::new())
        .expect("store");
    for path in [MANIFEST, CATALOG] {
        let bytes = std::fs::read(fixture(&format!("bucket/{path}"))).expect("fixture");
        bucket.put(path, Bytes::from(bytes)).await.expect("put");
    }
    let data = TempDir::new().expect("data");
    std::fs::create_dir_all(data.path().join("durable")).expect("mkdir");
    std::fs::copy(
        fixture("durable/default.db"),
        data.path().join("durable/default.db"),
    )
    .expect("copy");
    let mut cx = Cx::new(meta);
    cx.bucket = Some(bucket);
    cx.data_dir = Some(data.path().to_path_buf());
    let found = Fingerprint::check(&cx, Some(PREVIEW_2))
        .await
        .expect("the real preview.2 dump is recognised");
    assert_eq!(found.namespaces, 1);
    assert_eq!(found.manifests_opened, 1);
    assert!(found.durable_checked);
}
