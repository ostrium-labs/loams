//! SDKP Part A for Rust: the token corpus, the session, the runtime clauses
//! R11–R14, the collection handle and `query_all`, each pinned against a stub
//! so no server is needed. The canonical names `check-languages.mjs` looks
//! for are the test functions' names.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use loams::builders::{Filter, Search, Sort, TextRetriever, VectorOptions};
use loams::collection_v1::{
    CollectionInfo, CountDocumentsRequest, CountDocumentsResponse, DropCollectionRequest,
    DropCollectionResponse, GetCollectionRequest, GetDocumentsRequest, GetDocumentsResponse, Hit,
    ScrollDocumentsRequest, ScrollDocumentsResponse, SearchRequest, SearchResponse,
    WriteDocumentsRequest, WriteDocumentsResponse,
};
use loams::{
    BatchOptions, CallOptionsOverrides, Code, CollectionCalls, CollectionHandle,
    ConsistencySession, ConsistencyToken, DocId, Document, ErrorKind, LoamsError, OffsetRecord,
    PatchChange, RetryPlan, call_with_retry, prepare_request, record_response,
};
use serde_json::{Value, json};

fn tokens_corpus() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/tokens.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the token corpus")).expect("JSON")
}

/// `rust_consistency_token_merge`: the token corpus, parse, print, merge and
/// the malformed cases.
#[test]
fn rust_consistency_token_merge() {
    let corpus = tokens_corpus();
    for case in corpus["parse"].as_array().expect("parse cases") {
        let text = case["text"].as_str().expect("text");
        let token = ConsistencyToken::parse(text).expect(text);
        let items: Vec<Value> = token
            .0
            .iter()
            .map(|p| json!([p.stream.to_string(), p.partition, p.offset.to_string()]))
            .collect();
        assert_eq!(Value::Array(items), case["items"], "{text}");
        assert_eq!(token.to_string(), text);
    }
    for case in corpus["merge"].as_array().expect("merge cases") {
        let a = ConsistencyToken::parse(case["a"].as_str().expect("a")).expect("a parses");
        let b = ConsistencyToken::parse(case["b"].as_str().expect("b")).expect("b parses");
        let expect = case["expect"].as_str().expect("expect");
        assert_eq!(a.merge(&b).to_string(), expect, "{}", case["name"]);
        assert_eq!(b.merge(&a).to_string(), expect, "{} commutes", case["name"]);
    }
    for bad in corpus["malformed"].as_array().expect("malformed") {
        let bad = bad.as_str().expect("a string");
        let error = ConsistencyToken::parse(bad).expect_err(bad);
        assert_eq!(error.kind, ErrorKind::InvalidArgument, "{bad}");
        assert_eq!(
            error.client_reason(),
            Some("invalid_consistency_token"),
            "{bad}"
        );
    }
}

/// `rust_session_read_your_writes`: a write's token merges into the session,
/// and the next read with a `consistency` field is sent `at_least` it.
#[test]
fn rust_session_read_your_writes() {
    let session = ConsistencySession::new();
    let write = WriteDocumentsResponse {
        token: "v1:s4/p0@12".to_owned(),
        ..Default::default()
    };
    record_response(&session, &http::HeaderMap::new(), &write);
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "loams-consistency-token",
        http::HeaderValue::from_static("v1:s4/p1@3"),
    );
    record_response(&session, &headers, &WriteDocumentsResponse::default());
    assert_eq!(session.current().as_deref(), Some("v1:s4/p0@12,s4/p1@3"));

    let mut read = GetDocumentsRequest {
        collection: "notes".to_owned(),
        ..Default::default()
    };
    prepare_request(&mut read, Some("app"), session.current().as_deref(), "rpc").expect("prepared");
    assert_eq!(
        read.consistency.as_option().map(|c| c.at_least.as_str()),
        Some("v1:s4/p0@12,s4/p1@3")
    );

    // An explicit consistency wins.
    let mut explicit: SearchRequest = serde_json::from_value(
        json!({"collection": "notes", "consistency": {"freshness": "FRESHNESS_EVENTUAL"}}),
    )
    .expect("a request");
    prepare_request(
        &mut explicit,
        Some("app"),
        session.current().as_deref(),
        "rpc",
    )
    .expect("prepared");
    assert_eq!(
        explicit
            .consistency
            .as_option()
            .map(|c| c.at_least.as_str()),
        Some("")
    );

    // The graph spelling is a oneof arm.
    let mut graph = loams::graph_v1::ExecuteRequest::default();
    prepare_request(&mut graph, Some("app"), Some("v1:s1/p0@1"), "rpc").expect("prepared");
    let json = serde_json::to_value(&graph).expect("serialises");
    assert_eq!(json["consistency"]["atLeast"], "v1:s1/p0@1");

    // One token per package, and graph reads never take the session's
    // (SDKP-17): a graph token names one graph.
    let scoped = ConsistencySession::new();
    scoped.record_in(Some("v1:s1/p0@4"), "loams.collection.v1");
    scoped.record_in(Some("v1:s2/p0@9"), "loams.graph.v1");
    assert_eq!(
        loams::app::session_token_for(Some(&scoped), "loams.collection.v1").as_deref(),
        Some("v1:s1/p0@4")
    );
    assert_eq!(
        loams::app::session_token_for(Some(&scoped), "loams.graph.v1"),
        None
    );

    // A string that is not a `v1:` token is not the session's business.
    record_response(
        &session,
        &http::HeaderMap::new(),
        &json!({"token": "page-2"}),
    );
    assert_eq!(session.rejected(), 0);
}

