# Loams Ruby SDK (preview)

> **Pre-release (preview).** This SDK is published as a preview of Loams. Its API
> can change between previews without notice: pin the exact version, and expect
> breaking changes until 1.0. Report problems at
> <https://github.com/ostrium-labs/loams/issues>.

## Install

```sh
gem install loams
```

or, in a `Gemfile`, pin the version exactly:

```ruby
gem "loams", "0.1.4"
```

A prerelease tag such as `v0.1.0-preview.2` is spelled `0.1.0.pre.2` on RubyGems and needs `--pre`. The gem needs
Ruby 3.1 or newer and `google-protobuf` 4.x.

## Use

```ruby
require "loams"

client = Loams.client("https://acme.loams.dev")
```

## Build an app

The app-building modules are `namespaces`, `collections`, `documents`,
`search`, `vector` and `graph` (experimental), with the `Schema`, `Filter` and
`Search` builders. [`examples/notes/notes.rb`](examples/notes/notes.rb) is the
whole flow:

```ruby
require "loams"

# LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE.
client = Loams.from_env(session_consistency: true)
# or: Loams::Client.new("http://127.0.0.1:8080", api_key: "…", namespace: "app", timeout: 30)

client.namespaces.create("app")
client.collections.create("notes", schema: Loams::Schema.new
  .keyword("title").text("body").keyword("tags").i64("n").vector("embedding", 4))

notes = client.collection("notes")
notes.upsert([{ id: 1, source: { title: "Refunds", body: "refund policy", tags: ["policy"], n: 1 },
                vectors: { embedding: [1, 0, 0, 0] } }])

hits = notes.search(
  Loams::Search.on("notes")
    .text("refund", fields: ["body"])
    .vector("embedding", [1, 0, 0, 0])
    .fuse(Loams::Fusion.rrf(60))          # or .dbsf, .weighted(0.7, 0.3)
    .filter(Loams::Filter.eq("tags", "policy") & Loams::Filter.range("n", gte: 1))
    .highlight("body")
    .select(include: ["title"])
    .limit(10),
).hits
hits.each { |h| puts Loams.to_h(h.source)["title"] }
```

- **Answers are messages.** `Loams.to_h(message)` turns one (or a `Struct`
  such as `hit.source`) into plain Ruby; `Loams::Values.from_document_id(hit.pk)`
  gives a key back as an Integer or a String.
- **Writes.** `documents.upsert`, `delete`, `patch` and `write` (raw ops) answer a
  consistency `token`. `documents.write_batch(collection, enumerable, chunk_rows:,
  chunk_bytes:)` sends chunks one at a time, each with its own idempotency key,
  and answers a `Loams::BatchResult` (the merged `token`, `chunks`, `rows`); a
  failed chunk raises `Loams::BatchError` with `chunks_applied` and the `token`
  of the chunks before it.
- **Reads.** `documents.get`, `count`, `scroll` / `scroll_all`, `search.query`,
  `search.query_all` (follows `search_after`; the sort must end with `Sort.pk`),
  `search.text(collection, text, fields:)`,
  `search.hybrid(collection, text, vector, field:, fields:, fusion:)` and
  `vector.search(collection, vector, field:)`. Each takes `consistency:`
  (`:strong`, `:eventual` or a token). With `session_consistency: true` every
  token an answer carries is merged into the session and attached to later reads.
- **Graph** (experimental): `graph.create`, `query` (GQL, with Ruby `parameters:`),
  `query_batch`, `explain`, `list_all`, `delete`; `Loams::App::Graph.records(res)`
  gives rows as Hashes.
- **The default namespace** fills every request that names none; with neither,
  the call raises `Loams::InvalidArgumentError` before any network call.
  **Timeouts** default to 30 s per unary call (`timeout:` on the client or the call,
  `nil` for none); a spent budget raises `Loams::DeadlineExceededError` with reason
  `client_deadline_exceeded` and is never retried.
- **SQL.** `sql.query(sql, params: [...], max_rows:, consistency:)` answers
  `columns` and `rows`; `Loams::App::Sql.records(res)` gives rows as Hashes.
  `query_arrow` answers the Arrow IPC bytes. SQL reads see the session's
  collection writes.
- **Streams.** `streams.create`, `get`, `list` / `list_all`, `produce(stream,
  records, partition:, producer_id:, producer_seq:)` (a record is a String, a
  JSON value, or `{key:, value:, headers:}` with an optional key; a repeated
  producer sequence is deduplicated), `fetch` (`Loams::App::Streams.records(res)`
  decodes it), `produce_events` / `fetch_events` (CloudEvents as Hashes), and
  `subscribe(stream, partition:, from_offset:)`, a `Loams::Subscription` to
  enumerate, `cancel` and resume from `offset`.
- Durable execution has no module: use the Resonate SDK with
  `LOAMS_ENDPOINT/durable/` as its URL.

Every module's raw descriptor-derived calls stay reachable as
`client.module("collections")` and friends.

## Develop

```sh
bundle install
bundle exec rake test   # the conformance suite and the app tests
LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 bundle exec rake test  # plus the app scenario against `loams dev`
bundle exec rake descriptors  # regenerate gen/loams-descriptor.binpb (needs buf)
gem build loams.gemspec # what sdk-release.yml publishes
```

Licensed under Apache-2.0 (see `LICENSE`).
