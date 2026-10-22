//! Task 9: search execution, `_search`, `_count`, `_msearch` and ES scores.
//!
//! The fixtures index LangChain's `foo`/`bar`/`baz` texts with
//! `ConsistentFakeEmbeddings`-like 16-dimensional vectors, and LlamaIndex's
//! six nodes with 3-dimensional vectors, through `_bulk`.

use std::collections::{BTreeMap, BTreeSet};

use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::docs::cache_fixture;
use crate::harness::{Answer, Es};

const EINSTEIN: &str = "bd2e080b-159a-4030-acc3-d98afd2ba49b";
const BRONTE: &str = "f658de3b-8cef-4d1c-8bed-9a263c907251";
const CURIE: &str = "0b31ae71-b797-4e88-8495-031371a7752e";
const KING: &str = "c330d77f-90bd-4c51-9ed2-57d8d693b3b0";

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
async fn bulk_index(es: &Es, index: &str, docs: &[(&str, Value)]) {
    let mut lines = Vec::new();
    for (id, doc) in docs {
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

/// `ConsistentFakeEmbeddings`: fifteen ones and the text's position.
fn fake(i: usize) -> Vec<f32> {
    let mut v = vec![1.0; 15];
    v.push(i as f32);
    v
}

/// LangChain's fixture in `index`: `foo`, `bar`, `baz` with pages 0–2, ids
/// 1–3, under `DenseVectorStrategy`'s mapping (or `index: false`, the
/// script-score strategy's).
async fn langchain(es: &Es, index: &str, indexed: bool) {
    let mut vector = json!({"type": "dense_vector", "dims": 16, "index": indexed});
    if indexed {
        vector["similarity"] = json!("cosine");
    }
    Es::ok(
        es.put(
            &format!("/{index}"),
            Some(json!({"mappings": {"properties": {"vector": vector}}})),
        )
        .await,
    );
    let docs: Vec<(String, Value)> = ["foo", "bar", "baz"]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            (
                (i + 1).to_string(),
                json!({"text": text, "vector": fake(i), "metadata": {"page": i}}),
            )
        })
        .collect();
    let docs: Vec<(&str, Value)> = docs
        .iter()
        .map(|(id, d)| (id.as_str(), d.clone()))
        .collect();
    bulk_index(es, index, &docs).await;
}

/// LlamaIndex's six nodes in `index`, with `similarity`.
async fn llama(es: &Es, index: &str, similarity: &str) {
    Es::ok(
        es.put(
            &format!("/{index}"),
            Some(json!({"mappings": {"properties": {
                "embedding": {"type": "dense_vector", "dims": 3, "index": true, "similarity": similarity},
                "content": {"type": "text"},
            }}})),
        )
        .await,
    );
    let node = |content: &str, metadata: Value, embedding: [f32; 3]| json!({"content": content, "metadata": metadata, "embedding": embedding});
    bulk_index(
        es,
        index,
        &[
            (
                KING,
                node(
                    "lorem ipsum",
                    json!({"author": "Stephen King", "theme": "Friendship"}),
                    [1.0, 0.0, 0.0],
                ),
            ),
            (
                "c3d1e1dd-8fb4-4b8f-b7ea-7fa96038d39d",
                node(
                    "lorem ipsum",
                    json!({"director": "Francis Ford Coppola", "theme": "Mafia"}),
                    [0.0, 1.0, 0.0],
                ),
            ),
            (
                "c3ew11cd-8fb4-4b8f-b7ea-7fa96038d39d",
                node(
                    "lorem ipsum",
                    json!({"director": "Christopher Nolan"}),
                    [0.0, 0.0, 1.0],
                ),
            ),
            (
                CURIE,
                node(
                    "I was taught that the way of progress was neither swift nor easy.",
                    json!({"author": "Marie Curie"}),
                    [0.0, 0.0, 0.9],
                ),
            ),
            (
                EINSTEIN,
                node(
                    "The important thing is not to stop questioning. Curiosity has its own \
                     reason for existing.",
                    json!({"author": "Albert Einstein"}),
                    [0.0, 0.0, 0.5],
                ),
            ),
            (
                BRONTE,
                node(
                    "I am no bird; and no net ensnares me; I am a free human being with an \
                     independent will.",
                    json!({"author": "Charlotte Bronte"}),
                    [0.0, 0.0, 0.3],
                ),
            ),
        ],
    )
    .await;
}

async fn search(es: &Es, path: &str, body: Value) -> Answer {
    Es::ok(es.post(path, body).await)
}

fn hits(a: &Answer) -> &Vec<Value> {
    a.body["hits"]["hits"].as_array().expect("hits")
}

fn ids(a: &Answer) -> Vec<String> {
    hits(a)
        .iter()
        .map(|h| h["_id"].as_str().expect("_id").to_string())
        .collect()
}

fn scores(a: &Answer) -> BTreeMap<String, f64> {
    hits(a)
        .iter()
        .map(|h| {
            (
                h["_id"].as_str().expect("_id").to_string(),
                h["_score"].as_f64().expect("_score"),
            )
        })
        .collect()
}

