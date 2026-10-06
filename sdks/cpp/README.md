# The Loams C++ SDK

The C++ client for [Loams](https://loams.dev), from the unified Connect API of
design [§44](../../../docs/design/44-unified-api-and-sdks.md). One client object
with namespaced modules, the runtime contract of
[docs/sdk/runtime-contract.md](../../../docs/sdk/runtime-contract.md), and the
shared conformance corpus of design §44 §10.4.

**Connect over HTTP, not gRPC.** One port serves the Connect protocol, gRPC and
gRPC-Web (D600), and this SDK speaks the two HTTP-shaped ones:
`application/json`, `application/proto`, `application/grpc-web+proto` and
`application/grpc-web+json`. That is why there is no grpc++ dependency and why
`DEPENDENCIES.md` records one instead.

## Requirements

C++17, CMake ≥ 3.20, **protobuf 36.1**, **libcurl 8.x**, and pthreads. All three
are preinstalled on the machines and CI runners that develop this repository:

```sh
sudo apt-get install -y cmake clang protobuf-compiler libprotobuf-dev libcurl4-openssl-dev
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

## What the runtime does

| Clause | Where | What it means here |
|---|---|---|
| **R1** credentials | `loams/token_source.hpp` | A bearer from a token source, in `Authorization: Bearer`, **never** in a URL. A `401` with `reason = token_expired` triggers exactly **one** refresh and **one** retry; a second expiry reaches the caller as a `TokenExpiredError`. An API key cannot refresh, so its refresh is the no-op the contract describes. |
| **R2** retry | `loams/retry.hpp` | A call's retry class comes from the binding table, not from a guess. Retryable: `unavailable`, `deadline_exceeded`, `resource_exhausted`. Base 100 ms, ×2, cap 2 s, 3 retries, **full** jitter. `max_retries = 0` disables. |
| **R3** idempotency | `loams/idempotency.hpp` | A mutating call that declares `idempotency_key` gets one **per logical call**, minted before the first attempt, and **the same key on every retry** — a UUIDv7 unless the caller supplies one. The request is cloned, never mutated in place. |
| **R4** consistency | `loams/consistency.hpp` | `session_consistency` is **off by default**. A store folds returned tokens in and attaches them to later reads; two *different* tokens meeting is an **error**, not a merge, because the `v1:` encoding is not in the protos yet and a silently-merged token reads stale data. |
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