![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# The Loams C++ SDK

The C++ client for [Loams](https://loams.dev), from the unified Connect API of
design [§44](../../docs/design/44-unified-api-and-sdks.md). One client object
with namespaced modules, the runtime contract of
[docs/sdk/runtime-contract.md](../../docs/sdk/runtime-contract.md), and the
shared conformance corpus of design §44 §10.4.

**Connect over HTTP, not gRPC.** One port serves the Connect protocol, gRPC and
gRPC-Web (D600), and this SDK speaks the two HTTP-shaped ones:
`application/json`, `application/proto`, `application/grpc-web+proto` and
`application/grpc-web+json`. That is why there is no grpc++ dependency and why
`DEPENDENCIES.md` records one instead.

## Install

Download `loams-cpp-0.1.2.tar.gz` from the
[GitHub release](https://github.com/ostrium-labs/loams/releases) and use it from
CMake with `find_package(loams CONFIG)`. There are no vcpkg or Conan ports yet.

## Requirements

C++17, CMake ≥ 3.20, **protobuf 36.1**, **libcurl 8.x**, and pthreads. The
protobuf runtime must be exactly 36.1 (the stubs' gencode version). A distribution
that ships it (Arch does) is enough; Ubuntu 24.04's `libprotobuf-dev` is 3.21, so
CI builds the v36.1 tag from source with its vendored Abseil and caches it (the
`sdk-cpp` job in `.github/workflows/ci.yml` has the exact CMake flags):

```sh
sudo apt-get install -y cmake clang libcurl4-openssl-dev pkg-config
# then protobuf v36.1 as in the sdk-cpp job, with PKG_CONFIG_PATH and
# CMAKE_PREFIX_PATH pointing at its install prefix
```

## Build and test

```sh
cmake -S sdks/cpp -B sdks/cpp/build -DCMAKE_BUILD_TYPE=Release
cmake --build sdks/cpp/build -j 4
cd sdks/cpp/build && ctest --output-on-failure
```

Seven tests: the six `sdks/conformance/required.mjs` names for C++ —
`cpp_conformance_all_required_fixtures`, `cpp_retry_reuses_idempotency_key`,
`cpp_error_reason_mapping`, `cpp_stream_resume_with_cursor`,
`cpp_token_source_refresh`, `cpp_pagination_iterator` — and
`cpp_conformance_test_names`, which asserts the other six are registered with CTest.

One of them writes `sdks/fixtures/results/cpp.json`, and that file is what
`node sdks/conformance/check-languages.mjs --check cpp` reads:

```sh
node sdks/conformance/check-languages.mjs --check cpp
sdks/conformance/run-test.sh cpp conformance_all_required_fixtures
```

## Quickstart

```cpp
#include <loams/loams.hpp>

int main() {
  // `EnvToken()` reads LOAMS_API_KEY, then LOAMS_TOKEN, then sends nothing —
  // which is what GetInstance needs, so this runs against a fresh `loams dev`.
  auto loams = loams::MakeLoams({.endpoint = "http://127.0.0.1:8080",
                                 .token_source = loams::EnvToken()});

  loams::instance::v1::GetInstanceResponse info;
  loams->Instance()->GetInstance(&info);
  std::cout << info.name() << " " << info.server_version() << "\n";

  // Feature detection **before** calling. A package this build does not serve
  // answers `unimplemented` with reason `feature_not_in_variant`; the guard asks
  // the catalogue instead and costs no RPC once it is cached.
  try {
    loams->System()->Guard("live");
  } catch (const loams::FeatureNotInVariantError& error) {
    std::cout << "not in this variant: " << error.Variant() << "\n";
  }

  // Errors are typed and branched on by type, never by message.
  try {
    loams::live::v1::QueryRequest request;
    loams::live::v1::QueryResponse response;
    loams->Live()->Query(&request, &response);
  } catch (const loams::LoamsError& error) {
    std::cerr << loams::ToString(error.CodeValue()) << " / " << loams::ToString(error.ReasonValue()) << "\n";
  }
}
```

`examples/quickstart/main.cpp` is the same thing, compiled by the build so a change
that breaks it breaks the build.

## The shape

```cpp
auto loams = loams::MakeLoams({.endpoint = endpoint});

loams->Instance()->GetInstance(&info);                    // loams.instance
loams->Live()->Query(&request, &response);                // loams.live
loams->Tables()->Mutate(&request, &response);              // loams.tables (same RPC)
loams->Approvals()->DecideApproval(&request, &response);  // loams.approvals
loams->Devices()->ListDevices(&request, &response);       // loams.devices
loams->Operations()->ListOperations(&request, &response);  // loams.operations
loams->Notifications()->ListNotifications(&r, &response);  // loams.notifications
loams->System()->Available("live");                       // feature detection
loams->System()->Version();                               // PROTO_REV vs api_versions
```

Module and method names are `PascalCase`, which is design §44 §7.1's rule for C++.

## Build an app

The app-building modules (SDKP) are `Namespaces()`, `Collections()`,
`Documents()`, `Search()`, `Vector()` and `Graph()` (experimental), with the
`Schema`, `Filter` and `Search` builders (`loams/builders.hpp`) and
`Collection(name)`, a handle that fills the collection in.
[`examples/notes/main.cpp`](examples/notes/main.cpp) is the whole flow, and the
build compiles it:

```cpp
loams::Options options = loams::OptionsFromEnv();  // LOAMS_ENDPOINT, LOAMS_API_KEY, LOAMS_NAMESPACE
options.session_consistency = true;                 // reads see this client's writes
auto client = loams::MakeLoams(std::move(options));

client->Namespaces()->Create("app");
client->Collections()->Create("notes",
    loams::Schema().Keyword("title").Text("body").Keyword("tags").I64("n").Vector("embedding", 4));

auto notes = client->Collection("notes");
notes.Upsert({loams::Document::FromJson(1, R"({"title":"Refunds","body":"refund policy","tags":["policy"]})",
                                        {{"embedding", {1, 0, 0, 0}}})});

auto hits = notes.Search(loams::Search()
    .Text("refund", {{"body"}, {}})
    .Vector("embedding", {1, 0, 0, 0})
    .Fuse(loams::Fusion::Rrf(60))  // or Dbsf(), Weighted({0.7f, 0.3f})
    .Where(loams::Filter::Eq("tags", loams::Json("policy")))
    .Highlight("body")
    .Limit(10));
```

- **Answers are the response messages** (`collection_v1::SearchResponse` and so on);
  `loams::DocId::From(hit.pk())` gives a key back.
- **Writes.** `Documents()->Upsert`, `Delete`, `Patch` and `Write` (raw ops) answer
  a consistency `token`. `WriteBatch(collection, docs, {chunk_rows, chunk_bytes})`
  sends chunks one at a time, each with its own idempotency key, and answers a
  `BatchResult` (merged `token`, `chunks`, `rows`); a failed chunk throws
  `BatchError` with `ChunksApplied()` and the `Token()` of the chunks before it.
- **Reads.** `Get`, `Count`, `Scroll` / `ScrollAll` / `ScrollEach`, `Search()->Query`,
  `QueryAll` / `QueryEach` (follow `search_after`; the sort must end with
  `Sort::Pk()`), `Text`, `Hybrid` and `Vector()->Search`. `ReadOptions::consistency`
  is `strong`, `eventual` or a token.
- **Graph** (experimental): `Create`, `Query` (GQL with `GraphValue` parameters; a
  read-only statement retries as a read), `QueryBatch`, `Explain`, `ListAll`,
  `Delete`; `GraphModule::Records(response)` gives rows as maps.
- **The default namespace** (`Options::default_namespace`, or `CallOptions::ns`)
  fills every request that names none; with neither the call throws
  `InvalidArgumentError` before any network call. **Timeouts**: `Options::timeout`
  (30 s) is a unary call's budget, retries included, sent as `connect-timeout-ms`;
  `CallOptions::timeout` overrides it. A spent budget throws `DeadlineExceededError`
  with `UnknownReason() == "client_deadline_exceeded"` and is never retried.
- **SQL** (`loams/data.hpp`): `Sql()->Query(sql, {Json(1), …}, {max_rows, consistency})`
  answers `columns` and `rows`; `SqlModule::Records(res)` gives rows as maps. `QueryArrow`
  answers the Arrow IPC bytes. SQL reads see the session's collection writes.
- **Streams:** `Streams()->Create`, `Get`, `List` / `ListAll`, `Produce(stream, records,
  {partition, producer_id, producer_seq})` (a `ProduceRecord` has an optional key, a value and
  headers; a repeated producer sequence is deduplicated), `Fetch`, `ProduceEvents` /
  `FetchEvents` (CloudEvents as a JSON array), and `Subscribe(stream, {partition, offset})`, a
  `loams::Subscription` pull reader (`Next()`, `Cancel()`, `Offset()` to resume).
- Durable execution has no module: use a Resonate SDK with `LOAMS_ENDPOINT/durable/`.
- `ctest -R cpp_app_` runs the app scenario against `LOAMS_TEST_ENDPOINT`
  (skipped without one); the other `cpp_*` app tests need no server.

## What the runtime does

| Clause | Where | What it means here |
|---|---|---|
| **R1** credentials | `loams/token_source.hpp` | A bearer from a token source, in `Authorization: Bearer`, **never** in a URL. A `401` with `reason = token_expired` triggers exactly **one** refresh and **one** retry; a second expiry reaches the caller as a `TokenExpiredError`. An API key cannot refresh, so its refresh is the no-op the contract describes. |
| **R2** retry | `loams/retry.hpp` | A call's retry class comes from the binding table, not from a guess. Retryable: `unavailable`, `deadline_exceeded`, `resource_exhausted`. Base 100 ms, ×2, cap 2 s, 3 retries, **full** jitter. `max_retries = 0` disables. |
| **R3** idempotency | `loams/idempotency.hpp` | A mutating call that declares `idempotency_key` gets one **per logical call**, minted before the first attempt, and **the same key on every retry** — a UUIDv7 unless the caller supplies one. The request is cloned, never mutated in place. |
| **R4** consistency | `loams/consistency.hpp` | `ConsistencyToken` parses the `v1:` encoding and merges two tokens (max offset per stream and partition). `session_consistency` is **off by default**; with it on, the app modules merge every `token`, `read_token` and `consistency_token` into the session (one per proto package) and send `consistency.at_least` on every read that leaves `consistency` unset. |
| **R5** unavailable services | `loams/system.hpp` | `Available`/`Served`/`Unavailable`/`Guard` read `GetInstance.services[]`, cached for the life of the client with one shared in-flight fetch. `Guard` raises the **same** `FeatureNotInVariantError` a call does, so one `catch` covers both. |
| **R6** pagination | `loams/pagination.hpp` | `PageIterator` follows `next_page_token` to the end and yields **items**, not pages. It is refused at construction for an unpaged binding, because an iterator over one returns a single page and looks like a complete list. |
| **R7** streams | `loams/stream.hpp` | Server streams only (D420). A `MessageStream` tracks the cursor of every message and re-opens from it on a retryable failure, so a reconnect neither misses what changed nor re-yields what it already yielded. A stream with no resume **reports** rather than starting over. |
| **R8** errors | `loams/error.hpp`, `loams/reason.hpp` | `Code` gives the class; `Reason` — the closed enum generated from `docs/api/reasons.md` — is the branch. Three cases stay distinct: a newer server's reason is surfaced in `UnknownReason`, a failure below the API is a `TransportError` carrying no reason, and a mapped error stays mapped. |
| **R9** version | `loams/version.hpp` | `kProtoRev` beside the server's `api_versions`. A package the SDK speaks and the server does not is a **warning**, returned in `VersionReport::warnings` and never thrown. |

## What is hand-written, and why

`include/loams/facade.hpp` and `src/bindings.cpp` are the **Q604 fallback**: a
hand-written facade, not generated output. Design §44 §7.3 says the module surface
is generated from the protos' `loams.options.v1` annotations by
`protoc-gen-loams-facade`, and that "if the plugin proves too costly for a
language, that language falls back to a hand-written facade checked by the same
conformance suite (D606, Q604)". There is no `sdks/templates/cpp/` and no C++
renderer, so the binding table is transcribed.

What makes the transcription checkable rather than a guess is that
`cpp_conformance_all_required_fixtures` drives **every** required fixture's
recorded steps through it: a binding that named the wrong RPC path, the wrong
retry class or the wrong idempotency field fails there. A binding no fixture
exercises is a comment until something exercises it.

The **runtime** is hand-written by design, whatever the facade is — §44 §7.3:
"only the runtime … and the typed builders are hand-written, once per language".

## Generated stubs

`gen/loams/**` is protoc's output over this repository's `proto/` tree, committed
so a consumer needs no `protoc`, no `buf` and no network, and regenerated with:

```sh
buf generate --template sdks/cpp/buf.gen.yaml
git diff --exit-code -- sdks/cpp/gen
```

**The plugin pin and the installed protobuf must be the same release** — the
generated headers carry a `#error` that says so. `DEPENDENCIES.md` has the details.

## Licence

Apache-2.0 (`LICENSE`), with `NOTICE` and `DEPENDENCIES.md`. Nothing here is AGPL
and nothing here is PgDog.
