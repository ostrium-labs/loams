"""The notes app (SDKP Part D): the app scenario of `sdks/fixtures/app-scenario.md`
as a program, through the public `loams` API.

    LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app python sdks/python/examples/notes/main.py

It is type-checked in CI (`mypy --strict examples`), so it cannot drift from the
API; running it needs a `loams dev`.
"""

from __future__ import annotations

import json

from google.protobuf import json_format

from loams import AlreadyExistsError, Document, Filter, Loams, Schema, Search, Sort, rrf
from loams.collection.v1.collection_pb2 import CreateCollectionRequest, CreateNamespaceRequest
from loams.stream.v1.stream_pb2 import CreateStreamRequest, ProduceRequest, Record

NOTES = [
    Document(1, {"title": "Refund policy", "body": "refund policy for every order", "tags": "policy", "n": 1},
             {"embedding": [1, 0, 0, 0]}),
    Document(2, {"title": "Shipping times", "body": "shipping takes three days", "tags": "shipping", "n": 2},
             {"embedding": [0.9, 0.1, 0, 0]}),
    Document(3, {"title": "Refund window", "body": "the refund window is thirty days", "tags": "policy", "n": 3},
             {"embedding": [0, 0, 1, 0]}),
    Document(4, {"title": "Returns", "body": "returns need a receipt before a refund", "tags": "policy", "n": 4},
             {"embedding": [0.5, 0.5, 0, 0]}),
]


def main() -> None:
    # LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE; reads
    # see this process's own writes.
    with Loams.from_env(session_consistency=True) as loams:
        try:
            loams.namespaces.create(CreateNamespaceRequest(namespace=loams.namespace or "app"))
        except AlreadyExistsError:
            pass
        schema = Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4)
        try:
            loams.collections.create(
                json_format.ParseDict({"name": "notes", "schema": schema.to_json()}, CreateCollectionRequest())
            )
        except AlreadyExistsError:
            pass

        notes = loams.collection("notes")
        notes.upsert(NOTES)

        # Hybrid: text and vector, fused with RRF, filtered, highlighted, projected.
        hits = notes.search(
            notes.query()
            .text("refund", fields=["body"])
            .vector("embedding", [1, 0, 0, 0], k=10)
            .fuse(rrf(60))
            .filter(Filter.eq("tags", "policy"))
            .highlight("body")
            .select(include=["title"])
            .limit(3)
        )
        for hit in hits.hits:
            print(hit.pk, hit.score, hit.source)

        # Every policy note, page by page (the sort ends in the primary key).
        paged = (
            Search("notes")
            .text("refund", fields=["body"])
            .filter(Filter.eq("tags", "policy"))
            .sort(Sort.score(), Sort.pk())
            .limit(2)
        )
        for hit in loams.search.query_all(paged):
            print("page hit", hit.pk)

        print("policy notes:", notes.count(where=Filter.eq("tags", "policy")).count)

        # SQL over the same collection, with a `$1` parameter, as records.
        per_tag = loams.sql.records(
            "SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", [1]
        )
        for row in per_tag:
            print(row["tags"], row["c"])

        # A stream of note events: produce with a producer key, then read it back.
        loams.streams.create(CreateStreamRequest(name="note-events", partitions=1))
        loams.streams.produce(
            ProduceRequest(
                stream="note-events",
                records=[Record(value=json.dumps({"note": 1, "op": "created"}).encode())],
                producer_id="notes-example",
                producer_seq=1,
            )
        )
        for event in loams.streams.subscribe("note-events"):
            print("event at", event.offset)
            break

        # A bulk load: chunked, one idempotency key per chunk, the tokens merged.
        loaded = notes.write_batch(
            (Document(1000 + i, {"title": f"note {i}", "tags": "bulk", "n": i}) for i in range(100)),
            chunk_rows=25,
        )
        print(f"wrote {loaded.rows} notes in {loaded.chunks} chunks; token {loaded.token}")


if __name__ == "__main__":
    main()