/// `rust_default_namespace` (R11).
#[test]
fn rust_default_namespace() {
    let mut filled = GetCollectionRequest {
        collection: "notes".to_owned(),
        ..Default::default()
    };
    prepare_request(&mut filled, Some("app"), None, "rpc").expect("filled");
    assert_eq!(filled.namespace, "app");

    let mut explicit = GetCollectionRequest {
        namespace: "other".to_owned(),
        ..Default::default()
    };
    prepare_request(&mut explicit, Some("app"), None, "rpc").expect("kept");
    assert_eq!(explicit.namespace, "other");

    let mut neither = GetCollectionRequest::default();
    let error = prepare_request(
        &mut neither,
        None,
        None,
        "loams.collection.v1.CollectionService/GetCollection",
    )
    .expect_err("refused");
    assert_eq!(error.kind, ErrorKind::InvalidArgument);
    assert_eq!(
        error.metadata().get("field").map(String::as_str),
        Some("namespace")
    );

    let loams = loams::Loams::from_env_with(|name| match name {
        "LOAMS_ENDPOINT" => Some("http://127.0.0.1:1".to_owned()),
        "LOAMS_NAMESPACE" => Some("env".to_owned()),
        _ => None,
    })
    .expect("a client");
    assert_eq!(loams.namespace(), Some("env"));
    assert!(loams::Loams::from_env_with(|_| None).is_err());
    let with = loams.with_namespace("app");
    assert_eq!(with.namespace(), Some("app"));
}

fn plan(timeout: Duration, retries: u32) -> RetryPlan {
    RetryPlan {
        retry_safe: true,
        max_retries: retries,
        token_source: None,
        headers: Vec::new(),
        consistency: None,
        session: None,
        timeout: Some(timeout),
        random: || 0.0,
    }
}

/// `rust_timeout_deadline` (R12).
#[tokio::test]
async fn rust_timeout_deadline() {
    let attempts = AtomicUsize::new(0);
    let started = std::time::Instant::now();
    let error = call_with_retry(
        (),
        |(), _attempt| {
            attempts.fetch_add(1, Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<(), connectrpc::ConnectError>(())
            }
        },
        &plan(Duration::from_millis(40), 3),
        "loams.collection.v1.DocumentService/GetDocuments",
    )
    .await
    .expect_err("ran out");
    assert_eq!(error.code, Code::DeadlineExceeded);
    assert!(error.is_client_deadline_exceeded());
    assert_eq!(error.client_reason(), Some(loams::CLIENT_DEADLINE_EXCEEDED));
    assert_eq!(attempts.load(Ordering::SeqCst), 1, "never retried");
    assert!(started.elapsed() < Duration::from_secs(5));

    // A server's `deadline_exceeded` inside the budget is retried as usual.
    let attempts = AtomicUsize::new(0);
    let answer = call_with_retry(
        (),
        |(), _attempt| {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Err(connectrpc::ConnectError::new(
                        Code::DeadlineExceeded,
                        "server deadline",
                    ))
                } else {
                    Ok(7)
                }
            }
        },
        &plan(Duration::from_secs(5), 3),
        "rpc",
    )
    .await
    .expect("retried");
    assert_eq!(answer, 7);

    // A zero budget is no deadline.
    let answer = call_with_retry(
        (),
        |(), _| async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Ok::<u8, connectrpc::ConnectError>(1)
        },
        &plan(Duration::ZERO, 0),
        "rpc",
    )
    .await
    .expect("no deadline");
    assert_eq!(answer, 1);
    assert_eq!(loams::DEFAULT_TIMEOUT, Duration::from_secs(30));
}

