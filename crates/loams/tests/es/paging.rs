//! Task 10: `search_after` paging without a point in time (C35) and
//! `_delete_by_query` over the native `delete_by_filter` (Ruling 6, D87).

use std::sync::Arc;

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::docs::cache_fixture;
use crate::harness::{Answer, Es};

/// `lines` as an NDJSON body, each line terminated.
fn ndjson(lines: &[Value]) -> Vec<u8> {
    let mut body = String::new();
    for line in lines {
        body.push_str(&line.to_string());
        body.push('\n');
    }
    body.into_bytes()
}

/// Indexes `docs` into `index` through `_bulk?refresh=true`.
async fn bulk_index(es: &Es, index: &str, docs: &[(String, Value)]) {
    for chunk in docs.chunks(1_000) {
        let mut lines = Vec::new();
        for (id, doc) in chunk {
            lines.push(json!({"index": {"_index": index, "_id": id}}));
            lines.push(doc.clone());
        }
        let a = es
            .send_raw(
                Method::POST,
                "/_bulk?refresh=true",
                Some(("application/x-ndjson", ndjson(&lines))),
                &[],
            )
            .await;
        assert_eq!(a.status, StatusCode::OK, "{}", a.text);
        assert_eq!(a.body["errors"], false, "{}", a.text);
    }
}

fn ids(a: &Answer) -> Vec<String> {
    a.body["hits"]["hits"]
        .as_array()
        .unwrap_or_else(|| panic!("hits: {}", a.text))
        .iter()
        .map(|h| h["_id"].as_str().expect("_id").to_string())
        .collect()
}

async fn count(es: &Es, index: &str) -> u64 {
    let a = Es::ok(es.get(&format!("/{index}/_count")).await);
    a.body["count"].as_u64().expect("count")
}

