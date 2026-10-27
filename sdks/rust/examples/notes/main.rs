//! The notes app (SDKP Part D): the app scenario of
//! `sdks/fixtures/app-scenario.md` as a program, through the public API.
//!
//! ```sh
//! LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app cargo run --example notes
//! ```
//!
//! `cargo test` builds it, so it cannot drift from the API; running it needs a
//! `loams dev`.

use futures::StreamExt;
use loams::builders::search::{HighlightOptions, SelectOptions};
use loams::builders::{Filter, Fusion, Schema, Search, Sort, TextRetriever, VectorOptions};
use loams::collection_v1::{CreateCollectionRequest, CreateNamespaceRequest};
use loams::{
    BatchOptions, CallOptionsOverrides, Document, ErrorKind, Loams, LoamsError, stream_v1,
};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), LoamsError> {
    // LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE; reads
    // see this process's own writes.
    let loams = Loams::from_env()?.session_consistency(true);
    let none = CallOptionsOverrides::new;

    let created = loams
        .namespaces()
        .create(
            CreateNamespaceRequest {
                namespace: loams.namespace().unwrap_or("app").to_owned(),
                ..Default::default()
            },
            none(),
        )
        .await;
    ignore_existing(created.map(|_| ()))?;
    let schema = Schema::new()
        .text("title")
        .text("body")
        .keyword("tags")
        .i64("n")
        .vector("embedding", 4);
    let created = loams
        .collections()
        .create(
            CreateCollectionRequest {
                name: "notes".to_owned(),
                schema: buffa::MessageField::some(schema.to_struct()?),
                ..Default::default()
            },
            none(),
        )
        .await;
    ignore_existing(created.map(|_| ()))?;

    let notes = loams.collection("notes", None);
    notes
        .upsert(
            &[
                Document::new(1u64)
                    .source(json!({"title": "Refund policy", "body": "refund policy for every order", "tags": "policy", "n": 1}))
                    .vector("embedding", vec![1.0, 0.0, 0.0, 0.0]),
                Document::new(2u64)
                    .source(json!({"title": "Refund window", "body": "the refund window is thirty days", "tags": "policy", "n": 2}))
                    .vector("embedding", vec![0.0, 0.0, 1.0, 0.0]),
            ],
            none(),
        )
        .await?;

    // Hybrid: text and vector, fused with RRF, filtered, highlighted, projected.
    let hits = notes
        .search(
            notes
                .query()
                .text(TextRetriever::fields("refund", ["body"]))
                .vector("embedding", &[1.0, 0.0, 0.0, 0.0], VectorOptions::k(10))
                .fuse(Fusion::rrf(Some(60)))
                .filter(Filter::eq("tags", "policy"))
                .highlight("body", HighlightOptions::default())
                .select(SelectOptions::include(["title"]))
                .limit(3),
            none(),
        )
        .await?;
    println!("{} hits", hits.hits.len());

    // Every policy note, page by page (the sort ends in the primary key).
    let all = loams::handle::query_all(
        &loams,
        Search::on("notes")
            .text(TextRetriever::fields("refund", ["body"]))
            .sort([Sort::score(None), Sort::pk(None)])
            .limit(2),
        none(),
    );
    futures::pin_mut!(all);
    while let Some(hit) = all.next().await {
        println!("page hit {:?}", hit?.pk);
    }

    let policy = notes
        .count(Some(&Filter::eq("tags", "policy")), none())
        .await?;
    println!("policy notes: {:?}", policy.count);

    // SQL over the same collection, with a `$1` parameter, as records.
    let per_tag = loams
        .sql()
        .records(
            "SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags",
            vec![json!(1)],
            none(),
        )
        .await?;
    for row in &per_tag {
        println!("{} {}", row["tags"], row["c"]);
    }

    // A stream of note events: produce with a producer key, then read it back.
    loams
        .streams()
        .create(
            stream_v1::CreateStreamRequest {
                name: "note-events".into(),
                partitions: 1,
                ..Default::default()
            },
            none(),
        )
        .await?;
    loams
        .streams()
        .produce(
            stream_v1::ProduceRequest {
                stream: "note-events".into(),
                records: vec![stream_v1::Record {
                    value: Some(br#"{"note":1,"op":"created"}"#.to_vec()),
                    ..Default::default()
                }],
                producer_id: Some("notes-example".into()),
                producer_seq: Some(1),
                ..Default::default()
            },
            none(),
        )
        .await?;
    let mut events = loams.subscribe(
        stream_v1::FetchRequest {
            stream: "note-events".into(),
            ..Default::default()
        },
        none(),
    );
    if let Some(event) = events.next().await {
        println!("event at {}", event?.offset);
    }

    // A bulk load: chunked, one idempotency key per chunk, the tokens merged.
    let loaded = notes
        .write_batch(
            (0..100u64).map(|i| {
                Document::new(1000 + i)
                    .source(json!({"title": format!("note {i}"), "tags": "bulk", "n": i}))
            }),
            BatchOptions {
                chunk_rows: 25,
                ..BatchOptions::default()
            },
            none(),
        )
        .await
        .map_err(LoamsError::from)?;
    println!("wrote {} notes in {} chunks", loaded.rows, loaded.chunks);
    Ok(())
}

/// A create that finds the thing already there is done, not failed.
fn ignore_existing(result: Result<(), LoamsError>) -> Result<(), LoamsError> {
    match result {
        Err(error) if error.kind == ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}