/// A stub of the app calls that records every request and answers from a
/// script.
#[derive(Default)]
struct Stub {
    calls: Mutex<Vec<(String, Value)>>,
    write_answers: Mutex<VecDeque<Result<WriteDocumentsResponse, LoamsError>>>,
    search_pages: Mutex<VecDeque<Vec<u64>>>,
    scroll_pages: Mutex<VecDeque<(Vec<u64>, Option<u64>)>>,
    keys: Mutex<Vec<String>>,
}

impl Stub {
    fn record<T: serde::Serialize>(&self, name: &str, request: &T) {
        self.calls.lock().expect("not poisoned").push((
            name.to_owned(),
            serde_json::to_value(request).expect("serialises"),
        ));
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().expect("not poisoned").clone()
    }
}

fn doc_id_json(id: u64) -> Value {
    json!({"uint": id.to_string()})
}

#[async_trait(?Send)]
impl CollectionCalls for Stub {
    async fn write_documents(
        &self,
        request: WriteDocumentsRequest,
        options: CallOptionsOverrides,
    ) -> Result<WriteDocumentsResponse, LoamsError> {
        self.record("write", &request);
        if let Some(key) = options.idempotency_key {
            self.keys.lock().expect("not poisoned").push(key);
        }
        self.write_answers
            .lock()
            .expect("not poisoned")
            .pop_front()
            .unwrap_or_else(|| Ok(WriteDocumentsResponse::default()))
    }

    async fn get_documents(
        &self,
        request: GetDocumentsRequest,
        _: CallOptionsOverrides,
    ) -> Result<GetDocumentsResponse, LoamsError> {
        self.record("get", &request);
        Ok(GetDocumentsResponse::default())
    }

    async fn scroll_documents(
        &self,
        request: ScrollDocumentsRequest,
        _: CallOptionsOverrides,
    ) -> Result<ScrollDocumentsResponse, LoamsError> {
        self.record("scroll", &request);
        let (ids, next) = self
            .scroll_pages
            .lock()
            .expect("not poisoned")
            .pop_front()
            .unwrap_or_default();
        let documents: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": doc_id_json(*id)}))
            .collect();
        let mut page = json!({ "documents": documents });
        if let Some(next) = next {
            page["next"] = doc_id_json(next);
        }
        Ok(serde_json::from_value(page).expect("a page"))
    }

    async fn count_documents(
        &self,
        request: CountDocumentsRequest,
        _: CallOptionsOverrides,
    ) -> Result<CountDocumentsResponse, LoamsError> {
        self.record("count", &request);
        Ok(CountDocumentsResponse::default())
    }

    async fn search(
        &self,
        request: SearchRequest,
        _: CallOptionsOverrides,
    ) -> Result<SearchResponse, LoamsError> {
        self.record("search", &request);
        let pks = self
            .search_pages
            .lock()
            .expect("not poisoned")
            .pop_front()
            .unwrap_or_default();
        let hits: Vec<Value> = pks
            .iter()
            .map(|pk| json!({"pk": doc_id_json(*pk), "sortValues": [1.0, pk]}))
            .collect();
        Ok(serde_json::from_value(json!({ "hits": hits })).expect("a response"))
    }

    async fn get_collection(
        &self,
        request: GetCollectionRequest,
        _: CallOptionsOverrides,
    ) -> Result<CollectionInfo, LoamsError> {
        self.record("info", &request);
        Ok(CollectionInfo::default())
    }

    async fn drop_collection(
        &self,
        request: DropCollectionRequest,
        _: CallOptionsOverrides,
    ) -> Result<DropCollectionResponse, LoamsError> {
        self.record("drop", &request);
        Ok(DropCollectionResponse::default())
    }
}