async fn dbq(es: &Es, path: &str, body: Value) -> Answer {
    es.post(path, body).await
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[tokio::test]
async fn search_after_skips_equal_sort_values() {
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/i",
            Some(json!({"mappings": {"properties": {"created_at": {"type": "long"}}}})),
        )
        .await,
    );
    let docs: Vec<(String, Value)> = [1000, 1000, 1001, 1001, 1002]
        .into_iter()
        .enumerate()
        .map(|(i, t)| (format!("d{i}"), json!({"created_at": t})))
        .collect();
    bulk_index(&es, "i", &docs).await;
    let a = Es::ok(es.get("/i/_search?sort=created_at:asc&size=2").await);
    assert_eq!(ids(&a), ["d0", "d1"]);
    assert_eq!(a.body["hits"]["hits"][1]["sort"], json!([1000]));
    // No tiebreaker: the tied 1000s are skipped with the page's last value.
    let a = Es::ok(
        es.post(
            "/i/_search",
            json!({"sort": [{"created_at": "asc"}], "size": 10, "search_after": [1000]}),
        )
        .await,
    );
    assert_eq!(ids(&a), ["d2", "d3", "d4"], "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn chat_history_pages_in_creation_order() {
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/i",
            Some(json!({"mappings": {"properties": {
                "session_id": {"type": "keyword"},
                "created_at": {"type": "date"},
                "history": {"type": "text"}
            }}})),
        )
        .await,
    );
    let mut made = Vec::new();
    for n in 0..14u64 {
        // Pairs share a millisecond.
        let a = es
            .post(
                "/i/_doc?refresh=true",
                json!({"session_id": "s", "created_at": 1_700_000_000_000u64 + n / 2, "history": format!("m{n}")}),
            )
            .await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
        made.push(a.body["_id"].as_str().expect("_id").to_string());
    }
    let query = json!({"term": {"session_id": "s"}});
    let a = Es::ok(
        es.post(
            "/i/_search?sort=created_at:asc&size=100",
            json!({"query": query}),
        )
        .await,
    );
    assert_eq!(ids(&a), made, "ULID tie-break keeps insertion order");
    let last = a.body["hits"]["hits"][13]["sort"].clone();
    let a = Es::ok(
        es.post(
            "/i/_search",
            json!({"query": query, "sort": [{"created_at": "asc"}], "size": 100, "search_after": last}),
        )
        .await,
    );
    assert!(ids(&a).is_empty(), "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn search_after_pages_through_a_multi_index_alias() {
    let es = Es::start().await;
    for index in ["p1", "p2"] {
        Es::ok(
            es.put(
                &format!("/{index}"),
                Some(json!({"mappings": {"properties": {"t": {"type": "long"}}}})),
            )
            .await,
        );
    }
    bulk_index(
        &es,
        "p1",
        &[1, 3, 5].map(|t| (format!("a{t}"), json!({"t": t}))),
    )
    .await;
    bulk_index(
        &es,
        "p2",
        &[2, 4, 6].map(|t| (format!("b{t}"), json!({"t": t}))),
    )
    .await;
    Es::ok(
        es.post(
            "/_aliases",
            json!({"actions": [
                {"add": {"index": "p1", "alias": "both"}},
                {"add": {"index": "p2", "alias": "both"}}
            ]}),
        )
        .await,
    );
    let mut seen = Vec::new();
    let mut after: Option<Value> = None;
    loop {
        let mut body = json!({"sort": [{"t": "desc"}], "size": 2});
        if let Some(after) = &after {
            body["search_after"] = after.clone();
        }
        let a = Es::ok(es.post("/both/_search", body).await);
        let page = ids(&a);
        if page.is_empty() {
            break;
        }
        after = Some(a.body["hits"]["hits"][page.len() - 1]["sort"].clone());
        seen.extend(page);
    }
    assert_eq!(seen, ["b6", "a5", "b4", "a3", "b2", "a1"]);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn search_after_on_score_is_refused() {
    let es = Es::start().await;
    Es::ok(es.put("/i", None).await);
    let a = es
        .post(
            "/i/_search",
            json!({"query": {"match_all": {}}, "search_after": [1.2]}),
        )
        .await;
    assert_eq!(a.status.as_u16(), 400, "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pit_requests_are_refused() {
    let es = Es::start().await;
    Es::ok(es.put("/i", None).await);
    let a = es
        .send(Method::POST, "/i/_pit?keep_alive=1m", None, &[])
        .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    assert_eq!(
        a.body,
        json!({"error": "no handler found for uri [/i/_pit?keep_alive=1m] and method [POST]"})
    );
    let a = es
        .post(
            "/i/_search",
            json!({"pit": {"id": "x", "keep_alive": "1m"}, "query": {"match_all": {}}}),
        )
        .await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some("Loams does not support [pit] (Elasticsearch API Phase A)"),
    );
    es.server.shutdown().await.expect("shutdown");
}

/// The LlamaIndex six nodes: `metadata.ref_doc_id` `test-0` … `test-5`,
/// `metadata.theme` `a` for the first three and `b` for the rest.
async fn llama_nodes(es: &Es, index: &str) {
    Es::ok(
        es.put(
            &format!("/{index}"),
            Some(json!({"mappings": {"properties": {
                "content": {"type": "text"},
                "embedding": {"type": "dense_vector", "dims": 3, "index": true, "similarity": "cosine"},
                "metadata": {"properties": {
                    "ref_doc_id": {"type": "keyword"},
                    "theme": {"type": "keyword"}
                }}
            }}})),
        )
        .await,
    );
    let docs: Vec<(String, Value)> = (0..6)
        .map(|i| {
            (
                format!("n{i}"),
                json!({
                    "content": format!("node {i}"),
                    "embedding": [1.0, i as f32, 0.5],
                    "metadata": {"ref_doc_id": format!("test-{i}"), "theme": if i < 3 { "a" } else { "b" }}
                }),
            )
        })
        .collect();
    bulk_index(es, index, &docs).await;
}

#[tokio::test]
async fn delete_by_query_deletes_exactly_the_matches() {
    let es = Es::start().await;
    llama_nodes(&es, "i").await;
    let a = Es::ok(
        dbq(
            &es,
            "/i/_delete_by_query?refresh=true",
            json!({"query": {"term": {"metadata.ref_doc_id": "test-0"}}}),
        )
        .await,
    );
    assert_eq!(
        keys(&a.body),
        [
            "took",
            "timed_out",
            "total",
            "deleted",
            "batches",
            "version_conflicts",
            "noops",
            "retries",
            "throttled_millis",
            "requests_per_second",
            "throttled_until_millis",
            "failures"
        ]
    );
    assert_eq!(a.body["deleted"], 1, "{}", a.text);
    assert_eq!(a.body["total"], 1);
    assert_eq!(a.body["batches"], 1);
    assert_eq!(a.body["noops"], 0);
    assert_eq!(a.body["version_conflicts"], 0);
    assert_eq!(a.body["failures"], json!([]));
    assert_eq!(a.body["retries"], json!({"bulk": 0, "search": 0}));
    assert_eq!(count(&es, "i").await, 5);
    // The C36 bool form: `terms _id` of two ids and a keyword term.
    let a = Es::ok(
        dbq(
            &es,
            "/i/_delete_by_query?refresh=true&conflicts=proceed",
            json!({"query": {"bool": {"filter": [
                {"terms": {"_id": ["n1", "n4"]}},
                {"term": {"metadata.theme": "a"}}
            ]}}}),
        )
        .await,
    );
    assert_eq!(
        (&a.body["deleted"], &a.body["total"]),
        (&json!(1), &json!(1))
    );
    assert_eq!(count(&es, "i").await, 4);
    // Nothing matches: zero batches.
    let a = Es::ok(
        dbq(
            &es,
            "/i/_delete_by_query",
            json!({"query": {"term": {"metadata.theme": "zzz"}}}),
        )
        .await,
    );
    assert_eq!(
        (&a.body["deleted"], &a.body["total"], &a.body["batches"]),
        (&json!(0), &json!(0), &json!(0))
    );
    // `q` selects too.
    let a = Es::ok(dbq(&es, "/i/_delete_by_query?q=metadata.theme:b", Value::Null).await);
    assert_eq!(a.body["deleted"], 3, "{}", a.text);
    assert_eq!(count(&es, "i").await, 1);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn delete_by_query_ignores_documents_written_after_it_started() {
    // Batches of 100, so the request runs 30 batches while the writer runs.
    let es = Arc::new(Es::start_with(|config| config.query.filter_write_batch = 100).await);
    Es::ok(
        es.put(
            "/i",
            Some(json!({"mappings": {"properties": {"tag": {"type": "keyword"}}}})),
        )
        .await,
    );
    let docs: Vec<(String, Value)> = (0..3_000)
        .map(|i| (format!("d{i:05}"), json!({"tag": "x"})))
        .collect();
    bulk_index(&es, "i", &docs).await;
    let writer = {
        let es = es.clone();
        tokio::spawn(async move {
            // Start once the request's first batch is visible: its pin is
            // taken before that batch is written.
            while count(&es, "i").await >= 3_000 {
                tokio::task::yield_now().await;
            }
            for n in 0..100 {
                let a = es
                    .put(&format!("/i/_doc/late{n:03}"), Some(json!({"tag": "x"})))
                    .await;
                assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
            }
        })
    };
    let a = Es::ok(
        dbq(
            &es,
            "/i/_delete_by_query?scroll_size=500&refresh=true",
            json!({"query": {"term": {"tag": "x"}}}),
        )
        .await,
    );
    writer.await.expect("writer");
    assert_eq!(a.body["deleted"], 3_000, "{}", a.text);
    assert_eq!(a.body["total"], 3_000);
    assert_eq!(a.body["batches"], 30);
    assert_eq!(count(&es, "i").await, 100);
    let a = Es::ok(es.get("/i/_search?size=200").await);
    assert!(
        ids(&a).iter().all(|id| id.starts_with("late")),
        "{}",
        a.text
    );
    Arc::try_unwrap(es)
        .ok()
        .expect("one owner")
        .server
        .shutdown()
        .await
        .expect("shutdown");
}

#[tokio::test]
async fn delete_by_query_with_max_docs_stops() {
    let es = Es::start_with(|config| config.query.filter_write_batch = 4).await;
    Es::ok(
        es.put(
            "/i",
            Some(json!({"mappings": {"properties": {"tag": {"type": "keyword"}}}})),
        )
        .await,
    );
    let docs: Vec<(String, Value)> = (0..25)
        .map(|i| (format!("d{i:02}"), json!({"tag": "x"})))
        .collect();
    bulk_index(&es, "i", &docs).await;
    let a = Es::ok(
        dbq(
            &es,
            "/i/_delete_by_query?refresh=true",
            json!({"query": {"match_all": {}}, "max_docs": 10}),
        )
        .await,
    );
    assert_eq!(a.body["deleted"], 10, "{}", a.text);
    assert_eq!(a.body["total"], 10);
    assert_eq!(a.body["batches"], 3);
    assert_eq!(count(&es, "i").await, 15);
    // The URL's `max_docs` works too.
    let a = Es::ok(
        dbq(
            &es,
            "/i/_delete_by_query?max_docs=2",
            json!({"query": {"match_all": {}}}),
        )
        .await,
    );
    assert_eq!(a.body["deleted"], 2, "{}", a.text);
    assert_eq!(count(&es, "i").await, 13);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn delete_by_query_through_an_alias_deletes_in_every_member() {
    let es = Es::start().await;
    cache_fixture(&es).await;
    for (index, id) in [
        ("test_index2", "a"),
        ("test_index2", "b"),
        ("test_index1", "c"),
    ] {
        let a = es
            .put(
                &format!("/{index}/_doc/{id}?refresh=true"),
                Some(json!({"llm_output": id})),
            )
            .await;
        assert!(a.status.is_success(), "{}", a.text);
    }
    let a = Es::ok(
        dbq(
            &es,
            "/test_alias/_delete_by_query?refresh=true",
            json!({"query": {"match_all": {}}}),
        )
        .await,
    );
    assert_eq!(
        (&a.body["deleted"], &a.body["total"]),
        (&json!(3), &json!(3)),
        "{}",
        a.text
    );
    assert_eq!(a.body["batches"], 2);
    assert_eq!(count(&es, "test_alias").await, 0);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn delete_by_query_errors_follow_es() {
    let es = Es::start().await;
    llama_nodes(&es, "i").await;
    let a = dbq(&es, "/i/_delete_by_query", json!({})).await;
    a.assert_error(
        400,
        "action_request_validation_exception",
        Some("Validation Failed: 1: query is missing;"),
    );
    let a = dbq(
        &es,
        "/i/_delete_by_query",
        json!({"query": {"match_all": {}}, "script": "ctx._source.remove('x')"}),
    )
    .await;
    a.assert_error(
        400,
        "parsing_exception",
        Some("Unknown key for a VALUE_STRING in [script]."),
    );
    // ES's `max_docs` and `scroll_size` texts (row T11-3).
    for (path, body, kind, reason) in [
        (
            "/i/_delete_by_query?max_docs=0",
            json!({"query": {"match_all": {}}}),
            "illegal_argument_exception",
            "[max_docs] should be >= [slices]",
        ),
        (
            "/i/_delete_by_query",
            json!({"query": {"match_all": {}}, "max_docs": -2}),
            "illegal_argument_exception",
            "[max_docs] parameter cannot be negative, found [-2]",
        ),
        (
            "/i/_delete_by_query?scroll_size=0",
            json!({"query": {"match_all": {}}}),
            "action_request_validation_exception",
            "Validation Failed: 1: [size] cannot be [0] in a scroll context;",
        ),
        (
            "/i/_delete_by_query?scroll_size=20000",
            json!({"query": {"match_all": {}}}),
            "search_phase_execution_exception",
            "all shards failed",
        ),
    ] {
        let a = dbq(&es, path, body).await;
        assert_eq!(a.status.as_u16(), 400, "{}", a.text);
        assert_eq!(a.body["error"]["type"], kind, "{}", a.text);
        assert_eq!(a.body["error"]["reason"], reason, "{}", a.text);
    }
    let a = dbq(
        &es,
        "/missing/_delete_by_query",
        json!({"query": {"match_all": {}}}),
    )
    .await;
    a.assert_error(404, "index_not_found_exception", None);
    for path in [
        "/i/_delete_by_query?wait_for_completion=false",
        "/i/_delete_by_query?slices=2",
        "/i/_delete_by_query?requests_per_second=10",
    ] {
        let a = dbq(&es, path, json!({"query": {"match_all": {}}})).await;
        a.assert_error(400, "illegal_argument_exception", None);
    }
    let a = dbq(
        &es,
        "/i/_delete_by_query?nope=1",
        json!({"query": {"match_all": {}}}),
    )
    .await;
    a.assert_error(400, "illegal_argument_exception", None);
    // A knn query is not a leaf query.
    let a = dbq(
        &es,
        "/i/_delete_by_query",
        json!({"query": {"knn": {"field": "embedding", "query_vector": [1.0, 0.0, 0.0], "k": 1}}}),
    )
    .await;
    assert_eq!(a.status.as_u16(), 400, "{}", a.text);
    assert_eq!(count(&es, "i").await, 6, "nothing was deleted");
    es.server.shutdown().await.expect("shutdown");
}

// ----- the scroll API (#950) -----

/// The `_id`s of a page.
fn page_ids(a: &Answer) -> Vec<String> {
    a.body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["_id"].as_str().expect("_id").to_string())
        .collect()
}

#[tokio::test]
async fn scroll_pages_through_every_match() {
    let es = Es::start().await;
    let docs: Vec<(String, Value)> = (0..25)
        .map(|i| (format!("d{i:02}"), json!({"n": i, "even": i % 2 == 0})))
        .collect();
    bulk_index(&es, "sc", &docs).await;
    // helpers.scan's first request.
    let a = Es::ok(
        es.post(
            "/sc/_search?scroll=5m",
            json!({"query": {"match_all": {}}, "size": 10, "sort": "_doc"}),
        )
        .await,
    );
    let mut seen = page_ids(&a);
    assert_eq!(seen.len(), 10);
    assert_eq!(a.body["hits"]["total"]["value"], 25);
    let mut id = a.body["_scroll_id"]
        .as_str()
        .expect("_scroll_id")
        .to_string();
    assert_eq!(
        a.body
            .as_object()
            .expect("object")
            .keys()
            .next()
            .map(String::as_str),
        Some("_scroll_id")
    );
    let mut pages = 1;
    loop {
        let a = Es::ok(
            es.post("/_search/scroll", json!({"scroll": "5m", "scroll_id": id}))
                .await,
        );
        let ids = page_ids(&a);
        id = a.body["_scroll_id"]
            .as_str()
            .expect("_scroll_id")
            .to_string();
        if ids.is_empty() {
            break;
        }
        assert_eq!(a.body["hits"]["total"]["value"], 25);
        seen.extend(ids);
        pages += 1;
        assert!(pages < 10, "the scroll does not end");
    }
    assert_eq!(pages, 3);
    let mut expected: Vec<String> = docs.iter().map(|(id, _)| id.clone()).collect();
    expected.sort();
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(sorted, expected, "every match once");
    // Clearing answers as ES does.
    let a = es
        .send(
            Method::DELETE,
            "/_search/scroll",
            Some(json!({"scroll_id": [id]})),
            &[],
        )
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    assert_eq!(a.body, json!({"succeeded": true, "num_freed": 1}));
    let a = es
        .send(Method::DELETE, "/_search/scroll/_all", None, &[])
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
}

#[tokio::test]
async fn scroll_with_a_query_a_sort_and_the_url_forms() {
    let es = Es::start().await;
    let docs: Vec<(String, Value)> = (0..12)
        .map(|i| (format!("d{i:02}"), json!({"n": i, "even": i % 2 == 0})))
        .collect();
    bulk_index(&es, "sc", &docs).await;
    // A query and a field sort, size as a URL parameter.
    let a = Es::ok(
        es.post(
            "/sc/_search?scroll=1m&size=2",
            json!({"query": {"term": {"even": true}}, "sort": [{"n": "desc"}]}),
        )
        .await,
    );
    let mut ns: Vec<i64> = Vec::new();
    let collect = |a: &Answer, ns: &mut Vec<i64>| {
        for h in a.body["hits"]["hits"].as_array().expect("hits") {
            ns.push(h["_source"]["n"].as_i64().expect("n"));
            assert_eq!(h["sort"][0], h["_source"]["n"], "the user sort stays");
        }
    };
    collect(&a, &mut ns);
    let mut id = a.body["_scroll_id"].as_str().expect("id").to_string();
    loop {
        // The id in the path, GET.
        let a = Es::ok(es.get(&format!("/_search/scroll/{id}?scroll=1m")).await);
        if a.body["hits"]["hits"].as_array().expect("hits").is_empty() {
            break;
        }
        collect(&a, &mut ns);
        id = a.body["_scroll_id"].as_str().expect("id").to_string();
    }
    assert_eq!(ns, [10, 8, 6, 4, 2, 0]);
    // Without a sort, hits carry no `sort`.
    let a = Es::ok(es.post("/sc/_search?scroll=1m", json!({"size": 5})).await);
    assert!(
        a.body["hits"]["hits"][0].get("sort").is_none(),
        "{}",
        a.text
    );
}

#[tokio::test]
async fn scroll_refusals_and_missing_contexts() {
    let es = Es::start().await;
    bulk_index(&es, "sc", &[("a".to_string(), json!({"n": 1}))]).await;
    es.post("/sc/_search?scroll=1m", json!({"from": 1}))
        .await
        .assert_error(400, "action_request_validation_exception", None);
    es.post(
        "/sc/_search?scroll=1m",
        json!({"search_after": [1], "sort": ["n"]}),
    )
    .await
    .assert_error(400, "action_request_validation_exception", None);
    es.post("/sc/_search?scroll=2d", json!({}))
        .await
        .assert_error(400, "illegal_argument_exception", None);
    es.post(
        "/_search/scroll",
        json!({"scroll": "1m", "scroll_id": "bogus"}),
    )
    .await
    .assert_error(404, "search_context_missing_exception", None);
    es.post("/_search/scroll", json!({"scroll": "1m"}))
        .await
        .assert_error(400, "action_request_validation_exception", None);
    // A scroll from another namespace is missing here.
    let a = Es::ok(es.post("/sc/_search?scroll=1m", json!({})).await);
    let id = a.body["_scroll_id"].as_str().expect("id").to_string();
    let other = es
        .send(
            Method::POST,
            "/_search/scroll",
            Some(json!({"scroll": "1m", "scroll_id": id})),
            &[("loams-namespace", "other")],
        )
        .await;
    assert_eq!(other.status, StatusCode::NOT_FOUND, "{}", other.text);
    let a = es
        .send(
            Method::DELETE,
            "/_search/scroll",
            Some(json!({"scroll_id": "bogus"})),
            &[],
        )
        .await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert_eq!(a.body, json!({"succeeded": true, "num_freed": 0}));
}