fn close(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

fn knn(field: &str, vector: &[f32], k: usize) -> Value {
    json!({"field": field, "query_vector": vector, "k": k, "num_candidates": 50})
}

#[tokio::test]
async fn a_langchain_knn_body_returns_filtered_sources() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let a = search(
        &es,
        "/lc/_search?_source_includes=metadata,text",
        json!({"knn": {"field": "vector", "filter": [], "k": 1, "num_candidates": 50,
                       "query_vector": fake(0)},
               "size": 1, "_source": true}),
    )
    .await;
    assert_eq!(ids(&a), ["1"], "{}", a.text);
    assert_eq!(
        hits(&a)[0]["_source"],
        json!({"text": "foo", "metadata": {"page": 0}})
    );
    assert_eq!(a.body["hits"]["total"]["value"], 1, "{}", a.text);
    assert_eq!(a.body["_shards"]["total"], 1);
    assert!(!a.header("loams-consistency-token").is_empty());
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn knn_scores_follow_the_es_formula_per_similarity() {
    let es = Es::start().await;
    // cosine: an identical vector scores 1.
    langchain(&es, "lc", true).await;
    let a = search(
        &es,
        "/lc/_search",
        json!({"knn": knn("vector", &fake(0), 1)}),
    )
    .await;
    assert!(close(scores(&a)["1"], 1.0, 1e-6), "{}", a.text);
    // l2_norm: 1 / (1 + d²).
    llama(&es, "l2", "l2_norm").await;
    let a = search(
        &es,
        "/l2/_search",
        json!({"knn": knn("embedding", &[0.0, 0.0, 0.5], 3), "size": 3}),
    )
    .await;
    assert_eq!(ids(&a), [EINSTEIN, BRONTE, CURIE], "{}", a.text);
    let s = scores(&a);
    assert!(close(s[EINSTEIN], 1.0, 1e-5), "{}", a.text);
    assert!(close(s[BRONTE], 1.0 / 1.04, 1e-5), "{}", a.text);
    assert!(close(s[CURIE], 1.0 / 1.16, 1e-5), "{}", a.text);
    assert!(close(
        a.body["hits"]["max_score"].as_f64().unwrap(),
        1.0,
        1e-5
    ));
    // dot_product (unit vectors): (1 + dot) / 2.
    for (index, similarity) in [("dp", "dot_product"), ("mip", "max_inner_product")] {
        Es::ok(
            es.put(
                &format!("/{index}"),
                Some(json!({"mappings": {"properties": {"v": {
                    "type": "dense_vector", "dims": 2, "index": true, "similarity": similarity}}}})),
            )
            .await,
        );
    }
    bulk_index(
        &es,
        "dp",
        &[
            ("a", json!({"v": [1.0, 0.0]})),
            ("b", json!({"v": [0.6, 0.8]})),
        ],
    )
    .await;
    let a = search(&es, "/dp/_search", json!({"knn": knn("v", &[1.0, 0.0], 2)})).await;
    let s = scores(&a);
    assert!(close(s["a"], 1.0, 1e-6), "{}", a.text);
    assert!(close(s["b"], 0.8, 1e-6), "{}", a.text);
    // max_inner_product: 1 / (1 − dot) for a negative dot, dot + 1 else.
    bulk_index(
        &es,
        "mip",
        &[
            ("neg", json!({"v": [-1.0, 0.0]})),
            ("pos", json!({"v": [1.0, 0.0]})),
        ],
    )
    .await;
    let a = search(
        &es,
        "/mip/_search",
        json!({"knn": knn("v", &[2.0, 0.0], 2)}),
    )
    .await;
    assert_eq!(ids(&a), ["pos", "neg"], "{}", a.text);
    let s = scores(&a);
    assert!(close(s["pos"], 3.0, 1e-6), "{}", a.text);
    assert!(close(s["neg"], 1.0 / 3.0, 1e-6), "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_query_plus_knn_sums_boosted_es_scores() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let text = search(
        &es,
        "/lc/_search",
        json!({"query": {"match": {"text": "foo"}}}),
    )
    .await;
    let vector = search(
        &es,
        "/lc/_search",
        json!({"knn": knn("vector", &fake(0), 3), "size": 3}),
    )
    .await;
    let mut boosted = knn("vector", &fake(0), 3);
    boosted["boost"] = json!(2.0);
    let a = search(
        &es,
        "/lc/_search",
        json!({"query": {"bool": {"must": [{"match": {"text": {"query": "foo"}}}], "filter": []}},
               "knn": boosted, "size": 3}),
    )
    .await;
    let (bm25, cos) = (scores(&text), scores(&vector));
    let summed = scores(&a);
    assert_eq!(summed.len(), 3, "{}", a.text);
    for (id, score) in &summed {
        let want = bm25.get(id).copied().unwrap_or(0.0) + 2.0 * cos[id];
        assert!(close(*score, want, 1e-5), "{id}: {score} vs {want}");
    }
    assert_eq!(ids(&a)[0], "1");
    // The total counts the union of the query's matches and the knn hits.
    assert_eq!(a.body["hits"]["total"]["value"], 3, "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn rrf_orders_by_reciprocal_rank() {
    let es = Es::start().await;
    llama(&es, "l2", "l2_norm").await;
    let text_query = json!({"match": {"content": "human"}});
    let vector = knn("embedding", &[0.0, 0.0, 0.5], 6);
    let a = search(
        &es,
        "/l2/_search",
        json!({"retriever": {"rrf": {"retrievers": [
                   {"standard": {"query": text_query}},
                   {"knn": vector}],
               "rank_constant": 60, "rank_window_size": 10}},
               "size": 3}),
    )
    .await;
    assert_eq!(ids(&a)[0], BRONTE, "{}", a.text);
    assert!(close(scores(&a)[BRONTE], 1.0 / 61.0 + 1.0 / 62.0, 1e-6));
    let rank = |a: &Answer| -> BTreeMap<String, usize> {
        ids(a)
            .into_iter()
            .enumerate()
            .map(|(i, id)| (id, i + 1))
            .collect()
    };
    let text = rank(&search(&es, "/l2/_search", json!({"query": text_query})).await);
    let knn_only = rank(&search(&es, "/l2/_search", json!({"knn": vector, "size": 6})).await);
    for (id, score) in scores(&a) {
        let want: f64 = [&text, &knn_only]
            .iter()
            .filter_map(|ranks| ranks.get(&id))
            .map(|r| 1.0 / (60.0 + *r as f64))
            .sum();
        assert!(close(score, want, 1e-6), "{id}: {score} vs {want}");
    }
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn bm25_bool_match_with_filter() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let a = search(
        &es,
        "/lc/_search",
        json!({"query": {"bool": {
            "must": [{"match": {"text": {"query": "foo bar baz"}}}],
            "filter": [{"term": {"metadata.page": 1}}]}}}),
    )
    .await;
    assert_eq!(ids(&a), ["2"], "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_plain_match_query_ranks_by_bm25() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let a = search(
        &es,
        "/lc/_search",
        json!({"query": {"match": {"text": {"query": "bar"}}}}),
    )
    .await;
    assert_eq!(ids(&a)[0], "2", "{}", a.text);
    assert!(hits(&a)[0]["_score"].as_f64().unwrap() > 0.0);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn equal_bm25_scores_are_ordered_by_id() {
    let es = Es::start().await;
    let docs: Vec<(&str, Value)> = [
        ("5", "foo"),
        ("2", "bar"),
        ("3", "foo"),
        ("4", "baz"),
        ("1", "foo"),
    ]
    .into_iter()
    .map(|(id, text)| (id, json!({"text": text})))
    .collect();
    bulk_index(&es, "c34", &docs).await;
    let a = search(
        &es,
        "/c34/_search",
        json!({"query": {"match": {"text": "foo"}}}),
    )
    .await;
    assert_eq!(ids(&a), ["1", "3", "5"], "{}", a.text);
    let s: BTreeSet<String> = scores(&a).values().map(|s| s.to_string()).collect();
    assert_eq!(s.len(), 1, "equal scores: {}", a.text);
    for hit in hits(&a) {
        let keys: BTreeSet<&str> = hit
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(["_index", "_id", "_score", "_source"]));
    }
    es.server.shutdown().await.expect("shutdown");
}

fn script_body(source: &str, vector: &[f32], query: Value) -> Value {
    json!({"query": {"script_score": {"query": query,
        "script": {"source": source, "params": {"query_vector": vector}}}}})
}

#[tokio::test]
async fn script_score_cosine_is_exact_cos_plus_one() {
    let es = Es::start().await;
    langchain(&es, "ss", false).await;
    let a = search(
        &es,
        "/ss/_search",
        script_body(
            "cosineSimilarity(params.query_vector, 'vector') + 1.0",
            &fake(0),
            json!({"match_all": {}}),
        ),
    )
    .await;
    assert_eq!(ids(&a)[0], "1", "{}", a.text);
    assert!(close(scores(&a)["1"], 2.0, 1e-6), "{}", a.text);
    assert_eq!(a.body["hits"]["total"]["value"], 3, "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn script_score_with_a_filter_scores_only_matches() {
    let es = Es::start().await;
    langchain(&es, "ss", false).await;
    let a = search(
        &es,
        "/ss/_search",
        script_body(
            "cosineSimilarity(params.query_vector, 'vector') + 1.0",
            &fake(1),
            json!({"bool": {"filter": [{"term": {"metadata.page": 0}}]}}),
        ),
    )
    .await;
    assert_eq!(ids(&a), ["1"], "{}", a.text);
    assert_eq!(a.body["hits"]["total"]["value"], 1, "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

/// A 3-dimensional unindexed vector index with two documents.
async fn script3(es: &Es) {
    Es::ok(
        es.put(
            "/ss3",
            Some(json!({"mappings": {"properties": {"v": {
                "type": "dense_vector", "dims": 3, "index": false}}}})),
        )
        .await,
    );
    bulk_index(
        es,
        "ss3",
        &[
            ("a", json!({"v": [0.0, 0.0, 0.5]})),
            ("b", json!({"v": [0.0, 0.0, 0.3]})),
        ],
    )
    .await;
}

#[tokio::test]
async fn script_score_sigmoid_dot_is_exact() {
    let es = Es::start().await;
    script3(&es).await;
    let a = search(
        &es,
        "/ss3/_search",
        script_body(
            "\n double value = dotProduct(params.query_vector, 'v');\n return sigmoid(1, Math.E, -value);\n ",
            &[0.0, 0.0, 1.0],
            json!({"match_all": {}}),
        ),
    )
    .await;
    assert_eq!(ids(&a), ["a", "b"], "{}", a.text);
    let s = scores(&a);
    assert!(
        close(s["a"], 1.0 / (1.0 + (-0.5f64).exp()), 1e-6),
        "{}",
        a.text
    );
    assert!(
        close(s["b"], 1.0 / (1.0 + (-0.3f64).exp()), 1e-6),
        "{}",
        a.text
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn script_score_inverse_l2_is_exact() {
    let es = Es::start().await;
    script3(&es).await;
    let a = search(
        &es,
        "/ss3/_search",
        script_body(
            "1 / (1 + l2norm(params.query_vector, 'v'))",
            &[0.0, 0.0, 1.0],
            json!({"match_all": {}}),
        ),
    )
    .await;
    assert_eq!(ids(&a), ["a", "b"], "{}", a.text);
    let s = scores(&a);
    assert!(close(s["a"], 1.0 / 1.5, 1e-6), "{}", a.text);
    assert!(close(s["b"], 1.0 / 1.7, 1e-6), "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn an_unindexed_vector_rejects_knn() {
    let es = Es::start().await;
    langchain(&es, "ss", false).await;
    let a = es
        .post("/ss/_search", json!({"knn": knn("vector", &fake(0), 1)}))
        .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_comma_list_searches_every_index() {
    let es = Es::start().await;
    for (index, field) in [("ia", "text"), ("ib", "custom")] {
        let docs: Vec<(String, Value)> = ["foo", "bar", "baz"]
            .iter()
            .enumerate()
            .map(|(i, t)| ((i + 1).to_string(), json!({ field: t })))
            .collect();
        let docs: Vec<(&str, Value)> = docs
            .iter()
            .map(|(id, d)| (id.as_str(), d.clone()))
            .collect();
        bulk_index(&es, index, &docs).await;
    }
    let a = search(
        &es,
        "/ia,ib/_search",
        json!({"query": {"multi_match": {"query": "foo bar baz", "fields": ["text", "custom"]}}}),
    )
    .await;
    let pairs: BTreeSet<(String, String)> = hits(&a)
        .iter()
        .map(|h| {
            let source = h["_source"].as_object().unwrap();
            let content = source
                .values()
                .next()
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            (content, h["_index"].as_str().unwrap().to_string())
        })
        .collect();
    let want: BTreeSet<(String, String)> = ["foo", "bar", "baz"]
        .iter()
        .flat_map(|t| {
            [
                (t.to_string(), "ia".to_string()),
                (t.to_string(), "ib".to_string()),
            ]
        })
        .collect();
    assert_eq!(pairs, want, "{}", a.text);
    assert_eq!(a.body["hits"]["total"]["value"], 6);
    assert_eq!(a.body["_shards"]["total"], 2);
    // Equal scores are ordered by _id, then _index (Ruling 15).
    let order: Vec<(String, String)> = hits(&a)
        .iter()
        .map(|h| {
            (
                h["_id"].as_str().unwrap().into(),
                h["_index"].as_str().unwrap().into(),
            )
        })
        .collect();
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "{}", a.text);
    // Paging over the merged hits.
    let page = search(
        &es,
        "/ia,ib/_search",
        json!({"query": {"multi_match": {"query": "foo bar baz", "fields": ["text", "custom"]}},
               "from": 2, "size": 2}),
    )
    .await;
    let paged: Vec<(String, String)> = hits(&page)
        .iter()
        .map(|h| {
            (
                h["_id"].as_str().unwrap().into(),
                h["_index"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(paged, order[2..4].to_vec(), "{}", page.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn wildcard_on_a_text_field_matches_terms() {
    let es = Es::start().await;
    llama(&es, "li", "cosine").await;
    let mut body = knn("embedding", &[1.0, 0.0, 0.0], 10);
    body["filter"] = json!([{"wildcard": {"metadata.author": "stephe*"}}]);
    let a = search(&es, "/li/_search", json!({"knn": body})).await;
    assert_eq!(ids(&a), [KING], "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_keyword_subfield_term_filters_knn() {
    let es = Es::start().await;
    llama(&es, "li", "cosine").await;
    let mut body = knn("embedding", &[0.0, 0.0, 1.0], 10);
    body["filter"] = json!([{"term": {"metadata.author.keyword": {"value": "Stephen King"}}}]);
    let a = search(&es, "/li/_search", json!({"knn": body})).await;
    assert_eq!(ids(&a), [KING], "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn total_hits_relation_is_gte_past_the_limit() {
    let es = Es::start().await;
    let docs: Vec<(String, Value)> = (0..30)
        .map(|i| (format!("d{i:02}"), json!({"n": i, "text": "foo"})))
        .collect();
    let docs: Vec<(&str, Value)> = docs
        .iter()
        .map(|(id, d)| (id.as_str(), d.clone()))
        .collect();
    bulk_index(&es, "t30", &docs).await;
    let a = search(&es, "/t30/_search", json!({"track_total_hits": 10})).await;
    assert_eq!(
        a.body["hits"]["total"],
        json!({"value": 10, "relation": "gte"})
    );
    let a = search(&es, "/t30/_search", json!({"track_total_hits": true})).await;
    assert_eq!(
        a.body["hits"]["total"],
        json!({"value": 30, "relation": "eq"})
    );
    let a = search(&es, "/t30/_search", json!({"track_total_hits": false})).await;
    assert!(a.body["hits"].get("total").is_none(), "{}", a.text);
    let a = search(&es, "/t30/_search?rest_total_hits_as_int=true", json!({})).await;
    assert_eq!(a.body["hits"]["total"], 30);
    // URL parameters override the body (Ruling 20).
    let a = search(
        &es,
        "/t30/_search?track_total_hits=5&size=2",
        json!({"size": 20}),
    )
    .await;
    assert_eq!(
        a.body["hits"]["total"],
        json!({"value": 5, "relation": "gte"})
    );
    assert_eq!(hits(&a).len(), 2);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn field_sort_returns_sort_values_and_null_scores() {
    let es = Es::start().await;
    bulk_index(
        &es,
        "chat",
        &[
            (
                "b",
                json!({"session_id": "s", "created_at": "2026-09-24T10:00:01.000Z"}),
            ),
            (
                "a",
                json!({"session_id": "s", "created_at": "2026-09-24T10:00:00.000Z"}),
            ),
            (
                "c",
                json!({"session_id": "t", "created_at": "2026-09-24T10:00:02.000Z"}),
            ),
        ],
    )
    .await;
    let a = search(
        &es,
        "/chat/_search?sort=created_at:asc",
        json!({"query": {"term": {"session_id": "s"}}, "size": 100,
               "version": true, "seq_no_primary_term": true}),
    )
    .await;
    assert_eq!(ids(&a), ["a", "b"], "{}", a.text);
    assert_eq!(hits(&a)[0]["sort"], json!([1_790_244_000_000_i64]));
    assert_eq!(hits(&a)[1]["sort"], json!([1_790_244_001_000_i64]));
    for hit in hits(&a) {
        assert_eq!(hit["_score"], Value::Null);
        assert!(hit["_version"].as_u64().unwrap() >= 1, "{hit}");
        assert_eq!(
            hit["_version"].as_u64(),
            hit["_seq_no"].as_u64().map(|n| n + 1)
        );
        assert_eq!(hit["_primary_term"], 1);
    }
    assert_eq!(a.body["hits"]["max_score"], Value::Null);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn count_matches_search_total() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let query = json!({"term": {"metadata.page": 1}});
    let count = Es::ok(es.post("/lc/_count", json!({"query": query})).await);
    assert_eq!(count.body["_shards"]["total"], 1);
    let a = search(
        &es,
        "/lc/_search",
        json!({"query": query, "track_total_hits": true}),
    )
    .await;
    assert_eq!(count.body["count"], a.body["hits"]["total"]["value"]);
    assert_eq!(count.body["count"], 1);
    let a = Es::ok(es.get("/lc/_count?q=text:foo").await);
    assert_eq!(a.body["count"], 1, "{}", a.text);
    let a = Es::ok(es.get("/lc/_count").await);
    assert_eq!(a.body["count"], 3, "{}", a.text);
    let a = es
        .post("/lc/_count", json!({"query": {"match_all": {}}, "size": 1}))
        .await;
    a.assert_error(400, "parsing_exception", None);
    let a = es
        .post(
            "/lc/_count",
            json!({"query": {"knn": knn("vector", &fake(0), 1)}}),
        )
        .await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some("[knn] is not supported by the count API"),
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn search_and_count_through_an_alias_cover_every_member() {
    let es = Es::start().await;
    cache_fixture(&es).await;
    Es::ok(es.put("/test_index3", None).await);
    Es::ok(
        es.post(
            "/_aliases",
            json!({"actions": [{"add": {"index": "test_index3", "alias": "test_alias"}}]}),
        )
        .await,
    );
    // The cache maps its fields through the alias, on every member
    // (`_manage_cache_index`, C49); a member without `timestamp` would fail
    // the sorted search (row T9-6).
    Es::ok(
        es.put(
            "/test_alias/_mapping",
            Some(json!({"properties": {
                "llm_output": {"type": "text", "index": false},
                "timestamp": {"type": "date"}}})),
        )
        .await,
    );
    let keys = ["k1", "k2", "k3", "k4", "k5"];
    for (index, n, ts) in [
        ("test_index2", 3, "2026-01-01T00:00:00Z"),
        ("test_index3", 5, "2026-01-02T00:00:00Z"),
    ] {
        for key in &keys[..n] {
            let a = es
                .put(
                    &format!("/{index}/_doc/{key}?refresh=true"),
                    Some(json!({"llm_output": key, "timestamp": ts})),
                )
                .await;
            assert!(a.status.is_success(), "{}", a.text);
        }
    }
    let a = Es::ok(
        es.post("/test_alias/_count", json!({"query": {"match_all": {}}}))
            .await,
    );
    assert_eq!(a.body["count"], 8, "{}", a.text);
    assert_eq!(a.body["_shards"]["total"], 3);
    let a = search(
        &es,
        "/test_alias/_search",
        json!({"query": {"ids": {"values": keys}}, "size": 10}),
    )
    .await;
    let order: Vec<(String, String)> = hits(&a)
        .iter()
        .map(|h| {
            (
                h["_id"].as_str().unwrap().into(),
                h["_index"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(order.len(), 8, "{}", a.text);
    let mut sorted = order.clone();
    sorted.sort();
    assert_eq!(order, sorted, "equal scores: by _id, then _index");
    assert_eq!(a.body["hits"]["total"]["value"], 8);
    let a = search(
        &es,
        "/test_alias/_search",
        json!({"query": {"term": {"_id": "k2"}}, "sort": {"timestamp": {"order": "asc"}}}),
    )
    .await;
    let indices: Vec<&str> = hits(&a)
        .iter()
        .map(|h| h["_index"].as_str().unwrap())
        .collect();
    assert_eq!(indices, ["test_index2", "test_index3"], "{}", a.text);
    assert_eq!(ids(&a), ["k2", "k2"]);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn hybrid_through_a_multi_index_alias_is_refused() {
    let es = Es::start().await;
    langchain(&es, "h1", true).await;
    langchain(&es, "h2", true).await;
    Es::ok(
        es.post(
            "/_aliases",
            json!({"actions": [
                {"add": {"index": "h1", "alias": "two"}},
                {"add": {"index": "h2", "alias": "two"}},
                {"add": {"index": "h1", "alias": "one"}}]}),
        )
        .await,
    );
    let body = json!({"query": {"match": {"text": "foo"}}, "knn": knn("vector", &fake(0), 3)});
    let a = es.post("/two/_search", body.clone()).await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some("Loams does not support [hybrid search over several indices] (Elasticsearch API Phase A)"),
    );
    let one = search(&es, "/one/_search", body.clone()).await;
    let direct = search(&es, "/h1/_search", body).await;
    assert_eq!(ids(&one), ids(&direct));
    assert_eq!(scores(&one), scores(&direct));
    assert!(hits(&one).iter().all(|h| h["_index"] == "h1"));
    es.server.shutdown().await.expect("shutdown");
}

async fn msearch(es: &Es, path: &str, lines: &[Value]) -> Answer {
    let a = es
        .send_raw(
            Method::POST,
            path,
            Some(("application/x-ndjson", ndjson(lines))),
            &[],
        )
        .await;
    assert_eq!(a.status, StatusCode::OK, "{}", a.text);
    a
}

#[tokio::test]
async fn msearch_answers_in_order_with_per_item_errors() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let a = msearch(
        &es,
        "/lc/_msearch",
        &[
            json!({"index": "lc"}),
            json!({"query": {"match": {"text": "foo"}}}),
            json!({"index": "missing"}),
            json!({"query": {"match_all": {}}}),
            json!({}),
            json!({"query": {"match_all": {}}}),
        ],
    )
    .await;
    let responses = a.body["responses"].as_array().expect("responses");
    assert_eq!(responses.len(), 3, "{}", a.text);
    assert_eq!(responses[0]["status"], 200);
    assert_eq!(responses[0]["hits"]["hits"][0]["_id"], "1");
    assert_eq!(responses[1]["status"], 404);
    assert_eq!(responses[1]["error"]["type"], "index_not_found_exception");
    assert_eq!(responses[2]["status"], 200);
    assert_eq!(responses[2]["hits"]["total"]["value"], 3);
    assert!(a.body["took"].is_u64());
    let a = es
        .send_raw(
            Method::POST,
            "/_msearch",
            Some(("application/x-ndjson", b"\n".to_vec())),
            &[],
        )
        .await;
    a.assert_error(
        400,
        "action_request_validation_exception",
        Some("Validation Failed: 1: no requests added;"),
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn msearch_accepts_the_beir_batch() {
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/beir-d",
            Some(
                json!({"settings": {"number_of_shards": 1}, "mappings": {"properties": {
                "title": {"type": "text", "analyzer": "english"},
                "txt": {"type": "text", "analyzer": "english"}}}}),
            ),
        )
        .await,
    );
    let words = [
        "solar", "energy", "wind", "power", "grid", "storage", "battery", "cells",
    ];
    let docs: Vec<(String, Value)> = (0..20)
        .map(|i| {
            (
                format!("doc{i}"),
                json!({"title": format!("{} {}", words[i % 8], words[(i + 3) % 8]),
                       "txt": format!("{} {} {}", words[(i * 3) % 8], words[(i + 1) % 8], words[i % 5])}),
            )
        })
        .collect();
    let docs: Vec<(&str, Value)> = docs
        .iter()
        .map(|(id, d)| (id.as_str(), d.clone()))
        .collect();
    bulk_index(&es, "beir-d", &docs).await;
    let queries = [
        "solar power",
        "wind grid",
        "battery storage",
        "energy",
        "cells solar wind",
    ];
    let body = |q: &str| {
        json!({"_source": false, "size": 1001, "query": {"multi_match": {
            "query": q, "type": "best_fields", "fields": ["title", "txt"], "tie_breaker": 0.5}}})
    };
    let mut lines = Vec::new();
    for q in queries {
        lines.push(json!({"index": "beir-d", "search_type": "dfs_query_then_fetch"}));
        lines.push(body(q));
    }
    let a = msearch(&es, "/_msearch", &lines).await;
    let responses = a.body["responses"].as_array().expect("responses");
    assert_eq!(responses.len(), 5, "{}", a.text);
    for (response, q) in responses.iter().zip(queries) {
        assert_eq!(response["status"], 200, "{response}");
        let direct = search(&es, "/beir-d/_search", body(q)).await;
        let got: Vec<(String, f64)> = response["hits"]["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| {
                assert!(h.get("_source").is_none(), "{h}");
                (
                    h["_id"].as_str().unwrap().into(),
                    h["_score"].as_f64().unwrap(),
                )
            })
            .collect();
        let want: Vec<(String, f64)> = hits(&direct)
            .iter()
            .map(|h| {
                (
                    h["_id"].as_str().unwrap().into(),
                    h["_score"].as_f64().unwrap(),
                )
            })
            .collect();
        assert!(!got.is_empty(), "{q}");
        assert_eq!(got, want, "{q}");
    }
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn missing_and_unmatched_indices_follow_the_options() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let a = es.post("/missing/_search", json!({})).await;
    a.assert_error(
        404,
        "index_not_found_exception",
        Some("no such index [missing]"),
    );
    let a = search(
        &es,
        "/lc,missing/_search?ignore_unavailable=true",
        json!({}),
    )
    .await;
    assert_eq!(a.body["hits"]["total"]["value"], 3, "{}", a.text);
    let a = search(&es, "/missing/_search?ignore_unavailable=true", json!({})).await;
    assert_eq!(a.body["_shards"]["total"], 0);
    assert_eq!(a.body["hits"]["hits"], json!([]));
    let a = search(&es, "/nomatch*/_search", json!({})).await;
    assert_eq!(a.body["hits"]["total"]["value"], 0, "{}", a.text);
    let a = es
        .post("/nomatch*/_search?allow_no_indices=false", json!({}))
        .await;
    a.assert_error(404, "index_not_found_exception", None);
    let a = es.post("/missing/_count", json!({})).await;
    a.assert_error(404, "index_not_found_exception", None);
    let a = Es::ok(
        es.post("/missing/_count?ignore_unavailable=true", json!({}))
            .await,
    );
    assert_eq!(a.body["count"], 0);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_query_string_syntax_error_is_a_parse_failure() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let a = es
        .post(
            "/lc/_search",
            json!({"query": {"query_string": {"query": "text:(foo"}}}),
        )
        .await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    assert_eq!(a.body["error"]["type"], "search_phase_execution_exception");
    let root = &a.body["error"]["root_cause"][0];
    assert_eq!(root["type"], "query_shard_exception", "{}", a.text);
    assert_eq!(root["reason"], "Failed to parse query [text:(foo]");
    assert_eq!(root["index"], "lc");
    let a = es.get("/lc/_count?q=text:(foo").await;
    assert_eq!(a.status, StatusCode::BAD_REQUEST, "{}", a.text);
    assert_eq!(
        a.body["error"]["root_cause"][0]["reason"],
        "Failed to parse query [text:(foo]"
    );
    es.server.shutdown().await.expect("shutdown");
}

/// `multi_match` scores with BM25 (best_fields: the best field's score) and
/// ranks; with no `fields`, and `query_string` with no `default_field`, search
/// every text field (`index.query.default_field` is `*`).
#[tokio::test]
async fn multi_match_and_query_string_score_over_every_text_field() {
    let es = Es::start().await;
    let docs: Vec<(&str, Value)> = vec![
        (
            "1",
            json!({"title": "Cold River", "body": "a river in the mountains"}),
        ),
        (
            "2",
            json!({"title": "Saline Creek", "body": "a long river and its basin"}),
        ),
        (
            "3",
            json!({"title": "Green Hill", "body": "a hill by a lake"}),
        ),
    ];
    bulk_index(&es, "mm", &docs).await;
    let scored = |a: &Answer, what: &str| {
        let mut got = ids(a);
        got.sort();
        assert_eq!(got, ["1", "2"], "{what}: {}", a.text);
        let s: Vec<f64> = hits(a)
            .iter()
            .map(|h| h["_score"].as_f64().unwrap())
            .collect();
        assert!(s.iter().all(|s| *s > 0.0), "{what}: {}", a.text);
        assert!(s.windows(2).all(|w| w[0] >= w[1]), "{what}: {}", a.text);
        assert_eq!(a.body["hits"]["max_score"].as_f64(), Some(s[0]), "{what}");
        scores(a)
    };
    let title = scores(
        &search(
            &es,
            "/mm/_search",
            json!({"query": {"match": {"title": "river"}}}),
        )
        .await,
    );
    let body = scores(
        &search(
            &es,
            "/mm/_search",
            json!({"query": {"match": {"body": "river"}}}),
        )
        .await,
    );
    let of = |m: &BTreeMap<String, f64>, id: &str| m.get(id).copied().unwrap_or(0.0);

    let best = search(
        &es,
        "/mm/_search",
        json!({"query": {"multi_match": {"query": "river", "fields": ["title", "body"]}}}),
    )
    .await;
    let best = scored(&best, "multi_match");
    for (id, score) in &best {
        let want = of(&title, id).max(of(&body, id));
        assert!(
            close(*score, want, 1e-4),
            "best_fields {id}: {score} vs {want}"
        );
    }
    // Doc 1 has "river" in both fields and outranks doc 2.
    assert!(best["1"] > best["2"], "{best:?}");

    let most = search(
        &es,
        "/mm/_search",
        json!({"query": {"multi_match": {"query": "river", "fields": ["title", "body"],
                                        "type": "most_fields"}}}),
    )
    .await;
    for (id, score) in scored(&most, "most_fields") {
        let want = of(&title, &id) + of(&body, &id);
        assert!(
            close(score, want, 1e-4),
            "most_fields {id}: {score} vs {want}"
        );
    }

    for (what, query) in [
        (
            "multi_match without fields",
            json!({"multi_match": {"query": "river"}}),
        ),
        (
            "query_string without default_field",
            json!({"query_string": {"query": "river"}}),
        ),
        (
            "query_string default_field *",
            json!({"query_string": {"query": "river", "default_field": "*"}}),
        ),
    ] {
        let a = search(&es, "/mm/_search", json!({ "query": query })).await;
        scored(&a, what);
    }
    es.server.shutdown().await.expect("shutdown");
}

/// `_source` filtering on `_search`, in each spelling Elasticsearch takes.
#[tokio::test]
async fn search_source_filtering_restricts_each_hit() {
    let es = Es::start().await;
    let docs: Vec<(&str, Value)> = vec![
        (
            "1",
            json!({"title": "Cold River", "body": "a river", "meta": {"a": 1, "b": 2}}),
        ),
        (
            "2",
            json!({"title": "Saline River", "body": "a creek", "meta": {"a": 3, "b": 4}}),
        ),
    ];
    bulk_index(&es, "src", &docs).await;
    let query = json!({"match": {"title": "river"}});
    let keys = |a: &Answer| -> Vec<Vec<String>> {
        hits(a)
            .iter()
            .map(|h| match h.get("_source").and_then(Value::as_object) {
                Some(source) => {
                    let mut keys: Vec<String> = source.keys().cloned().collect();
                    keys.sort();
                    keys
                }
                None => vec!["<none>".to_string()],
            })
            .collect()
    };
    for (source, want) in [
        (json!(["title"]), vec!["title"]),
        (json!("title"), vec!["title"]),
        (
            json!({"includes": ["title", "meta.a"]}),
            vec!["meta", "title"],
        ),
        (json!({"excludes": ["body", "meta"]}), vec!["title"]),
        (
            json!({"includes": ["meta"], "excludes": ["meta.b"]}),
            vec!["meta"],
        ),
        (json!(false), vec!["<none>"]),
    ] {
        let a = search(
            &es,
            "/src/_search",
            json!({"query": query, "_source": source}),
        )
        .await;
        let got = keys(&a);
        assert_eq!(got.len(), 2, "{source}: {}", a.text);
        for hit in got {
            assert_eq!(hit, want, "{source}: {}", a.text);
        }
        if source == json!({"includes": ["meta"], "excludes": ["meta.b"]}) {
            assert_eq!(hits(&a)[0]["_source"]["meta"].as_object().unwrap().len(), 1);
        }
    }
    // The URL parameters.
    for path in [
        "/src/_search?_source_includes=title",
        "/src/_search?_source=title",
        "/src/_search?_source_excludes=body,meta",
    ] {
        let a = search(&es, path, json!({"query": query})).await;
        for hit in keys(&a) {
            assert_eq!(hit, ["title"], "{path}: {}", a.text);
        }
    }
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn min_score_and_boost_shape_the_scores() {
    let es = Es::start().await;
    langchain(&es, "lc", true).await;
    let all = search(
        &es,
        "/lc/_search",
        json!({"knn": knn("vector", &fake(0), 3), "size": 3}),
    )
    .await;
    let cos = scores(&all);
    // A knn boost multiplies the ES score; min_score is in boosted space.
    let mut boosted = knn("vector", &fake(0), 3);
    boosted["boost"] = json!(2.0);
    let threshold = 2.0 * cos["2"] - 1e-4;
    let a = search(
        &es,
        "/lc/_search",
        json!({"knn": boosted, "size": 3, "min_score": threshold}),
    )
    .await;
    assert_eq!(ids(&a), ["1", "2"], "{}", a.text);
    for (id, score) in scores(&a) {
        assert!(close(score, 2.0 * cos[&id], 1e-5), "{id}");
    }
    // Query-only min_score.
    let a = search(
        &es,
        "/lc/_search",
        json!({"query": {"match": {"text": "foo"}}, "min_score": 1000.0}),
    )
    .await;
    assert!(hits(&a).is_empty(), "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_field_after_a_score_sort_is_refused() {
    let es = Es::start().await;
    bulk_index(
        &es,
        "tie",
        &[
            ("b", json!({"text": "foo", "n": 3})),
            ("a", json!({"text": "foo", "n": 1})),
        ],
    )
    .await;
    // The engine breaks score ties by PK only (PR #74 review).
    let a = es
        .post(
            "/tie/_search",
            json!({"query": {"match": {"text": "foo"}}, "sort": ["_score", {"n": "desc"}]}),
        )
        .await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some("Loams does not support [sort on a field after _score] (Elasticsearch API Phase A)"),
    );
    let a = search(
        &es,
        "/tie/_search",
        json!({"query": {"match": {"text": "foo"}}, "sort": ["_score"]}),
    )
    .await;
    assert_eq!(ids(&a), ["a", "b"], "{}", a.text);
    for hit in hits(&a) {
        assert_eq!(hit["sort"], json!([hit["_score"]]), "{hit}");
    }
    es.server.shutdown().await.expect("shutdown");
}

// ----- sort on _seq_no (#949) -----

/// The `_id`s of a search answer, in order.
fn seq_ids(a: &Answer) -> Vec<String> {
    a.body["hits"]["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|h| h["_id"].as_str().expect("_id").to_string())
        .collect()
}

#[tokio::test]
async fn sort_on_seq_no_orders_by_write() {
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/meta",
            // One partition: `_seq_no` orders writes within a partition only,
            // as ES's does within a shard (ES's default is one shard).
            Some(json!({"settings": {"number_of_shards": 1},
                        "mappings": {"properties": {"collection_name": {"type": "keyword"}}}})),
        )
        .await,
    );
    // Written in the order a, b, c, then a again: a is now the newest.
    for (id, c, v) in [
        ("a", "c1", 1),
        ("b", "c1", 2),
        ("c", "c2", 3),
        ("a", "c1", 4),
    ] {
        let a = es
            .put(
                &format!("/meta/_doc/{id}?refresh=true"),
                Some(json!({"collection_name": c, "v": v})),
            )
            .await;
        assert!(a.status.is_success(), "{}", a.text);
    }
    // The NVIDIA RAG Blueprint's "newest write" read.
    let a = Es::ok(
        es.post(
            "/meta/_search",
            json!({"query": {"term": {"collection_name": "c1"}},
                   "sort": [{"_seq_no": {"order": "desc"}}], "size": 1}),
        )
        .await,
    );
    assert_eq!(seq_ids(&a), ["a"], "{}", a.text);
    let hit = &a.body["hits"]["hits"][0];
    assert_eq!(hit["_source"]["v"], 4);
    assert!(hit["_score"].is_null());
    assert!(hit["sort"][0].is_u64(), "{}", a.text);
    assert!(hit.get("_seq_no").is_none());
    assert_eq!(a.body["hits"]["total"]["value"], 2);
    assert!(a.body["hits"]["max_score"].is_null());
    // Ascending, every form of the key, paging.
    for sort in [
        json!("_seq_no"),
        json!(["_seq_no"]),
        json!({"_seq_no": "asc"}),
    ] {
        let a = Es::ok(es.post("/meta/_search", json!({"sort": sort})).await);
        assert_eq!(seq_ids(&a), ["b", "c", "a"], "{sort}");
    }
    let a = Es::ok(
        es.post(
            "/meta/_search",
            json!({"sort": [{"_seq_no": "desc"}], "from": 1, "size": 1, "seq_no_primary_term": true}),
        )
        .await,
    );
    assert_eq!(seq_ids(&a), ["c"]);
    let hit = &a.body["hits"]["hits"][0];
    assert_eq!(hit["_seq_no"], hit["sort"][0]);
    // The URL form.
    let a = Es::ok(es.get("/meta/_search?sort=_seq_no:desc&size=2").await);
    assert_eq!(seq_ids(&a), ["a", "c"]);
    // With another key it is refused.
    es.post(
        "/meta/_search",
        json!({"sort": [{"_seq_no": "desc"}, {"collection_name": "asc"}]}),
    )
    .await
    .assert_error(400, "illegal_argument_exception", None);
}

// ----- aggregations (#951) -----

/// The NVIDIA RAG Blueprint's chunk index: `metadata.source.source_name`
/// mapped dynamically (text with a `keyword` subfield).
async fn blueprint_chunks(es: &Es) {
    let docs: Vec<(&str, Value)> = vec![
        (
            "c1",
            json!({"text": "one", "metadata": {"source": {"source_name": "a.pdf"}, "page": 1}}),
        ),
        (
            "c2",
            json!({"text": "two", "metadata": {"source": {"source_name": "a.pdf"}, "page": 2}}),
        ),
        (
            "c3",
            json!({"text": "three", "metadata": {"source": {"source_name": "b.pdf"}, "page": 1}}),
        ),
        (
            "c4",
            json!({"text": "four", "metadata": {"source": {"source_name": "c.pdf"}, "page": 1}}),
        ),
    ];
    bulk_index(es, "chunks", &docs).await;
}

#[tokio::test]
async fn composite_with_top_hits_pages_like_the_blueprint() {
    let es = Es::start().await;
    blueprint_chunks(&es).await;
    let body = |after: Option<Value>| {
        let mut composite = json!({"size": 2, "sources": [{"source_name": {"terms": {"field": "metadata.source.source_name.keyword"}}}]});
        if let Some(after) = after {
            composite["after"] = after;
        }
        json!({"size": 0, "aggs": {"unique_sources": {
            "composite": composite,
            "aggs": {"top_hit": {"top_hits": {"size": 1}}}}}})
    };
    let mut names = Vec::new();
    let mut after = None;
    for _ in 0..5 {
        let a = Es::ok(es.post("/chunks/_search", body(after.clone())).await);
        assert!(a.body["hits"]["hits"].as_array().expect("hits").is_empty());
        let agg = &a.body["aggregations"]["unique_sources"];
        let buckets = agg["buckets"].as_array().expect("buckets");
        if buckets.is_empty() {
            break;
        }
        for b in buckets {
            let name = b["key"]["source_name"].as_str().expect("key").to_string();
            let hits = &b["top_hit"]["hits"];
            assert_eq!(hits["total"]["value"], b["doc_count"], "{}", a.text);
            assert!(hits["max_score"].is_null());
            let hit = &hits["hits"][0];
            assert_eq!(hit["_index"], "chunks");
            assert_eq!(
                hit["_source"]["metadata"]["source"]["source_name"],
                name.as_str()
            );
            names.push((name, b["doc_count"].as_u64().expect("doc_count")));
        }
        after = agg.get("after_key").cloned().filter(|v| !v.is_null());
        if after.is_none() {
            break;
        }
    }
    assert_eq!(
        names,
        [
            ("a.pdf".to_string(), 2),
            ("b.pdf".to_string(), 1),
            ("c.pdf".to_string(), 1)
        ]
    );
}

#[tokio::test]
async fn terms_and_metrics_with_a_query() {
    let es = Es::start().await;
    blueprint_chunks(&es).await;
    let a = Es::ok(
        es.post(
            "/chunks/_search",
            json!({"size": 1, "query": {"term": {"metadata.page": 1}},
                   "aggs": {"names": {"terms": {"field": "metadata.source.source_name.keyword"}},
                            "pages": {"max": {"field": "metadata.page"}}}}),
        )
        .await,
    );
    assert_eq!(a.body["hits"]["hits"].as_array().expect("hits").len(), 1);
    let buckets: BTreeMap<String, u64> = a.body["aggregations"]["names"]["buckets"]
        .as_array()
        .expect("buckets")
        .iter()
        .map(|b| {
            (
                b["key"].as_str().expect("key").to_string(),
                b["doc_count"].as_u64().expect("n"),
            )
        })
        .collect();
    assert_eq!(
        buckets,
        BTreeMap::from([
            ("a.pdf".to_string(), 1),
            ("b.pdf".to_string(), 1),
            ("c.pdf".to_string(), 1)
        ])
    );
    assert_eq!(a.body["aggregations"]["pages"]["value"], 1.0);
}

#[tokio::test]
async fn aggregation_refusals_and_unmapped_fields() {
    let es = Es::start().await;
    blueprint_chunks(&es).await;
    // An unmapped field answers ES's empty result.
    let a = Es::ok(
        es.post(
            "/chunks/_search",
            json!({"size": 0, "aggs": {
                "t": {"terms": {"field": "nope"}},
                "c": {"composite": {"sources": [{"x": {"terms": {"field": "nope"}}}]}},
                "m": {"max": {"field": "nope"}}}}),
        )
        .await,
    );
    assert_eq!(
        a.body["aggregations"]["t"]["buckets"],
        json!([]),
        "{}",
        a.text
    );
    assert_eq!(a.body["aggregations"]["c"]["buckets"], json!([]));
    assert!(a.body["aggregations"]["m"]["value"].is_null());
    // A text field needs fielddata, as in ES.
    es.post(
        "/chunks/_search",
        json!({"aggs": {"t": {"terms": {"field": "text"}}}}),
    )
    .await
    .assert_error(400, "illegal_argument_exception", None);
    // A type the engine does not serve, and several indices.
    es.post(
        "/chunks/_search",
        json!({"aggs": {"t": {"significant_terms": {"field": "metadata.page"}}}}),
    )
    .await
    .assert_error(400, "illegal_argument_exception", None);
    bulk_index(&es, "other", &[("o", json!({"n": 1}))]).await;
    es.post(
        "/chunks,other/_search",
        json!({"aggs": {"m": {"max": {"field": "n"}}}}),
    )
    .await
    .assert_error(400, "illegal_argument_exception", None);
}
