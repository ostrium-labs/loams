![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# Loams — the Swift SDK

Part of [design §44](../../docs/design/44-unified-api-and-sdks.md). One `Loams`
client with namespaced modules, speaking the Connect and gRPC-Web protocols on the
one port an instance serves.

> ## Status: written but **never compiled**
>
> The machine this SDK was written on **has no Swift toolchain**: `command -v
> swift swiftc` returns nothing. Every file here is therefore **UNVERIFIED** — it
> has never been compiled, and **no test in this directory has ever been run**.
>
> That is not a formality. Read `Tests/LoamsTests/` as the specification and
> `Sources/Loams/` as an unexecuted proposal to meet it. Before this SDK is
> trusted, somebody with a toolchain has to run:
>
> ```sh
> swift build
> swift test
> ```
>
> and expect to find compile errors. Expect them. This is a first draft against a
> contract, not a passing suite.
>
> Two concrete things a toolchain will settle, both called out where they bite:
>
>  - **The transport is written against the Connect wire protocol directly, over
>   `URLSession`, with no dependency.** The language matrix says Swift should use
>   `connect-swift`, and that remains the intent. Pinning a dependency whose entire
>   API surface could not be checked against a compiler would make `swift build`
>   fail during resolution — before compiling a line of this SDK — and would hide
>   whether the code below is correct. `HTTPTransport` is the seam where
>   `connect-swift` slots in. See `DEPENDENCIES.md`.
>  - **The facade is hand-written** (the D606 / Q604 fallback), because
>   `crates/loams-facade-gen` has no `swift.rs`. See `Sources/Loams/Facade.swift`'s
>   header.

## Install

```swift
.package(url: "https://github.com/ostrium-labs/loams-swift.git", from: "0.1.0")
```

**Nothing is published.** There is no tag, no SwiftPM registry entry, and no
release job. The version stays unpublished until the owner confirms the registry
account (design §44 §13).

## Quickstart

```swift
import Loams

let client = try Loams(
    Options(
        endpoint: URL(string: "https://acme.loams.dev")!,
        auth: APIKeyTokenSource(ProcessInfo.processInfo.environment["LOAMS_API_KEY"] ?? "")
    )
)

// What is this instance, and what does it serve? Needs no credential.
let info = try await client.instance.getInstance()
print(info.name, info.apiVersions)

// Is live sync in this build variant? Costs no RPC once the catalogue is cached.
do {
    try await client.system.guard("live")
} catch let error as FeatureNotInVariantError {
    print("not in this variant:", error.variant)
}
```

A runnable version is in `Examples/Quickstart`. It reads `LOAMS_ENDPOINT` and
`LOAMS_API_KEY` and prints no secrets.

## The six conformance tests

The names `sdks/conformance/check-languages.mjs` looks for, declared in
`Tests/LoamsTests/ConformanceTests.swift`:

| Name | Pins |
|---|---|
| `swift_conformance_all_required_fixtures` | R5, R8 — the whole recorded corpus through the public surface: a success, a structured-reason error, and the unavailable-service path in all three shapes |
| `swift_retry_reuses_idempotency_key` | R3 — one key per logical call, the **same** key on every retry, and `DeployRequest` left unkeyed |
| `swift_error_reason_mapping` | R8 — all 26 reasons map to a type; an unknown reason is surfaced, not dropped; a failure from below the API carries no reason; mapping twice loses nothing |
| `swift_stream_resume_with_cursor` | R7 — re-opens from the last cursor **applied**, without re-yielding what it already yielded |
| `swift_token_source_refresh` | R1 — exactly one refresh and one retry; a second expiry is reported; an API key's refresh is a no-op |
| `swift_pagination_iterator` | R6 — the token threading and the stop condition, and a refusal through the sequence for an unpaged binding |

**All six are UNVERIFIED.** They have not been compiled or run.

## Running the suite

The conformance tests need a fixture server. Two ways to get one, in the order
they are tried (`Tests/LoamsTests/Support/FixtureServer.swift`):

```sh
# 1. A live instance.
LOAMS_ENDPOINT=https://acme.loams.dev swift test

# 2. The shared server, if `node` is on PATH. This is what CI should do.
cd sdks/swift && swift test
```

With **neither** available the suite **fails**, naming both remedies. It does not
skip. The Go SDK has a third path — an in-process replay over `net/http/httptest`
— because Go's standard library has an HTTP server. Swift's does not, and
hand-writing one that never runs because the machine has no toolchain is exactly
the kind of green-but-unverified thing this repository has been burned by. A suite
that could not reach the corpus has not passed it.

`Tests/LoamsTests/RuntimeContractTests.swift` needs no server: it pins R2's
numbers, R4's session, R6's binding shape, R7's refusal, R8's codec, and the
transport's framing against a scripted `StubTransport`.

## What is deliberately not here

- **The hybrid query builder.** No hybrid query RPC exists: `loams.collection.v1`
  and `loams.query.v1` arrive with API1 Tasks 2 and 4. A builder now would be a
  hand-written API with no proto behind it, which is what §7.3 says not to do.
- **The bulk API.** The write RPCs arrive with API1 Tasks 3 and 4, and the API has
  no bidi (D420), so there is nothing to stream a request over.
- **`listAll`.** The pagination **iterator** is here and is tested. A per-call
  alias needs a generated signature to hang on, which arrives with
  `ListCollections` (API1 Task 2). `loams.paginate(binding:fetch:request:items:)`
  is the same function under its binding names.
- **Binary protobuf encoding.** `Codec.proto` is declared and **throws** rather
  than silently sending JSON under an `application/proto` content type. The
  descriptor-driven encoder comes with the generated Swift message types.
- **gRPC over HTTP/2.** It needs HTTP trailers, which `URLSession` does not
  expose. It arrives with `connect-swift`'s `ConnectNIO` product.
- **Typed `loams.live.v1` messages.** Only `loams.instance.v1` is typed. The live
  messages are `DynamicMessage`, which is honest about the fact that the corpus
  never returns one — it refuses every `loams.live.v1` call.

## The runtime contract

`docs/sdk/runtime-contract.md` is binding on every SDK, R1–R10. What Swift pins,
and where:

| Clause | Pinned by |
|---|---|
| R1 credentials | `swift_token_source_refresh`, `testRefreshingSourceFetchesOnFirstUse`, `testConcurrentRefreshesShareOneFetch`, `testEnvironmentIsReadEveryCall` |
| R2 retry and backoff | `testBackoffNumbersAreTheRulings`, `testBackoffIsFullJitterWithinItsCeiling`, `testRetryableCodes`, `testShouldRetryRefusals` |
| R3 idempotency | `swift_retry_reuses_idempotency_key`, `testUUIDv7ShapeAndOrdering`, `testUUIDv7TimeReadsTwelveDigits`, `testUUIDv7IsUniqueWithinAMillisecond` |
| R4 consistency tokens | `testSessionRefusesToMergeTwoDifferentTokens`, `testSessionIdempotenceAndEmptiness`, `testSessionClears` |
| R5 unavailable services | `swift_conformance_all_required_fixtures` (all three shapes), `testConformanceCatalogueIsShared` |
| R6 pagination | `swift_pagination_iterator`, `testProtoPackagesIncludeTheWellKnownTypes` |
| R7 streams | `swift_stream_resume_with_cursor`, `testUnimplementedStreamIsReportedNotRetried`, `testNoResumePolicyMeansNoReopen` |
| R8 errors | `swift_error_reason_mapping`, `testReasonRegistryHasAllTwentySix`, `testErrorInfoRoundTrips`, `testUnknownFieldIsSkippedAndKnownOnesSurvive`, `testErrorInfoIsFoundByTypeNotPosition` |
| R9 version reporting | `testConformanceReportsVersion` |
| R10 transports | `testContentTypesCoverTheCorpus`, `testGrpcWebStatusIsReadFromTrailers` |

**Not pinned, and why** — the same gaps the other SDKs have:

- **R2's `RetryInfo`** — no proto carries it, so the jittered backoff is all an SDK
  can do. The cap is pinned; the wire is not there.
- **R4's merge** — the token's encoding is not in the protos, so the session keeps
  what it was given and reports two tokens meeting as an error rather than merging
  them into a wrong one.
- **R6's and R7's end-to-end halves** — no RPC is paged and no stream is served in
  any variant, so both are pinned against a scripted transport. A fixture for an
  RPC the server does not serve would test the stub rather than the SDK.
- **R10's CORS preflight** — needs an origin and a server that answers preflights,
  and no test in this repository has one.

## Licence

Apache-2.0. See `LICENSE` and `NOTICE`.
