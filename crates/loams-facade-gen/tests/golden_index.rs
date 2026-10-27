//! `sdks/fixtures/facade-index.json`, over the real `proto/` tree (SDKP P0a's
//! `golden_index`).
//!
//! The index is what every hand-written SDK checks its facade against, so the
//! committed file must be exactly what `lang=index` renders from `proto/`: a
//! proto edit that changes the SDK surface fails here until
//! `scripts/sdk/gen.sh index` is run and the result committed.

use std::path::PathBuf;

use loams_facade_gen::{index, model_from_request, reasons};

mod protoreq;

fn render() -> String {
    let root = protoreq::root();
    let reasons = reasons::read(&root.join("docs/api/reasons.md")).expect("the reason registry");
    let model =
        model_from_request(protoreq::code_generator_request(), reasons).expect("the descriptors");
    index::render(&model, "v1")
}

fn committed() -> PathBuf {
    protoreq::root().join("sdks/fixtures/facade-index.json")
}

#[test]
fn golden_index() {
    let rendered = render();
    let path = committed();
    let expected =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    assert_eq!(
        rendered,
        expected,
        "{} is stale: run scripts/sdk/gen.sh index and commit the result",
        path.display()
    );
}

/// The app modules SDKP P0a annotates, with the retry classes and pagination
/// the plan's §2 tables give.
#[test]
fn the_index_has_the_app_modules() {
    let rendered = render();
    for expected in [
        r#"{"module": "namespaces", "call": "create", "rpc": "loams.collection.v1.NamespaceService/CreateNamespace", "retry": "safe""#,
        r#"{"module": "namespaces", "call": "list", "rpc": "loams.collection.v1.NamespaceService/ListNamespaces", "retry": "safe", "idempotencyKeyField": "", "pagination": {"items": "namespaces", "nextPageToken": "next_page_token"}"#,
        r#"{"module": "collections", "call": "create", "rpc": "loams.collection.v1.CollectionService/CreateCollection", "retry": "safe""#,
        r#"{"module": "documents", "call": "write", "rpc": "loams.collection.v1.DocumentService/WriteDocuments", "retry": "keyed", "idempotencyKeyField": "idempotency_key""#,
        r#"{"module": "documents", "call": "delete_by_filter", "rpc": "loams.collection.v1.DocumentService/DeleteByFilter", "retry": "never""#,
        r#"{"module": "search", "call": "query", "rpc": "loams.collection.v1.QueryService/Search", "retry": "safe""#,
        r#"{"module": "vector", "call": "search", "rpc": "loams.collection.v1.QueryService/Search", "retry": "safe""#,
        r#"{"module": "graph", "call": "query", "rpc": "loams.graph.v1.GraphService/Execute", "retry": "keyed""#,
        r#"{"module": "graph", "call": "query_stream", "rpc": "loams.graph.v1.GraphService/ExecuteStream", "retry": "never", "idempotencyKeyField": "", "pagination": null, "stream": true, "unstable": true}"#,
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
    // A method with no facade option stays out of every SDK.
    assert!(!rendered.contains("GetEngineInfo"));
    assert!(!rendered.contains("RestoreGraph"));
}