fn docs(n: u64) -> Vec<Document> {
    (1..=n)
        .map(|id| Document::new(id).source(json!({"title": format!("note {id}")})))
        .collect()
}

/// `rust_write_batch_chunks` (R13).
#[tokio::test]
async fn rust_write_batch_chunks() {
    let stub = Stub::default();
    for (n, partition) in [(10, 1), (20, 0), (30, 1)] {
        stub.write_answers
            .lock()
            .expect("not poisoned")
            .push_back(Ok(WriteDocumentsResponse {
                token: format!("v1:s1/p{partition}@{n}"),
                ..Default::default()
            }));
    }
    let notes = CollectionHandle::new(&stub, "notes", None);
    let result = notes
        .write_batch(
            docs(5),
            BatchOptions {
                chunk_rows: 2,
                ..BatchOptions::default()
            },
            CallOptionsOverrides::new(),
        )
        .await
        .expect("written");
    assert_eq!(result.chunks, 3);
    assert_eq!(result.rows, 5);
    assert_eq!(
        result.token.map(|t| t.to_string()).as_deref(),
        Some("v1:s1/p0@20,s1/p1@30")
    );
    let sizes: Vec<usize> = stub
        .calls()
        .iter()
        .map(|(_, request)| request["ops"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, [2, 2, 1]);
    let keys = stub.keys.lock().expect("not poisoned").clone();
    assert_eq!(keys.len(), 3);
    assert_eq!(
        keys.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3,
        "one key per chunk"
    );

    // Chunking by bytes.
    let stub = Stub::default();
    let big: Vec<Document> = (1..=3)
        .map(|id| Document::new(id).source(json!({"body": "x".repeat(400)})))
        .collect();
    CollectionHandle::new(&stub, "notes", None)
        .write_batch(
            big,
            BatchOptions {
                chunk_bytes: 1_000,
                ..BatchOptions::default()
            },
            CallOptionsOverrides::new(),
        )
        .await
        .expect("written");
    assert_eq!(stub.calls().len(), 2);

    // The first failed chunk stops the load.
    let stub = Stub::default();
    {
        let mut answers = stub.write_answers.lock().expect("not poisoned");
        answers.push_back(Ok(WriteDocumentsResponse {
            token: "v1:s1/p0@5".to_owned(),
            ..Default::default()
        }));
        answers.push_back(Err(LoamsError::new(
            ErrorKind::InvalidArgument,
            Code::InvalidArgument,
            "bad doc",
        )));
    }
    let error = CollectionHandle::new(&stub, "notes", None)
        .write_batch(
            docs(6),
            BatchOptions {
                chunk_rows: 2,
                ..BatchOptions::default()
            },
            CallOptionsOverrides::new(),
        )
        .await
        .expect_err("stopped");
    assert_eq!(error.chunks_applied, 1);
    assert_eq!(error.rows_applied, 2);
    assert_eq!(
        error.token.as_ref().map(ToString::to_string).as_deref(),
        Some("v1:s1/p0@5")
    );
    assert_eq!(error.source.code, Code::InvalidArgument);
    assert_eq!(error.reason(), loams::BATCH_PARTIALLY_APPLIED);
    assert_eq!(stub.calls().len(), 2, "nothing after the failed chunk");
    let as_loams: LoamsError = error.into();
    assert_eq!(
        as_loams.client_reason(),
        Some(loams::BATCH_PARTIALLY_APPLIED)
    );
}

#[derive(Debug)]
struct Rec(u64);

impl OffsetRecord for Rec {
    fn offset(&self) -> u64 {
        self.0
    }
}

/// `rust_streams_subscribe_resumes` (R14).
#[tokio::test]
async fn rust_streams_subscribe_resumes() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let fetch = {
        let requests = Arc::clone(&requests);
        move |offset: u64| {
            requests.lock().expect("not poisoned").push(offset);
            async move {
                Ok::<_, LoamsError>(
                    (0..5u64)
                        .filter(|o| *o >= offset)
                        .take(2)
                        .map(Rec)
                        .collect::<Vec<_>>(),
                )
            }
        }
    };
    let mut sub = loams::subscribe(1, fetch.clone());
    let mut seen = Vec::new();
    while let Some(record) = sub.next().await {
        seen.push(record.expect("a record").0);
        if seen.len() == 3 {
            break;
        }
    }
    assert_eq!(seen, [1, 2, 3]);
    assert_eq!(sub.offset(), 4);
    assert_eq!(*requests.lock().expect("not poisoned"), [1, 3]);
    drop(sub);

    let mut resumed = loams::subscribe(4, fetch);
    assert_eq!(
        resumed.next().await.expect("one more").expect("a record").0,
        4
    );

    // An empty partition backs off rather than spinning.
    let fetches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&fetches);
    let mut idle = loams::subscribe(0, move |_offset| {
        counter.fetch_add(1, Ordering::SeqCst);
        async { Ok::<Vec<Rec>, LoamsError>(Vec::new()) }
    });
    let waited = tokio::time::timeout(Duration::from_millis(250), idle.next()).await;
    assert!(waited.is_err(), "nothing arrived");
    let n = fetches.load(Ordering::SeqCst);
    assert!((2..=3).contains(&n), "{n} fetches in 250 ms");
}

