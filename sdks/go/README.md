![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# The Go SDK

Module path **`loams.dev/go`**. Design [§44](../../docs/design/44-unified-api-and-sdks.md)
§7 and §9 row 3, decisions **D604, D606, D612**, the language plan's Task 2, and
the clauses **R1–R10** of the [runtime contract](../../docs/sdk/runtime-contract.md).

```go
client := loams.New(loams.Options{
    Endpoint: os.Getenv("LOAMS_ENDPOINT"),
    Auth:     loams.APIKey(os.Getenv("LOAMS_API_KEY")),
})
info, err := client.Instance().GetInstance(ctx, &loams.GetInstanceRequest{})
```

Requires **Go 1.24 or newer** — the SDK uses range-over-func iterators
(`for item := range pages.Seq()`) and generics on free functions.

## Installing

```console
go get loams.dev/go@v0.1.2
```

Go 1.25 or newer: the module lives in `sdks/go` of the main repository, and the
`go-import` tag that says so carries a subdirectory, which only Go 1.25 reads
(see [Publishing](#publishing)). `v0.1.2` tracks Loams 0.1.2; in 0.x the API
may change between minor versions.

From a checkout of this repository, with a `replace`:

```go
require loams.dev/go v0.0.0
replace loams.dev/go => ../../path/to/sdks/go
```

## What is generated and what is hand-written

| Path | Provenance |
|---|---|
| `gen/loams/**` | **Generated.** `protoc-gen-go` and `protoc-gen-connect-go` over `proto/` (D604). Reproduce with `scripts/sdk/gen.sh go` (or `buf generate --template sdks/go/buf.gen.go.yaml` from the repository root); CI's `scripts/sdk/drift.sh go` fails on a diff. |
| `gen/facade/**` | **Hand-written for now.** The module catalogue, the binding table and the reason registry, transcribed from the `loams.options.v1` annotations. This is design §44 §7.3's Q604 fallback; the Go renderer (`crates/loams-facade-gen/src/go.rs`) does not exist yet. |
| `modules.go` | **Hand-written for now.** One Go method per generated facade call, three to five lines each. Deleted when the renderer lands. |
| everything else | **Hand-written runtime**, once: transport, credentials, retry, errors, tokens, pagination, streams. |

Nothing in the module surface holds an RPC path or a retry class: every method
resolves its binding by name from `gen/facade` and hands the generated stub's
method value to the shared invoker. That is why the hand-written fallback cannot
drift from the annotations while it exists.

## Context first

Every call takes a `context.Context` first, and every clause of the runtime
contract composes with it rather than against it:

```go
ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
defer cancel()

info, err := client.Instance().GetInstance(ctx, &loams.GetInstanceRequest{})
```

- A cancelled context aborts the attempt; no retry is spent on a call the caller
  has already given up on.
- A deadline covers **the whole call**, retries and backoff included. There is no
  `TimeoutMs` option: in Go a deadline is the context, not a per-call field.
- Backoff sleeps on `ctx`, so a shutdown does not wait out a 2 s backoff.
- A stream is driven by the caller's loop, so cancelling is a `ctx.Err()` check.

## Errors

```go
var notFound *loams.NotFoundError
if errors.As(err, &notFound) { … }

if loams.ReasonOf(err) == loams.ReasonFeatureNotInVariant { … }
```

`reason` is a stable `snake_case` string registered in
[`docs/api/reasons.md`](../../docs/api/reasons.md) and generated into
`loams.dev/go/gen/facade`, so it is a `Reason` constant rather than a string.
`FeatureNotInVariantError` and `TokenExpiredError` are subclasses of
`UnimplementedError` and `UnauthenticatedError` respectively, so `errors.As`
matches the coarse class too.

A failure from **below** the API — a socket, a cancelled context — carries no
`reason` at all, which is a different thing from a Loams service refusing.

## Feature detection

```go
if err := client.System().Guard(ctx, "live"); err != nil {
    var absent *loams.FeatureNotInVariantError
    if errors.As(err, &absent) {
        return runWithoutLiveSync()
    }
    return err
}
```

`Guard` costs no RPC once the catalogue is cached, and raises the **same error
type** a refused RPC produces, so one `errors.As` covers both.

## Server streams

```go
stream, err := client.Live().Watch(ctx, request, loams.WithStreamResume(
    loams.StreamResume[*loams.WatchRequest, *loams.Transition]{
        CursorOf: func(t *loams.Transition) string { return cursorOf(t) },
        Resume: func(c string, r *loams.WatchRequest) *loams.WatchRequest { … },
    }))
if err != nil {
    return err
}
defer stream.Close()

for stream.Receive() {
    apply(stream.Msg())
    if ctx.Err() != nil {
        break
    }
}
return stream.Err()
```

`for message := range stream.Range()` is the range-over-func form. `Receive`
returning false and `Err()` being nil means the stream **finished**; a non-nil
`Err` means it broke. A caller that ignores `Err` sees a silently truncated
stream, which for a watch looks exactly like a stream that works.

## Pagination

```go
items, err := loams.PaginateCall(client, "collections", "ListCollections",
    client.Collections().ListCollections, ctx, request)
if err != nil {
    return err
}
for collection := range items.Seq() {
    use(collection)
}
return items.Err()
```

No RPC is paged yet (`loams.collection.v1.ListCollections` arrives with API1
Task 2), so this is the iterator the generated binding will drive, tested against
a stub. The **same** `Seq`/`Err` split as `*Stream`, for the same reason: a
range-over-func cannot return an error, and a caller who skips `Err` sees a short
list that looks like the end of the list.

## Build an app

The app-building layer (SDKP) adds builders, a collection handle and the
runtime an app needs: a default namespace, deadlines, read-your-writes
consistency tokens, chunked bulk writes and stream subscriptions.

```go
client, err := loams.FromEnv(loams.Options{SessionConsistency: true}) // LOAMS_ENDPOINT, LOAMS_API_KEY, LOAMS_NAMESPACE
if err != nil {
    return err
}

schema, _ := loams.NewSchema().
    Text("title").Text("body").Keyword("tags").I64("n").
    Vector("embedding", 4).
    Struct() // CreateCollectionRequest.schema

notes := client.Collection("notes")
_, err = notes.Upsert(ctx, []loams.DocumentInput{{
    ID:      loams.UintID(1),
    Source:  loams.JSONObject{"title": "Refunds", "body": "refund policy", "tags": "policy", "n": 1},
    Vectors: map[string][]float32{"embedding": {1, 0, 0, 0}},
}})

hits, err := notes.Search(ctx, notes.Query().
    Text("refund", loams.TextOptions{Fields: []string{"body"}}).
    Vector("embedding", []float32{1, 0, 0, 0}, loams.VectorOptions{K: 10}).
    Fuse(loams.RRF(60)).
    Filter(loams.Filter.And(loams.Filter.Eq("tags", "policy"), loams.Filter.Range("n", loams.Bounds{Gte: 1}))).
    Highlight("body").
    Select(loams.SelectOptions{Include: []string{"title"}}).
    Limit(5))
```

- **Default namespace (R11).** `Options.Namespace` fills the `namespace` of
  every app request (`loams.collection.v1`, `graph.v1`, `sql.v1`,
  `stream.v1`) that leaves it empty; an explicit one wins, and neither is
  `*InvalidArgumentError` before any network call.
- **Deadlines (R12).** Unary calls default to 30 s (`Options.Timeout`,
  `NoTimeout`, or `WithTimeout` per call), retries included. Running out is a
  `*DeadlineExceededError` with reason `client_deadline_exceeded`, never
  retried. Streams have no default deadline.
- **Consistency tokens (R4).** `ParseConsistencyToken`, `Merge`, `String`,
  `Equal`, exact over `uint64`. With `SessionConsistency` every token a
  response carries merges into `client.Session()`, and every read with a
  `consistency` field is sent `at_least` the session
  (`WithoutSessionConsistency()` opts a call out).
- **Bulk writes (R13).** `WriteBatch` sends one chunk at a time (10 000
  operations or 4 MiB), each with its own idempotency key, returns the merged
  token, and stops at the first failed chunk with a `*BatchError`
  (`ChunksApplied`, `RowsApplied`, `Token`).
- **Subscriptions (R14).** `loams.Subscribe(ctx, fetch, offsetOf, from)` is a
  `*Subscription` with `Next`, `Record`, `Err`, `Offset` and `Close`.

**The app modules** are `client.Namespaces()`, `Collections()`, `Documents()`,
`Search()`, `Vector()` and `Graph()`, one method per facade call, with the
conveniences on `client.DocumentCalls()`, `client.SearchCalls()` and
`client.Collection(name)`. `Sql()` is `loams.sql`: `Query` answers `Columns`
and `Rows`, `Records(ctx, statement, []any{…})` answers one map per row, and
`loams.SqlRecords(answer)` reads an answer in hand; `QueryArrow` answers Arrow
IPC bytes (`ArrowIpc`), which apache/arrow-go's `ipc.NewReader` reads.
`Streams()` is `loams.streams`: `Create`, `Get`, `List` (and `ListAll`),
`Produce` (set `ProducerId` and `ProducerSeq` so a retry cannot append twice),
`Fetch`, `ProduceEvents` and `FetchEvents` (CloudEvents as a JSON batch), and
`Subscribe(ctx, request)` (R14), whose `Offset` is where to resume.
`examples/notes` is the
whole app scenario as a program, built by `go vet ./...`. The session's token
is used only for collection and SQL reads: a graph's token names one graph
(SDKP-17). The binding table is checked against
`sdks/fixtures/facade-index.json` by `go_facade_matches_index`. Durable
workflows use the Resonate SDK pointed at `LOAMS_ENDPOINT/durable/` (SDKP-1).

## Agents: a machine key

An agent registered at the identity provider (a Zitadel machine user, design 76 §10) signs in
with its private-key JWT key. `MachineKeyFromEnv` reads `LOAMS_AGENT_KEY_FILE`,
`LOAMS_AGENT_ISSUER` and `LOAMS_AGENT_PROJECT`. It trades a 5-minute RFC 7523 assertion for an
access token whose audience is the Loams project, caches the token, and trades again a minute
before it expires. It uses only the standard library:

```go
source, err := loams.MachineKeyFromEnv(nil) // nil: http.DefaultClient
```

A sandboxed agent holds no key: agentd vends it a 15-minute token file instead.

## Not here, and why

Two things design §44 §7.5 and §7.6 ask for are deliberately absent because
the protos to hang them on do not exist yet. Writing them now would be a
hand-written API with no proto behind it, which is what §7.3 says not to do.

| Missing | Waits on |
|---|---|
| `client.Bulk()` over Arrow Flight (`arrow-go`) | the write RPCs — API1 Tasks 3 and 4. There is no `SqlService/QueryArrow` in this repository and no Flight SQL proto, so there is nothing to bind a Flight client to. |
| a per-call `ListAll` alias | a generated signature, which arrives with `ListCollections`. `loams.PaginateCall` is the iterator underneath it. |

## Publishing

### Module path, repository and tags

- **Module path:** `loams.dev/go`, declared once, in `go.mod`.
- **Repository:** this one. The module is the `sdks/go` subdirectory of
  `github.com/ostrium-labs/loams`; there is no mirror repository.
- **Tags:** a module in a subdirectory is versioned by tags prefixed with that
  subdirectory, so release `v0.1.0` of Loams is tag
  `sdks/go/v0.1.0` on the same commit.
  `.github/workflows/sdk-release.yml` pushes it on a release tag when the
  repository variable `SDK_PUBLISH_GO` is `true`, then asks `proxy.golang.org` for
  the version so the module is cached and in the checksum database.

### The `go-import` meta tag (owner action, `loams-cloud`)

For `go get loams.dev/go` to resolve, `https://loams.dev/go?go-get=1` (and
`https://loams.dev/go/...?go-get=1` for subpackages) must answer, over HTTPS and
without a bot challenge, with:

```html
<meta name="go-import" content="loams.dev/go git https://github.com/ostrium-labs/loams sdks/go">
```

The fourth field, `sdks/go`, is the subdirectory (Go 1.25+). As of 2026-10-13
`loams.dev/go?go-get=1` answers **403**, so nothing resolves yet.

Verify after the first tag, from an empty module:

```console
GOPROXY=direct go list -m loams.dev/go@v0.1.0
GOPROXY=https://proxy.golang.org go list -m loams.dev/go@v0.1.0
```

## Tests

```console
go vet ./...
gofmt -l .
go test ./...
```

The six tests SDK2 Task 2 requires, named exactly as the plan states. Go's test
tool only runs functions beginning with `Test`, so each carries its work in a
subtest with the canonical name — which is what `go test -v` prints and what
`-run` targets:

| Canonical name | Go test |
|---|---|
| `go_conformance_all_required_fixtures` | `TestGoConformanceAllRequiredFixtures` |
| `go_retry_reuses_idempotency_key` | `TestGoRetryReusesIdempotencyKey` |
| `go_error_reason_mapping` | `TestGoErrorReasonMapping` |
| `go_stream_resume_with_cursor` | `TestGoStreamResumeWithCursor` |
| `go_token_source_refresh` | `TestGoTokenSourceRefresh` |
| `go_pagination_iterator` | `TestGoPaginationIterator` |

```console
go test -run 'TestGoConformanceAllRequiredFixtures/go_conformance_all_required_fixtures' -v ./...
```

`TestConformanceTestNames` fails if a name is missing from the registry, so the
set cannot quietly shrink.

The conformance suite replays `sdks/fixtures`, preferring the shared Node fixture
server (`sdks/conformance/fixture-server.mjs`) when Node is on `PATH` and falling
back to an in-process replay of the same recorded bytes — a Go module's `go test`
should not need a JavaScript runtime. `LOAMS_TEST_ENDPOINT` points the whole suite
at a live `loams dev` instead.

## Dependencies

Four modules, all Apache-2.0 or BSD-3-Clause, all pinned. Provenance and the
reason each is here is in [DEPENDENCIES.md](DEPENDENCIES.md). No AGPL, and no
PgDog.

## Licence

Apache-2.0, same as the repository. See [LICENSE](LICENSE) and
[NOTICE](NOTICE).
