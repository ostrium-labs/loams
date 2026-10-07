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
go get loams.dev/go
```

`loams.dev/go` is a **vanity import path**. Until the `go-import` meta tag is
served (see [Publishing](#publishing) below), install from the mirror repository:

```console
go get github.com/ostrium-labs/loams-go
```

or, from a checkout of this repository, with a `replace`:

```go
require loams.dev/go v0.0.0
replace loams.dev/go => ../../path/to/sdks/go
```

## What is generated and what is hand-written

| Path | Provenance |
|---|---|
| `gen/loams/**` | **Generated.** `protoc-gen-go` and `protoc-gen-connect-go` over `proto/` (D604). Reproduce with `buf generate --template sdks/go/buf.gen.go.yaml`. |
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

## Not here, and why

Three things design §44 §7.5 and §7.6 ask for are deliberately absent because
the protos to hang them on do not exist yet. Writing them now would be a
hand-written API with no proto behind it, which is what §7.3 says not to do.

| Missing | Waits on |
|---|---|
| the typed hybrid-query builder | `loams.collection.v1` / `loams.query.v1` — API1 Tasks 2 and 4. Q604 (one builder or thirteen) is unanswered. |
| `client.Bulk()` over Arrow Flight (`arrow-go`) | the write RPCs — API1 Tasks 3 and 4. There is no `SqlService/QueryArrow` in this repository and no Flight SQL proto, so there is nothing to bind a Flight client to. |
| a per-call `ListAll` alias | a generated signature, which arrives with `ListCollections`. `loams.PaginateCall` is the iterator underneath it. |

## Publishing

**Not published.** No Go module-proxy account is confirmed, so there is no
`GOPROXY` push, no tag and no `sdk-go-v*` release from this branch.

### Module path and the mirror repository

- **Module path:** `loams.dev/go`, declared once, in `go.mod`.
- **Mirror repository:** `github.com/ostrium-labs/loams-go` (D619). It holds the
  git tags the proxy reads; the module path stays `loams.dev/go`, which is how
  `go-import` works.
- **`ostrium-labs/loams-go` does not exist yet.** Creating a GitHub repository is
  an owner action, and it needs an authenticated owner token. It was not created
  from this branch and must not be created with a token that turns out not to
  have the right scope.

### The `go-import` meta tag — **BLOCKED, owner action**

For `go get loams.dev/go` to resolve, something has to answer
`https://loams.dev/go?go-get=1` with:

```html
<meta name="go-import" content="loams.dev/go git https://github.com/ostrium-labs/loams-go">
```

The go command reads that tag, learns where the module lives, and fetches from
there. Nothing about it can be done from inside this repository: it needs the
`loams.dev` site to serve it, and the mirror repository to exist.

**This is not implemented here and is not claimed to work.** What the owner has to
do, in order:

1. **Create the mirror repository** `github.com/ostrium-labs/loams-go`, public,
   with a `main` branch. Either empty, or with an initial commit that copies
   `sdks/go/**` (D619 says an `sdk-mirror` workflow writes it; until that workflow
   exists, a copy is the interim).
2. **Serve the meta tag.** Add a route in the `loams-cloud` app that answers
   `GET /go?go-get=1` with a minimal HTML page:

   ```html
   <!doctype html>
   <html><head>
     <meta name="go-import" content="loams.dev/go git https://github.com/ostrium-labs/loams-go">
   </head><body>Go module loams.dev/go</body></html>
   ```

   The tag must be on the **root-relative path `/go`**, and the page must be
   served over HTTPS with a valid certificate: the go command refuses plain
   `http://` meta responses, and a redirect loses the tag unless the final
   response carries it.
3. **Tag a release.** `git tag v0.1.0 sdk-go-v0.1.0` in the mirror (D615 names
   `sdk-<lang>-v<semver>` for the release tag and `v<semver>` for the module tag),
   and push the tag.
4. **Verify** — this is the check that says it works, and it is the only one:

   ```console
   GOPROXY=direct go install loams.dev/go/cmd/loams-example@latest
   ```

   Without the tag and without step 2, `go get loams.dev/go` fails with
   `invalid version: unknown revision`. The failure names which step is missing,
   which is why the steps are ordered.

5. **Once the proxy has it**, publishing is `go list -m` plus a push through
   `proxy.golang.org`, which needs `GONOSUMDB`/`GONOSUMCHECK`-free credentials:
   `GOPROXY=https://proxy.golang.org go list -m loams.dev/go@latest` after a tag
   is fetched directly. **Not done here.**

### Release tags

D615 names `sdk-go-v<semver>` as the tag that triggers the release job. **The
release job stays a dry run**: no registry credentials, no module-proxy account,
no publish. This branch creates no tag.

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