/// `rust_collection_handle_delegates` (SDKP §2.8).
#[tokio::test]
async fn rust_collection_handle_delegates() {
    let stub = Stub::default();
    let notes = CollectionHandle::new(&stub, "notes", Some("app"));
    let none = CallOptionsOverrides::new;
    notes.upsert(&docs(1), none()).await.expect("upsert");
    notes.get(&[DocId::Uint(1)], none()).await.expect("get");
    notes
        .delete(&[DocId::Uint(1)], none())
        .await
        .expect("delete");
    notes
        .patch(
            &DocId::Uint(1),
            &PatchChange {
                set: Some(json!({"a": 1})),
                ..PatchChange::default()
            },
            none(),
        )
        .await
        .expect("patch");
    notes
        .count(Some(&Filter::eq("tags", "x")), none())
        .await
        .expect("count");
    let scrolled: Vec<_> = notes.scroll_all(None, None, none()).collect().await;
    assert!(scrolled.is_empty());
    notes
        .search(Search::on("ignored").text("refund"), none())
        .await
        .expect("search");
    notes
        .vector_search(
            "embedding",
            &[1.0, 0.0, 0.0, 0.0],
            VectorOptions::k(3),
            Some(3),
            none(),
        )
        .await
        .expect("vector search");
    notes.info(none()).await.expect("info");
    notes.drop(none()).await.expect("drop");

    let calls = stub.calls();
    let names: Vec<&str> = calls.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "write", "get", "write", "write", "count", "scroll", "search", "search", "info", "drop"
        ]
    );
    for (name, request) in &calls {
        assert_eq!(request["collection"], "notes", "{name}");
        assert_eq!(request["namespace"], "app", "{name}");
    }
}

/// `rust_search_query_all_requires_tiebreak` (SDKP §2.4).
#[tokio::test]
async fn rust_search_query_all_requires_tiebreak() {
    let stub = Stub::default();
    let unsorted = Search::on("notes")
        .text(TextRetriever::query_string("refund"))
        .limit(2);
    let refused: Vec<Result<Hit, LoamsError>> =
        loams::handle::query_all(&stub, unsorted.clone(), CallOptionsOverrides::new())
            .collect()
            .await;
    assert_eq!(refused.len(), 1);
    assert_eq!(
        refused[0].as_ref().expect_err("refused").kind,
        ErrorKind::InvalidArgument
    );
    let score_only = unsorted.clone().sort([Sort::score(None)]);
    let refused: Vec<_> = loams::handle::query_all(&stub, score_only, CallOptionsOverrides::new())
        .collect()
        .await;
    assert!(refused[0].is_err());
    assert!(stub.calls().is_empty(), "no call was made");

    {
        let mut pages = stub.search_pages.lock().expect("not poisoned");
        pages.push_back(vec![1, 2]);
        pages.push_back(vec![3, 4]);
        pages.push_back(vec![5]);
    }
    let sorted = unsorted.sort([Sort::score(None), Sort::pk(None)]);
    let hits: Vec<Hit> = loams::handle::query_all(&stub, sorted, CallOptionsOverrides::new())
        .map(|hit| hit.expect("a hit"))
        .collect()
        .await;
    assert_eq!(hits.len(), 5);
    let calls = stub.calls();
    assert_eq!(calls.len(), 3);
    assert!(calls[0].1.get("searchAfter").is_none());
    assert_eq!(calls[1].1["searchAfter"], json!([1.0, 2.0]));
}
