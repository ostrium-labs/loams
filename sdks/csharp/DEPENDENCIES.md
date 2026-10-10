# C# SDK dependencies

Pinned by `src/Loams/Loams.csproj`. Everything here is Apache-2.0 or BSD-3-Clause:
**no AGPL, and no PgDog** (the licences `docs/design/13-decision-log.md` D64 and
D600 rule out).

| Package | Version | Licence | Why it is here |
|---|---|---|---|
| [`Google.Protobuf`](https://github.com/protocolbuffers/protobuf) | 3.28.3 | BSD-3-Clause | The protobuf runtime: the generated messages, `MessageParser`, and the descriptor walk the SDK reads `idempotency_key` and `idempotency_level` off — a **schema** rather than an object (R3, D610). The conformance driver asks the same descriptors through `InternalsVisibleTo`, so a binding whose type name does not resolve fails for the reason it would fail in a call. |

**That is the whole list.** One package, for one job.

## Versions, and why they are not the latest

Verified 2026-10-06 on **.NET SDK 8.0.425**, `net8.0`, Release.

- **`Google.Protobuf` is pinned to 3.28.3**, the version the checked-in stubs
  under `src/Loams/Gen/` were generated against, so `buf generate` plus
  `scripts/sdk/drift.sh` reproduces the committed output rather than a diff.
- **`net8.0` and nothing else** — no `netstandard2.1`. The floor is stated in
  `src/Loams/Loams.csproj`; a second target would double the matrix to test for a
  runtime this SDK does not need.

## What is deliberately *not* a dependency

This is the load-bearing part of the file, because C# is the language where the
obvious answer is wrong twice over.

- **`Grpc.Net.Client`** — what design §44 §9 row 8 names for C#, and it cannot do
  this SDK's job. It speaks gRPC and gRPC-Web but **not Connect**, and Connect is
  what the corpus's `application/json` and `application/proto` unary cases and
  the whole `application/connect+*` streaming family are recorded in — 13 of the
  28 required fixtures, including `live_watch`, the only case that proves a
  refusal arriving *inside* a stream envelope rather than as an HTTP status. R10
  ("one port, several protocols, one client") is unreachable without it.
  `Grpc.Net.Client.Web` is the browser transport and lives in a WASM-only
  package. The wire is therefore here, over `System.Net.Http` (`Transport.cs`),
  with `Google.Protobuf` for the messages.
- **`Google.Protobuf`'s own `JsonFormatter`** — it emits `{ "field": value }` with
  spaces after the braces and around every colon, while the corpus records the
  **compact** proto3 JSON mapping and `fixture-server.mjs` compares the request
  bytes byte for byte. A space is a different request. `CompactJson.cs` writes
  the compact form, in field-number order, with defaults omitted.
- **`System.Text.Json`'s base64** — for the error path, `Convert.FromBase64String`
  rejects unpadded standard base64 and **7 of the 10 distinct `details[].value`
  strings in the corpus are unpadded**, so it threw on most of the corpus's
  reasons and the throw was caught into "this error has no reason". `Base64.cs`
  is thirty lines and one decoder rather than a dependency whose only job is the
  same thirty lines across thirteen SDKs. See D646.
- **A DI container, a logging abstraction, or a resilience library (Polly)** — the
  retry loop is `CallInvoker.cs` because D610's backoff numbers are part of this
  SDK's contract and a caller who wants Polly wraps the client. `ITokenSource` is
  the one seam, and it is three members.
- **A test framework beyond xunit**, and no mocking library: the suite's
  `StubHandler` is an `HttpMessageHandler`, which is the seam the transport
  already exposes.

## The rule this file exists to keep

**One package reference, and every addition is justified here first.** The value
is not the licence count — it is that thirteen SDKs must produce the **same bytes
for the same call**, and a dependency that formats, encodes or frames on its own
is a place where two of them will quietly differ. That is the failure this
repository's conformance corpus exists to catch, and it is cheaper not to build
the opportunity.