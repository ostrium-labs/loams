# Loams Objective-C SDK (preview)

> **Pre-release (preview).** This SDK is published as a preview of Loams. Its API
> can change between previews without notice: pin the exact version, and expect
> breaking changes until 1.0. Report problems at
> <https://github.com/ostrium-labs/loams/issues>.

## Install

The CocoaPods package is not published yet. Use the released monorepo source:
check out the `v0.1.2` tag, then add the local Swift package at `sdks/objc` to
your Xcode project and select the `LoamsObjC` product. Its module can sit next
to the Swift SDK's `Loams` module. It supports iOS 15 and macOS 12 or newer.

## Build an app

The app-building modules are `namespaces`, `collections`, `documents`,
`search`, `vector` and `graph` (experimental) on `LoamsClient`, with the
`LoamsSchema`, `LoamsFilter` and `LoamsSearch` builders and
`[client collection:@"notes" namespace:nil]`, a handle that fills the collection
in. Every Swift `throws` call is an Objective-C method with a trailing `error:`.
[`Examples/Notes/main.m`](Examples/Notes/main.m) is the flow from Objective-C
(see its [setup guide](Examples/Notes/README.md));
from Swift:

```swift
let config = LoamsClientConfiguration.fromEnvironment()!  // LOAMS_ENDPOINT, LOAMS_API_KEY, LOAMS_NAMESPACE
config.sessionConsistency = true                          // reads see this client's writes
let client = LoamsClient(configuration: config)

_ = try client.namespaces.create("app")
_ = try client.collections.create("notes", schema: LoamsSchema()
    .keyword("title").text("body").keyword("tags").i64("n").vector("embedding", dim: 4))

let notes = client.collection("notes")
_ = try notes.upsert([LoamsDocument(id: 1, source: ["title": "Refunds", "body": "refund policy", "tags": ["policy"]],
                                    vectors: ["embedding": [1, 0, 0, 0]])])

let page = try notes.search(LoamsSearch.on("notes")
    .text("refund", fields: ["body"])
    .vector("embedding", values: [1, 0, 0, 0])
    .fuse(LoamsFusion.rrf(60))          // or .dbsf(), .weighted([0.7, 0.3])
    .filter(LoamsFilter.eq("tags", value: "policy"))
    .highlight("body")
    .select(include: ["title"])
    .limit(10))
```

- **Answers are proto3 JSON dictionaries** (`lowerCamelCase` keys, 64-bit integers
  as strings); `LoamsValues.fromDocumentId(hit["pk"])` gives a key back.
- **Writes.** `documents.upsert`, `delete`, `patch` and `write` (raw ops) answer a
  consistency `token`. `documents.writeBatch(_:documents:chunkRows:chunkBytes:)`
  sends chunks one at a time, each with its own idempotency key, and answers a
  `LoamsBatchResult` (`token`, `chunks`, `rows`); a failed chunk throws a
  `LoamsBatchError` with `chunksApplied` and the `token` of the chunks before it.
- **Reads.** `documents.get`, `count`, `scroll` / `scrollAll`, `search.query`,
  `search.queryAll` (follows `searchAfter`; the sort must end with `"pk"`),
  `search.text`, `search.hybrid(_:text:vector:field:fields:fusion:)` and
  `vector.search(_:vector:field:)`. `consistency:` is `strong`, `eventual` or a token.
- **Graph** (experimental): `graph.create`, `query` (GQL with `parameters`;
  `readOnly` retries as a read), `queryBatch`, `explain`, `listAll`, `delete`;
  `LoamsGraphModule.records(_:)` gives rows as dictionaries.
- **The default namespace** (`config.namespace`, or `LoamsCallOptions.namespace`)
  fills every request that names none; with neither the call fails with
  `invalid_argument` before any network call. **Timeouts**: `config.timeout` (30 s)
  is a unary call's budget, retries included, sent as `connect-timeout-ms`;
  `LoamsCallOptions.timeout` overrides it, and a spent budget fails with
  `deadline_exceeded` / `client_deadline_exceeded` and is never retried.
- **SQL.** `sql.query(_:params:maxRows:consistency:)` answers `columns` and
  `rows`; `LoamsSqlModule.records(_:)` gives rows as dictionaries. `queryArrow`
  answers the Arrow IPC bytes. SQL reads see the session's collection writes.
- **Streams.** `streams.create`, `get`, `list` / `listAll`,
  `produce(_:records:partition:producerId:producerSeq:)` (a record is `Data`, a
  string, a JSON value or `["key": …, "value": …, "headers": […]]`; a repeated
  producer sequence is deduplicated), `fetch` (`LoamsStreamsModule.records(_:)`
  decodes it into `LoamsStreamRecord`s), `produceEvents` / `fetchEvents`
  (CloudEvents as dictionaries), and `subscribe(_:partition:fromOffset:)`, a
  `LoamsSubscription`: `next()` pulls, `start(onRecord:)` calls a block per record
  and answers a `LoamsCancelToken`, and `offset` is where to resume.
- `LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 swift test --disable-swift-testing`
  also runs the app scenario against `loams dev`.

## Develop

```sh
swift build
swift test --disable-swift-testing
pod lib lint ../../Loams.podspec   # what sdk-release.yml checks (macOS)
```

Licensed under Apache-2.0 (see `LICENSE`).
