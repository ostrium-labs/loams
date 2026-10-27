//! Task 3: index, mapping and alias administration and `_refresh`.
//!
//! Where the plan checks a write index, `write_index` asks
//! `names::resolve_write` and also writes through the alias with
//! `PUT /{alias}/_doc/{id}?require_alias=true` (rows T3-5, T4-9).

use std::sync::atomic::{AtomicU64, Ordering};

use loams_es::mapping::index_uuid;
use loams_es::names::resolve_write;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::harness::Es;

const NS: &str = "default";

/// The C5 LangChain dense mapping, 16 dims, cosine.
fn langchain_dense() -> Value {
    json!({
        "mappings": {"properties": {
            "vector": {"type": "dense_vector", "dims": 16, "index": true, "similarity": "cosine"},
        }},
        "settings": {},
    })
}

/// The C50 LLM-cache mapping.
fn llm_cache_mapping() -> Value {
    json!({"properties": {
        "llm_output": {"type": "text", "index": false},
        "llm_params": {"type": "text", "index": false},
        "llm_input": {"type": "text", "index": false},
        "metadata": {"type": "object"},
        "timestamp": {"type": "date"},
    }})
}

/// Where a write through `alias` goes: `Ok(index)`, or the error's reason.
/// It asks `names::resolve_write`, then writes a new document with
/// `PUT /{alias}/_doc/<n>?require_alias=true` and checks that the write
/// lands (or fails) the same way (C49).
async fn write_index(es: &Es, alias: &str) -> Result<String, String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let resolved = match resolve_write(&es.server.collections(), NS, alias).await {
        Ok(Some(target)) => Ok(target.name),
        Ok(None) => Err("no such index or alias".to_string()),
        Err(err) => Err(err.reason),
    };
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let a = es
        .put(
            &format!("/{alias}/_doc/w{n}?require_alias=true&refresh=true"),
            Some(json!({"n": n})),
        )
        .await;
    let written = if a.status == StatusCode::CREATED {
        Ok(a.body["_index"].as_str().unwrap_or_default().to_string())
    } else {
        Err(a.body["error"]["reason"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    };
    assert_eq!(written, resolved, "{}", a.text);
    resolved
}

/// The `es_env_fx` fixture of LangChain's `test_cache.py`.
async fn cache_fixture(es: &Es) {
    Es::ok(es.put("/test_index1", None).await);
    Es::ok(es.put("/test_index2", None).await);
    Es::ok(es.put("/test_index1/_alias/test_alias", None).await);
    Es::ok(
        es.put(
            "/test_index2/_alias/test_alias",
            Some(json!({"is_write_index": true})),
        )
        .await,
    );
}

#[tokio::test]
async fn create_index_with_langchain_dense_mapping() {
    let es = Es::start().await;
    let a = Es::ok(es.put("/test_a", Some(langchain_dense())).await);
    assert_eq!(
        a.body,
        json!({"acknowledged": true, "shards_acknowledged": true, "index": "test_a"})
    );
    let a = Es::ok(es.get("/test_a").await);
    let index = &a.body["test_a"];
    assert_eq!(
        index["mappings"]["properties"]["vector"],
        json!({"type": "dense_vector", "dims": 16, "index": true, "similarity": "cosine"})
    );
    let info = es
        .server
        .collections()
        .get_collection(NS, "test_a")
        .await
        .expect("collection");
    let settings = &index["settings"]["index"];
    assert_eq!(settings["number_of_shards"], info.partitions.to_string());
    assert_eq!(settings["uuid"], index_uuid(info.id));
    assert_eq!(settings["provided_name"], "test_a");
    assert_eq!(index["aliases"], json!({}));
    // number_of_shards is the partition count (BEIR, C47).
    Es::ok(
        es.put(
            "/beir-x",
            Some(
                json!({"settings": {"number_of_shards": 1}, "mappings": {"properties": {
                    "title": {"type": "text", "analyzer": "english"},
                }}}),
            ),
        )
        .await,
    );
    let a = Es::ok(es.get("/beir-x?flat_settings=true").await);
    assert_eq!(a.body["beir-x"]["settings"]["index.number_of_shards"], "1");
    // A bad mapping and a bad name are refused.
    let a = es
        .put(
            "/bad",
            Some(json!({"mappings": {"properties": {"x": {"type": "nested"}}}})),
        )
        .await;
    a.assert_error(400, "mapper_parsing_exception", None);
    let a = es.put("/Bad", None).await;
    a.assert_error(400, "invalid_index_name_exception", None);
    let a = es.put("/bad", Some(json!({"mapping": {}}))).await;
    a.assert_error(
        400,
        "parse_exception",
        Some("unknown key [mapping] for create index"),
    );
    let a = es.head("/bad").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn creating_an_existing_index_is_resource_already_exists() {
    let es = Es::start().await;
    Es::ok(es.put("/test_a", Some(langchain_dense())).await);
    let a = es.put("/test_a", Some(langchain_dense())).await;
    a.assert_error(400, "resource_already_exists_exception", None);
    let uuid = index_uuid(
        es.server
            .collections()
            .get_collection(NS, "test_a")
            .await
            .expect("collection")
            .id,
    );
    assert_eq!(a.body["error"]["index"], "test_a");
    assert_eq!(a.body["error"]["index_uuid"], uuid);
    assert_eq!(
        a.body["error"]["reason"],
        format!("index [test_a/{uuid}] already exists")
    );
    // An alias's name is not a free index name.
    Es::ok(es.put("/test_a/_alias/al", None).await);
    let a = es.put("/al", None).await;
    a.assert_error(
        400,
        "invalid_index_name_exception",
        Some("Invalid index name [al], already exists as alias"),
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn head_index_is_200_or_404_without_body() {
    let es = Es::start().await;
    Es::ok(es.put("/test_a", None).await);
    let a = es.head("/test_a").await;
    assert_eq!(a.status, StatusCode::OK);
    assert!(a.text.is_empty(), "{}", a.text);
    assert_eq!(a.header("x-elastic-product"), "Elasticsearch");
    let a = es.head("/nope").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert!(a.text.is_empty(), "{}", a.text);
    assert_eq!(es.head("/test_*").await.status, StatusCode::OK);
    assert_eq!(es.head("/zz_*").await.status, StatusCode::NOT_FOUND);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn get_mapping_returns_the_langchain_metadata_mapping_verbatim() {
    let es = Es::start().await;
    let body = json!({"mappings": {"properties": {"metadata": {"properties": {
        "category": {"type": "keyword"},
        "score": {"type": "float"},
        "tags": {"type": "text"},
    }}}}});
    Es::ok(es.put("/i", Some(body)).await);
    // One document with a dynamic metadata.page.
    let source = json!({"metadata": {"page": 1, "category": "c"}});
    let a = es.put("/i/_doc/1", Some(source)).await;
    assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    let a = Es::ok(es.get("/i/_mapping").await);
    let metadata = &a.body["i"]["mappings"]["properties"]["metadata"]["properties"];
    assert_eq!(metadata["category"], json!({"type": "keyword"}));
    assert_eq!(metadata["score"], json!({"type": "float"}));
    assert_eq!(metadata["tags"], json!({"type": "text"}));
    assert_eq!(metadata["page"], json!({"type": "long"}));
    let a = Es::ok(es.get("/_mapping").await);
    assert!(
        a.body["i"]["mappings"]["properties"].is_object(),
        "{}",
        a.text
    );
    let a = es.get("/nope/_mapping").await;
    a.assert_error(404, "index_not_found_exception", None);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn get_all_lists_every_index() {
    let es = Es::start().await;
    Es::ok(es.put("/test_1", None).await);
    Es::ok(es.put("/test_2", None).await);
    let a = Es::ok(es.get("/_all").await);
    let keys: Vec<&String> = a.body.as_object().expect("object").keys().collect();
    assert!(keys.contains(&&"test_1".to_string()) && keys.contains(&&"test_2".to_string()));
    Es::ok(es.delete("/test_1").await);
    let a = Es::ok(es.get("/_all").await);
    assert!(a.body.get("test_1").is_none(), "{}", a.text);
    assert!(a.body.get("test_2").is_some(), "{}", a.text);
    let a = Es::ok(es.get("/test_2?features=aliases").await);
    assert_eq!(a.body, json!({"test_2": {"aliases": {}}}));
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn wildcard_and_all_deletes_are_refused() {
    let es = Es::start().await;
    for name in ["test_1", "test_2", "other"] {
        Es::ok(es.put(&format!("/{name}"), None).await);
    }
    for path in ["/test_*", "/_all", "/*", "/test_1,other_*"] {
        let a = es.delete(path).await;
        a.assert_error(
            400,
            "illegal_argument_exception",
            Some("Wildcard expressions or all indices are not allowed"),
        );
    }
    for name in ["test_1", "test_2", "other"] {
        assert_eq!(es.head(&format!("/{name}")).await.status, StatusCode::OK);
    }
    // An alias is not deleted as an index.
    Es::ok(es.put("/other/_alias/al", None).await);
    let a = es.delete("/al").await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some(
            "The provided expression [al] matches an alias, specify the corresponding concrete \
             indices instead.",
        ),
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_comma_list_delete_deletes_every_index() {
    let es = Es::start().await;
    Es::ok(es.put("/test_1", None).await);
    Es::ok(es.put("/test_2", None).await);
    let a = Es::ok(es.delete("/test_1,test_2").await);
    assert_eq!(a.body, json!({"acknowledged": true}));
    assert_eq!(es.head("/test_1").await.status, StatusCode::NOT_FOUND);
    assert_eq!(es.head("/test_2").await.status, StatusCode::NOT_FOUND);
    Es::ok(es.put("/test_1", None).await);
    let a = es.delete("/test_1,nope").await;
    a.assert_error(
        404,
        "index_not_found_exception",
        Some("no such index [nope]"),
    );
    // ES resolves the whole list first: nothing is deleted (row T11-3).
    assert_eq!(es.head("/test_1").await.status, StatusCode::OK);
    Es::ok(es.delete("/test_1,nope?ignore_unavailable=true").await);
    assert_eq!(es.head("/test_1").await.status, StatusCode::NOT_FOUND);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn refresh_all_is_a_noop_success() {
    let es = Es::start().await;
    Es::ok(es.put("/a", None).await);
    Es::ok(es.put("/b", None).await);
    let a = Es::ok(es.post("/_all/_refresh", json!({})).await);
    assert_eq!(
        a.body,
        json!({"_shards": {"total": 2, "successful": 2, "failed": 0}})
    );
    let a = Es::ok(es.get("/a/_refresh").await);
    assert_eq!(a.body["_shards"]["total"], 1);
    Es::ok(es.send(reqwest::Method::POST, "/_refresh", None, &[]).await);
    let a = es.get("/nope/_refresh").await;
    a.assert_error(404, "index_not_found_exception", None);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn aliases_repoint_atomically() {
    let es = Es::start().await;
    Es::ok(es.put("/test_1", None).await);
    Es::ok(es.put("/test_2", None).await);
    Es::ok(es.put("/test_1/_alias/al", None).await);
    let a = Es::ok(
        es.post(
            "/_aliases",
            json!({"actions": [
                {"remove": {"index": "test_1", "alias": "al"}},
                {"add": {"index": "test_2", "alias": "al"}},
            ]}),
        )
        .await,
    );
    assert_eq!(a.body, json!({"acknowledged": true, "errors": false}));
    let a = Es::ok(es.get("/_alias/al").await);
    assert_eq!(a.body, json!({"test_2": {"aliases": {"al": {}}}}));
    assert_eq!(es.head("/_alias/al").await.status, StatusCode::OK);
    assert_eq!(es.head("/_alias/zz").await.status, StatusCode::NOT_FOUND);
    let a = es.get("/_alias/zz").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND);
    assert_eq!(
        a.body,
        json!({"error": "alias [zz] missing", "status": 404})
    );
    // GET /_alias lists every index, with or without aliases.
    let a = Es::ok(es.get("/_alias").await);
    assert_eq!(
        a.body,
        json!({"test_1": {"aliases": {}}, "test_2": {"aliases": {"al": {}}}})
    );
    let a = Es::ok(es.get("/test_2/_alias/al").await);
    assert_eq!(a.body, json!({"test_2": {"aliases": {"al": {}}}}));
    // A failing action applies nothing.
    let a = es
        .post(
            "/_aliases",
            json!({"actions": [
                {"add": {"index": "test_1", "alias": "b"}},
                {"add": {"index": "missing", "alias": "b"}},
            ]}),
        )
        .await;
    a.assert_error(404, "index_not_found_exception", None);
    assert_eq!(es.head("/_alias/b").await.status, StatusCode::NOT_FOUND);
    let a = es
        .post(
            "/_aliases",
            json!({"actions": [{"add": {"index": "test_1", "alias": "b", "filter": {}}}]}),
        )
        .await;
    a.assert_error(400, "illegal_argument_exception", None);
    let a = es
        .post(
            "/_aliases",
            json!({"actions": [{"remove_index": {"index": "test_1"}}]}),
        )
        .await;
    a.assert_error(400, "illegal_argument_exception", None);
    let a = es
        .post(
            "/_aliases",
            json!({"actions": [{"add": {"index": "test_1", "alias": "test_2"}}]}),
        )
        .await;
    a.assert_error(
        400,
        "invalid_alias_name_exception",
        Some(
            "Invalid alias name [test_2]: an index or data stream exists with the same name as \
             the alias",
        ),
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn an_alias_can_name_two_indices_with_one_write_index() {
    let es = Es::start().await;
    cache_fixture(&es).await;
    let a = Es::ok(es.get("/_alias/test_alias").await);
    assert_eq!(
        a.body,
        json!({
            "test_index1": {"aliases": {"test_alias": {}}},
            "test_index2": {"aliases": {"test_alias": {"is_write_index": true}}},
        })
    );
    assert_eq!(es.head("/_alias/test_alias").await.status, StatusCode::OK);
    let a = Es::ok(es.get("/test_alias").await);
    let keys: Vec<&String> = a.body.as_object().expect("object").keys().collect();
    assert_eq!(keys, ["test_index1", "test_index2"]);
    assert_eq!(
        a.body["test_index2"]["aliases"],
        json!({"test_alias": {"is_write_index": true}})
    );
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index2".to_string())
    );
    let a = Es::ok(
        es.delete("/test_index1,test_index2/_alias/test_alias")
            .await,
    );
    assert_eq!(a.body, json!({"acknowledged": true}));
    assert_eq!(
        es.head("/_alias/test_alias").await.status,
        StatusCode::NOT_FOUND
    );
    // Deleting it again is 404, which the fixture ignores.
    let a = es
        .delete("/test_index1,test_index2/_alias/test_alias")
        .await;
    a.assert_error(
        404,
        "aliases_not_found_exception",
        Some("aliases [test_alias] missing"),
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn write_index_moves_like_elasticsearch() {
    let es = Es::start().await;
    cache_fixture(&es).await;
    let no_write_index = Err(
        "no write index is defined for alias [test_alias]. The write index may be explicitly \
         disabled using is_write_index=false or the alias points to multiple indices without \
         one being designated as a write index"
            .to_string(),
    );
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index2".to_string())
    );
    Es::ok(
        es.put(
            "/test_index2/_alias/test_alias",
            Some(json!({"is_write_index": false})),
        )
        .await,
    );
    let a = Es::ok(es.get("/_alias/test_alias").await);
    assert_eq!(
        a.body["test_index2"]["aliases"]["test_alias"],
        json!({"is_write_index": false})
    );
    assert_eq!(write_index(&es, "test_alias").await, no_write_index);
    Es::ok(
        es.put(
            "/test_index1/_alias/test_alias",
            Some(json!({"is_write_index": true})),
        )
        .await,
    );
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index1".to_string())
    );
    Es::ok(es.delete("/test_index2/_alias/test_alias").await);
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index1".to_string())
    );
    Es::ok(es.put("/test_index2/_alias/test_alias", None).await);
    let a = Es::ok(es.get("/_alias/test_alias").await);
    assert_eq!(
        a.body,
        json!({
            "test_index1": {"aliases": {"test_alias": {"is_write_index": true}}},
            "test_index2": {"aliases": {"test_alias": {}}},
        })
    );
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index1".to_string())
    );
    Es::ok(es.put("/test_index3", None).await);
    Es::ok(
        es.post(
            "/_aliases",
            json!({"actions": [{"add": {"index": "test_index3", "alias": "test_alias"}}]}),
        )
        .await,
    );
    let a = Es::ok(es.get("/_alias/test_alias").await);
    assert_eq!(a.body.as_object().expect("object").len(), 3, "{}", a.text);
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index1".to_string())
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn two_write_indices_are_refused() {
    let es = Es::start().await;
    Es::ok(es.put("/test_index1", None).await);
    Es::ok(es.put("/test_index2", None).await);
    let a = es
        .post(
            "/_aliases",
            json!({"actions": [
                {"add": {"index": "test_index1", "alias": "test_alias", "is_write_index": true}},
                {"add": {"index": "test_index2", "alias": "test_alias", "is_write_index": true}},
            ]}),
        )
        .await;
    a.assert_error(
        500,
        "illegal_state_exception",
        Some("alias [test_alias] has more than one write index [test_index1,test_index2]"),
    );
    let a = es.get("/_alias/test_alias").await;
    assert_eq!(a.status, StatusCode::NOT_FOUND, "{}", a.text);
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn deleting_a_member_index_removes_it_from_the_alias() {
    let es = Es::start().await;
    cache_fixture(&es).await;
    Es::ok(es.delete("/test_index2").await);
    let a = Es::ok(es.get("/_alias/test_alias").await);
    assert_eq!(
        a.body,
        json!({"test_index1": {"aliases": {"test_alias": {}}}})
    );
    // The only member, unset, is the write index (Ruling 9).
    assert_eq!(
        write_index(&es, "test_alias").await,
        Ok("test_index1".to_string())
    );
    Es::ok(es.delete("/test_index1").await);
    assert_eq!(
        es.head("/_alias/test_alias").await.status,
        StatusCode::NOT_FOUND
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn put_mapping_through_an_alias_updates_every_member() {
    let es = Es::start().await;
    cache_fixture(&es).await;
    let a = Es::ok(
        es.put("/test_alias/_mapping", Some(llm_cache_mapping()))
            .await,
    );
    assert_eq!(a.body, json!({"acknowledged": true}));
    for index in ["test_index1", "test_index2"] {
        let a = Es::ok(es.get(&format!("/{index}/_mapping")).await);
        assert_eq!(a.body[index]["mappings"], llm_cache_mapping(), "{index}");
    }
    // A mapping that conflicts in one member only changes neither.
    Es::ok(
        es.put(
            "/test_index2/_mapping",
            Some(json!({"properties": {"tag": {"type": "long"}}})),
        )
        .await,
    );
    let a = es
        .put(
            "/test_alias/_mapping",
            Some(json!({"properties": {"tag": {"type": "keyword"}, "extra": {"type": "keyword"}}})),
        )
        .await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some("mapper [tag] cannot be changed from type [long] to [keyword]"),
    );
    for index in ["test_index1", "test_index2"] {
        let a = Es::ok(es.get(&format!("/{index}/_mapping")).await);
        assert!(
            a.body[index]["mappings"]["properties"]
                .get("extra")
                .is_none(),
            "{index}: {}",
            a.text
        );
    }
    // write_index_only updates the write index alone.
    Es::ok(
        es.put(
            "/test_alias/_mapping?write_index_only=true",
            Some(json!({"properties": {"only": {"type": "keyword"}}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/test_index1,test_index2/_mapping").await);
    assert!(
        a.body["test_index1"]["mappings"]["properties"]
            .get("only")
            .is_none()
    );
    assert_eq!(
        a.body["test_index2"]["mappings"]["properties"]["only"],
        json!({"type": "keyword"})
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn put_mapping_adds_a_field() {
    let es = Es::start().await;
    Es::ok(es.put("/i", None).await);
    Es::ok(
        es.put(
            "/i/_mapping",
            Some(json!({"properties": {"tag": {"type": "keyword"}}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/i/_mapping").await);
    assert_eq!(
        a.body["i"]["mappings"]["properties"]["tag"],
        json!({"type": "keyword"})
    );
    let a = es
        .put(
            "/i/_mapping",
            Some(json!({"properties": {"tag": {"type": "long"}}})),
        )
        .await;
    a.assert_error(
        400,
        "illegal_argument_exception",
        Some("mapper [tag] cannot be changed from type [keyword] to [long]"),
    );
    // POST works too, and redeclaring is a no-op.
    Es::ok(
        es.post(
            "/i/_mapping",
            json!({"properties": {"tag": {"type": "keyword"}}}),
        )
        .await,
    );
    let a = es.put("/i/_mapping", None).await;
    a.assert_error(400, "parse_exception", Some("request body is required"));
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn a_default_pipeline_fails_every_write_item() {
    // The index side; the write side is Task 5's.
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/i",
            Some(json!({"settings": {"index": {"default_pipeline": "not-existing-pipeline"}}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/i").await);
    assert_eq!(
        a.body["i"]["settings"]["index"]["default_pipeline"],
        "not-existing-pipeline"
    );
    es.server.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn create_with_aliases_adds_them() {
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/i",
            Some(json!({"aliases": {"a1": {}, "a2": {"is_write_index": true}}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/i/_alias").await);
    assert_eq!(
        a.body,
        json!({"i": {"aliases": {"a1": {}, "a2": {"is_write_index": true}}}})
    );
    let a = es.put("/j", Some(json!({"aliases": {"i": {}}}))).await;
    a.assert_error(400, "invalid_alias_name_exception", None);
    assert_eq!(es.head("/j").await.status, StatusCode::NOT_FOUND);
    let a = es
        .put("/j", Some(json!({"aliases": {"a3": {"routing": "1"}}})))
        .await;
    a.assert_error(400, "illegal_argument_exception", None);
    assert_eq!(es.head("/j").await.status, StatusCode::NOT_FOUND);
    es.server.shutdown().await.expect("shutdown");
}

// ----- _cat/indices (MA1 Task 24) -----

/// Indices `a1` (2 documents), `a2` (empty) and `b1` (1 document), and an
/// alias `al` over `b1`.
async fn cat_fixture(es: &Es) {
    for index in ["a1", "a2", "b1"] {
        Es::ok(es.put(&format!("/{index}"), None).await);
    }
    for (index, id) in [("a1", "1"), ("a1", "2"), ("b1", "1")] {
        let a = es
            .put(
                &format!("/{index}/_doc/{id}?refresh=true"),
                Some(json!({"n": 1})),
            )
            .await;
        assert_eq!(a.status, StatusCode::CREATED, "{}", a.text);
    }
    Es::ok(es.put("/b1/_alias/al", None).await);
}

/// The `index` values of a JSON answer, in order.
fn cat_index_names(body: &Value) -> Vec<&str> {
    body.as_array()
        .expect("an array")
        .iter()
        .map(|row| row["index"].as_str().expect("index"))
        .collect()
}

#[tokio::test]
async fn cat_indices_json_lists_indices() {
    let es = Es::start().await;
    // No index yet: an empty array.
    assert_eq!(
        Es::ok(es.get("/_cat/indices?format=json").await).body,
        json!([])
    );
    cat_fixture(&es).await;
    let a = Es::ok(es.get("/_cat/indices?format=json").await);
    assert!(a.header("content-type").starts_with("application/json"));
    assert_eq!(a.header("x-elastic-product"), "Elasticsearch");
    assert_eq!(cat_index_names(&a.body), ["a1", "a2", "b1"]);
    let a1 = &a.body[0];
    let columns: Vec<&str> = a1
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        columns,
        [
            "health",
            "status",
            "index",
            "uuid",
            "pri",
            "rep",
            "docs.count",
            "docs.deleted",
            "store.size",
            "pri.store.size",
            "dataset.size"
        ]
    );
    // Every value is a string, as in ES.
    for value in a1.as_object().expect("object").values() {
        assert!(value.is_string(), "{value}");
    }
    assert_eq!(a1["health"], "green");
    assert_eq!(a1["status"], "open");
    assert_eq!(a1["pri"], "1");
    assert_eq!(a1["rep"], "0");
    assert_eq!(a1["docs.count"], "2");
    assert_eq!(a1["docs.deleted"], "0");
    assert_eq!(a.body[1]["docs.count"], "0");
    assert_eq!(a.body[2]["docs.count"], "1");
    let info = es
        .server
        .collections()
        .get_collection(NS, "a1")
        .await
        .expect("a1");
    assert_eq!(a1["uuid"], index_uuid(info.id));
    // An Accept of JSON without `format` answers JSON too.
    let a = es
        .send(
            reqwest::Method::GET,
            "/_cat/indices",
            None,
            &[("accept", "application/json")],
        )
        .await;
    assert_eq!(cat_index_names(&a.body), ["a1", "a2", "b1"]);
    // `health=` filters (every index is green), unknown parameters are 400.
    assert_eq!(
        Es::ok(es.get("/_cat/indices?format=json&health=yellow").await).body,
        json!([])
    );
    assert_eq!(
        cat_index_names(&Es::ok(es.get("/_cat/indices?format=json&health=green").await).body),
        ["a1", "a2", "b1"]
    );
    es.get("/_cat/indices?format=json&nope=1")
        .await
        .assert_error(
            400,
            "illegal_argument_exception",
            Some("request [/_cat/indices] contains unrecognized parameter: [nope]"),
        );
    es.get("/_cat/indices?format=yaml")
        .await
        .assert_error(400, "illegal_argument_exception", None);
    // `expand_wildcards` is accepted.
    Es::ok(
        es.get("/_cat/indices?format=json&expand_wildcards=all")
            .await,
    );
}

#[tokio::test]
async fn cat_indices_text_with_header() {
    let es = Es::start().await;
    cat_fixture(&es).await;
    let a = Es::ok(es.get("/_cat/indices?h=index,docs.count").await);
    assert!(
        a.header("content-type").starts_with("text/plain"),
        "{:?}",
        a.headers
    );
    assert_eq!(a.text, "a1 2\na2 0\nb1 1\n");
    let a = Es::ok(es.get("/_cat/indices?v&h=health,index,docs.count").await);
    assert_eq!(
        a.text,
        "health index docs.count\ngreen  a1             2\ngreen  a2             0\ngreen  b1             1\n"
    );
    // Without `h`, every column, in ES's order.
    let a = Es::ok(es.get("/_cat/indices?v=true").await);
    let header: Vec<&str> = a
        .text
        .lines()
        .next()
        .expect("header")
        .split_whitespace()
        .collect();
    assert_eq!(
        header,
        [
            "health",
            "status",
            "index",
            "uuid",
            "pri",
            "rep",
            "docs.count",
            "docs.deleted",
            "store.size",
            "pri.store.size",
            "dataset.size"
        ]
    );
    assert_eq!(a.text.lines().count(), 4);
}

#[tokio::test]
async fn cat_indices_h_and_s() {
    let es = Es::start().await;
    cat_fixture(&es).await;
    // Aliases keep their spelling; patterns expand.
    let a = Es::ok(es.get("/_cat/indices?format=json&h=i,dc&s=dc:desc").await);
    assert_eq!(
        a.body,
        json!([
            {"i": "a1", "dc": "2"},
            {"i": "b1", "dc": "1"},
            {"i": "a2", "dc": "0"},
        ])
    );
    let a = Es::ok(
        es.get("/_cat/indices?format=json&h=index,docs.*&s=index:desc")
            .await,
    );
    assert_eq!(
        a.body,
        json!([
            {"index": "b1", "docs.count": "1", "docs.deleted": "0"},
            {"index": "a2", "docs.count": "0", "docs.deleted": "0"},
            {"index": "a1", "docs.count": "2", "docs.deleted": "0"},
        ])
    );
    // Numbers sort by value, several keys in order.
    let a = Es::ok(
        es.get("/_cat/indices?format=json&h=index&s=docs.count,index")
            .await,
    );
    assert_eq!(cat_index_names(&a.body), ["a2", "b1", "a1"]);
    es.get("/_cat/indices?format=json&s=nope")
        .await
        .assert_error(
            400,
            "illegal_argument_exception",
            Some("Unable to sort by unknown sort key `nope`"),
        );
}

#[tokio::test]
async fn cat_indices_one_index_and_wildcard() {
    let es = Es::start().await;
    cat_fixture(&es).await;
    let names = |path: String| {
        let es = &es;
        async move {
            cat_index_names(&Es::ok(es.get(&path).await).body)
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(names("/_cat/indices/a1?format=json".into()).await, ["a1"]);
    assert_eq!(
        names("/_cat/indices/*1?format=json".into()).await,
        ["a1", "b1"]
    );
    // A pattern matches aliases too: `a*` reaches `b1` through `al`.
    assert_eq!(
        names("/_cat/indices/a*?format=json".into()).await,
        ["a1", "a2", "b1"]
    );
    assert_eq!(
        names("/_cat/indices/a2,b1?format=json".into()).await,
        ["a2", "b1"]
    );
    // An alias lists its member; `_all` and `*` list everything.
    assert_eq!(names("/_cat/indices/al?format=json".into()).await, ["b1"]);
    assert_eq!(
        names("/_cat/indices/_all?format=json".into()).await,
        ["a1", "a2", "b1"]
    );
    assert_eq!(
        names("/_cat/indices/*?format=json".into()).await,
        ["a1", "a2", "b1"]
    );
    // A wildcard matching nothing is empty; a missing name is 404.
    assert!(
        names("/_cat/indices/zz*?format=json".into())
            .await
            .is_empty()
    );
    es.get("/_cat/indices/missing?format=json")
        .await
        .assert_error(404, "index_not_found_exception", None);
}

#[tokio::test]
async fn cat_indices_bytes_units() {
    let es = Es::start().await;
    cat_fixture(&es).await;
    let cell = |unit: &'static str| {
        let es = &es;
        async move {
            let path =
                format!("/_cat/indices/a1?format=json&h=store.size,pri.store.size&bytes={unit}");
            let a = Es::ok(es.get(&path).await);
            assert_eq!(a.body[0]["store.size"], a.body[0]["pri.store.size"]);
            a.body[0]["store.size"].as_str().expect("size").to_string()
        }
    };
    // The size moves while the link and compaction settle, so each unit is
    // compared with a byte count read just before and just after it.
    for unit in ["kb", "mb", "gb", "human"] {
        let mut checked = false;
        for _ in 0..100 {
            let before: u64 = cell("b").await.parse().expect("bytes");
            let value = if unit == "human" {
                let a = Es::ok(es.get("/_cat/indices/a1?format=json&h=ss").await);
                a.body[0]["ss"].as_str().expect("ss").to_string()
            } else {
                cell(unit).await
            };
            let after: u64 = cell("b").await.parse().expect("bytes");
            if before != after {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
            let expected = match unit {
                "kb" => (before / 1024).to_string(),
                "mb" => (before >> 20).to_string(),
                "gb" => (before >> 30).to_string(),
                _ => loams_es::cat::size_text(before, None),
            };
            assert_eq!(value, expected, "{unit}");
            checked = true;
            break;
        }
        assert!(checked, "the size of a1 never held still ({unit})");
    }
    // `bytes=b` is the collection's `size_bytes`.
    let size_bytes = || async {
        es.server
            .collections()
            .get_collection(NS, "a1")
            .await
            .expect("a1")
            .size_bytes
    };
    let mut checked = false;
    for _ in 0..100 {
        let before = size_bytes().await;
        let b = cell("b").await;
        if size_bytes().await == before {
            assert_eq!(b, before.to_string());
            checked = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(checked, "the size of a1 never held still (b)");
    es.get("/_cat/indices?format=json&bytes=zb")
        .await
        .assert_error(400, "illegal_argument_exception", None);
}

// ----- _settings (#948) -----

#[tokio::test]
async fn settings_get_and_put_round_trip() {
    let es = Es::start().await;
    Es::ok(
        es.put(
            "/s1",
            Some(json!({"settings": {"number_of_shards": 2, "refresh_interval": "1s"}})),
        )
        .await,
    );
    Es::ok(es.put("/s2", None).await);
    let a = Es::ok(es.get("/s1/_settings").await);
    let index = &a.body["s1"]["settings"]["index"];
    assert_eq!(index["refresh_interval"], "1s", "{}", a.text);
    assert_eq!(index["number_of_shards"], "2");
    assert_eq!(index["provided_name"], "s1");
    assert!(index["uuid"].is_string() && index["creation_date"].is_string());
    // The NVIDIA RAG Blueprint's bulk-load toggle.
    let a = Es::ok(
        es.put(
            "/s1/_settings",
            Some(json!({"index": {"refresh_interval": "-1"}})),
        )
        .await,
    );
    assert_eq!(a.body, json!({"acknowledged": true}));
    let a = Es::ok(es.get("/s1/_settings").await);
    assert_eq!(a.body["s1"]["settings"]["index"]["refresh_interval"], "-1");
    // Flat and bare keys; `null` resets.
    Es::ok(
        es.put(
            "/s1/_settings",
            Some(json!({"index.number_of_replicas": 0, "max_result_window": 500})),
        )
        .await,
    );
    let a = Es::ok(es.get("/s1/_settings?flat_settings=true").await);
    let flat = &a.body["s1"]["settings"];
    assert_eq!(flat["index.number_of_replicas"], "0", "{}", a.text);
    assert_eq!(flat["index.max_result_window"], "500");
    Es::ok(
        es.put(
            "/s1/_settings",
            Some(json!({"index": {"refresh_interval": null}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/s1/_settings").await);
    assert!(
        a.body["s1"]["settings"]["index"]["refresh_interval"].is_null(),
        "{}",
        a.text
    );
    // `preserve_existing` keeps a setting already set.
    Es::ok(
        es.put(
            "/s1/_settings?preserve_existing=true",
            Some(json!({"index": {"max_result_window": 9, "refresh_interval": "5s"}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/s1/_settings").await);
    assert_eq!(
        a.body["s1"]["settings"]["index"]["max_result_window"],
        "500"
    );
    assert_eq!(a.body["s1"]["settings"]["index"]["refresh_interval"], "5s");
    // Several indices and one setting by name.
    let a = Es::ok(es.get("/_settings").await);
    let mut names: Vec<&String> = a.body.as_object().expect("object").keys().collect();
    names.sort();
    assert_eq!(names, ["s1", "s2"]);
    let a = Es::ok(es.get("/s*/_settings/index.refresh_interval").await);
    assert_eq!(
        a.body,
        json!({"s1": {"settings": {"index": {"refresh_interval": "5s"}}}})
    );
    let a = Es::ok(es.get("/s1/_settings/index.number_*").await);
    assert_eq!(
        a.body["s1"]["settings"]["index"],
        json!({"number_of_shards": "2", "number_of_replicas": "0"})
    );
    // `PUT /_settings` changes every index.
    Es::ok(
        es.put(
            "/_settings",
            Some(json!({"index": {"refresh_interval": "2s"}})),
        )
        .await,
    );
    let a = Es::ok(es.get("/s2/_settings").await);
    assert_eq!(a.body["s2"]["settings"]["index"]["refresh_interval"], "2s");
}

#[tokio::test]
async fn settings_put_refuses_static_and_unknown_settings() {
    let es = Es::start().await;
    Es::ok(es.put("/s1", None).await);
    let uuid = {
        let info = es
            .server
            .collections()
            .get_collection(NS, "s1")
            .await
            .expect("s1");
        index_uuid(info.id)
    };
    es.put(
        "/s1/_settings",
        Some(json!({"index": {"number_of_shards": 3}})),
    )
    .await
    .assert_error(
        400,
        "illegal_argument_exception",
        Some(&format!(
            "Can't update non dynamic settings [[index.number_of_shards]] for open indices [[s1/{uuid}]]"
        )),
    );
    es.put("/s1/_settings", Some(json!({"index": {"nope": 1}})))
        .await
        .assert_error(400, "illegal_argument_exception", None);
    es.put("/s1/_settings", Some(json!({}))).await.assert_error(
        400,
        "action_request_validation_exception",
        None,
    );
    es.put(
        "/missing/_settings",
        Some(json!({"index": {"refresh_interval": "1s"}})),
    )
    .await
    .assert_error(404, "index_not_found_exception", None);
    es.get("/missing/_settings")
        .await
        .assert_error(404, "index_not_found_exception", None);
    // A refusal changes nothing.
    let a = Es::ok(es.get("/s1/_settings").await);
    assert!(a.body["s1"]["settings"]["index"]["nope"].is_null());
}
