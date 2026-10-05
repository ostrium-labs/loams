# The Loams Java SDK

Design [§44](../design/44-unified-api-and-sdks.md) §7 (D604, D605, D606, D610,
D612), §9 row 2, §10.1 and §10.3; runtime contract R1–R10 in
[`docs/sdk/runtime-contract.md`](../sdk/runtime-contract.md); SDK2 Task 6.

```java
Client client = Client.builder()
        .endpoint("https://acme.loams.dev")
        .auth(TokenSources.apiKey(System.getenv("LOAMS_API_KEY")))
        .build();

GetInstanceResponse info = client.instance().getInstance(GetInstanceRequest.getDefaultInstance());

if (client.system().guard("live") == null) {
    StreamResume<Transition> resume = StreamResume.forMessages(
            t -> Long.toString(t.getEnd().getTs()),
            (cursor, original) -> WatchRequest.newBuilder()
                    .setResume(Resume.newBuilder()
                            .setLastVersion(stateVersion(cursor))
                            .build())
                    .build(),
            null,
            (cursor, t) -> lastSeen = cursor);

    try (Stream<Transition> stream = client.live().watch(request,
            CallOptions.withStreamResume(resume))) {
        while (stream.receive()) {
            apply(stream.message());
        }
    }
    if (stream.error() != null) {
        // the stream broke; nothing more will arrive
    }
}
```

```bash
cd sdks/java
./build.sh              # fetch dependencies, compile, run the suite
./build.sh test -t pagination
./build.sh stubs        # regenerate gen/ from proto/ (needs buf)
```

**Requires a JDK only.** No Maven, no Gradle, no network after the first run.
Tested on `javac 25.0.4.1` compiling with `--release 17`.

## What is generated and what is not

Two different things are called "generated" in this SDK, and only one of them is.

| | |
|---|---|
| **`gen/` — generated.** 236 files from `protoc-gen-java` over `proto/` via [`buf.gen.yaml`](buf.gen.yaml), pinned to `buf.build/protocolbuffers/java:v29.3`. Committed, like Go's `gen/` (D604), because a consumer of the published artifact builds from the artifact. `./build.sh stubs` regenerates; `scripts/sdk/drift.sh` does not yet check it (see below). | |
| **`src/dev/loams/facade/` — NOT generated.** | See below. |
| **`src/` and `test/` — hand-written.** The runtime and the suite. | |

### The facade is the Q604 hand-written fallback, not a generated surface

Design §44 §7.3 (D606) says the module and method surface is **generated** from
the `loams.options.v1` annotations, so thirteen languages cannot drift, and it
allows a hand-written fallback for a language where the generator proves costly
(Q604).

**`crates/loams-facade-gen` ships renderers for TypeScript, Python, Go and Rust.
It has no Java backend.** (`grep -rniE '\bjava\b' crates/loams-facade-gen/`
returns nothing.) This task could not add one: `crates/**` and
`scripts/sdk/gen.sh` are both outside the paths SDK2 Task 6 may write.

So `src/dev/loams/facade/` and `src/dev/loams/Modules.java` are a hand-written
transcription of the same annotations, field for field, and the conformance suite
checks them the same way it checks the generated facades.
`sdks/go/gen/facade/facade.go` is the same fallback in Go and says the same thing
at the top of its file. **They are not labelled generated anywhere.**

When `crates/loams-facade-gen/src/java.rs` lands, generation replaces these files
and they are deleted. **Do not add a method without the annotation that generates
it** — see §"What is not done" below.

### Why there is no `sdks/templates/java/template.env`

The manifest template belongs in `sdks/templates/java/template.env`, and the
contract is that `scripts/sdk/gen.sh` reads it. Neither is in this task's writable
paths, and **adding the template on its own would turn CI red**:

`scripts/sdk/drift.sh` enumerates `sdks/templates/*/template.env` and, for each
one, runs `buf generate` with the **`protoc-gen-loams-facade` Rust plugin**. A
`java/template.env` with a `FACADE_TEMPLATE` would make the drift check ask the
Rust generator for a Java facade it cannot render, and `check_language` would
report `java: buf generate failed`. So the template is owed **together with**
the renderer, not before it. What should land with the renderer:

```sh
FACADE_TEMPLATE=sdks/templates/java/buf.gen.facade.yaml
FACADE_OUT=sdks/java/src/dev/loams/facade
STUBS_TEMPLATE=sdks/templates/java/buf.gen.stubs.yaml
STUBS_OUT=sdks/java/gen
STUBS_COMMITTED=yes
ORDER=stubs,facade
```

`sdks/java/buf.gen.yaml` is already the stub template, in the same shape
`sdks/go/buf.gen.proto.yaml` and `sdks/python/buf.gen.yaml` use.

## The six conformance tests

Named exactly as `sdks/conformance/required.mjs` builds them from
`testName(language, short)`, because JUnit runs a method with any legal Java
identifier:

| Test | Where | What it pins |
|---|---|---|
| `java_conformance_all_required_fixtures` | `ConformanceTest` | Every case in `sdks/fixtures/index.json`, through the SDK's **public** surface: a success, a structured-reason error, and the unavailable-service path in all three of its shapes — the guard that costs no RPC, the refusal a call gets, and the refusal on a stream, where it arrives inside the Connect envelope on a 200. |
| `java_retry_reuses_idempotency_key` | `RetryIdempotencyTest` | An unkeyed mutation is **not** retried; the **same** key goes out on every retry of a keyed one; a caller-supplied key is kept. Asserted on the messages sent, not on a counter — a key regenerated per attempt would still produce three attempts. |
| `java_error_reason_mapping` | `ErrorReasonMappingTest` | All **26** reasons in `docs/api/reasons.md` map to their documented code and to a typed exception; the count is read from the page so a reason added there is covered the day it lands. Plus the three cases that must not be conflated: a reason from a newer server, a failure from below the API, and a mapped failure mapped twice. |
| `java_stream_resume_with_cursor` | `StreamResumeTest` | The resume machinery against a scripted receiver: the re-open carries the last **applied** cursor, nothing is re-yielded, and a failure the retry class does not cover is reported rather than spun on. The refusal-on-a-stream half is in the conformance test, against the corpus. |
| `java_token_source_refresh` | `TokenSourceTest` | A `401 token_expired` gets **exactly one** refresh and **one** retry, the refresh is not charged to the call's retry budget, a second expiry is reported, a source that cannot refresh is a no-op, 32 concurrent refreshes cost **one** exchange, and the bearer travels in `Authorization: Bearer` and not in the URL (checked against a real endpoint, because "not in the query string" is only checkable on the wire). |
| `java_pagination_iterator` | `PaginationTest` | The iterator follows `next_page_token` to the end over a **real generated message** (`ListApprovalsResponse`), yields items rather than pages, survives an empty page that still has a token, and reports a failed page and a non-paged binding instead of silently ending. |

`ConformanceTest.theSixCanonicalNamesExist()` reads the six names back through
reflection, so the set cannot quietly shrink to five.

## Deliberate skips, and why

None of these is a weakened test. Each is a place where a fixture would test the
stub rather than the SDK, and each is asserted so it fails loudly when it stops
being true.

- **`pagination_iterator`, end to end.** **No generated *call* is paged yet.**
  `ListApprovalsResponse` declares `approvals` and `next_page_token`, and the
  iterator pages it, but no module method returns one — API1 Task 2 owns the
  approvals module. `PaginationTest.noGeneratedCallIsPagedYet` fails the moment
  that lands, as the reminder to finish the clause.
- **`stream_resume_with_cursor`, end to end.** The API's only server stream
  (`loams.live.v1.LiveService/Watch`) is **never served in any variant**: it
  answers `feature_not_in_variant` in an end-stream envelope. So the corpus's
  `live_watch` case pins the *refusal* (asserted in the conformance test), and
  the resume machinery is pinned against a scripted `Receiver`.
- **`LOAMS_TEST_ENDPOINT`.** Not exercised. Nothing here builds a real instance.

## The fixture server

`Fixtures.start()` picks, in order:

1. `LOAMS_TEST_ENDPOINT` — a live `loams dev`, for `sdks/conformance/run.sh`.
2. `node sdks/conformance/fixture-server.mjs` — the shared server, so the Java
   suite really does run against the same one the other twelve do.
3. An **in-process replay** of `sdks/fixtures/recorded` using the JDK's own
   `com.sun.net.httpserver`, with the same matching rules — including a **404
   naming the gap** for anything unrecorded, never a silent 200.

A JVM suite should not need a JavaScript runtime to run, and the corpus is the
part that matters. Both paths are verified green (76/76 each).

## Runtime-contract coverage

`RuntimeContractTest` covers the clauses the six cannot: R4's consistency store
refusing to merge two tokens rather than reading stale data; R3's UUIDv7 layout,
including that the timestamp is **twelve** hex digits and not eight; the
streaming envelope, where a truncated frame is an error and not a clean end; the
Connect error shapes, including a body that is not a Connect error at all; the
JSON reader's refusals; the client's default retry budget; and a dependency
assertion that the runtime references nothing outside `dev.loams`, `java.*`,
`javax.*` and protobuf.

## Notes on the Java mapping

Places where Java forced a decision, each documented at the call site:

- **The error hierarchy is inheritance, not `errors.As`.** One class per code
  D611 names, with `TokenExpiredException extends UnauthenticatedException` and
  `FeatureNotInVariantException extends UnimplementedException`, so a caller who
  catches by code still matches by reason. `Errors.reasonOf` /
  `Errors.isLoamsError` are the Java spelling of the `errors.As` helper.
- **`Options.maxRetries()` is an `Integer`, and unset means 3, not none.**
  Java's `int` zero value makes "unset" and "no retries" indistinguishable, which
  would make the design's number something you had to opt *into*. `noRetries(true)`
  is the explicit opt-out. On a *call*, `withMaxRetries(0)` is unambiguous and
  does mean none.
- **The deadline is the caller's thread.** Java has no context to plumb, so every
  attempt and every backoff runs on the caller's thread and the retry loop checks
  the interrupt flag. A cancellation keeps its own code (`CANCELED`) rather than
  collapsing into `UNKNOWN`.
- **`PageIterator` is `Iterator` *and* `Iterable`, and `hasNext()` is not
  side-effect-free** because it fetches pages. An `advanced` flag stops `next()`
  from advancing a second time — without it, the last item of every page is
  dropped. That was a real bug this suite caught.
- **Two modules, one package.** `loams.live` and `loams.tables` are two facade
  names for `loams.live.v1`'s service, and a guard on either must cover both.

## What is **not** done

Stated plainly, because this project has been burned by work that claimed more
than it did.

- **No generated facade.** See above. `src/dev/loams/facade/` and
  `Modules.java` are hand-written and labelled as such. The fix is
  `crates/loams-facade-gen/src/java.rs`, outside this task's paths.
- **No `scripts/sdk/gen.sh java` case and no `sdks/templates/java/template.env`.**
  Both outside this task's paths, and adding the template without the renderer
  breaks `drift.sh`.
- **No CI job.** `.github/workflows/**` is outside this task's paths, so
  **nothing in CI runs this SDK**. `scripts/sdk/conformance/required.mjs` still
  names `./gradlew test` for `java`, which this SDK does not have — that entry
  needs to become `verified: false` with `./build.sh test` and then `true` once
  it is green in CI.
- **No `sdks/fixtures/results/java.json`.** `sdks/fixtures/**` is outside this
  task's paths. Go does not write one either.
- **No `pom.xml` and no Gradle build.** The plan's coordinates are
  `dev.loams:loams`, and a `pom.xml` is a one-file addition — but **neither `mvn`
  nor `gradle` exists on this box, so a build file would be committed unverified**,
  which is exactly the failure mode to avoid. `build.sh` is verified; a `pom.xml`
  is owed to whoever can run `mvn -v`.
- **No `connect-java`.** See [DEPENDENCIES.md](DEPENDENCIES.md). The wire is
  implemented in `src/dev/loams/connect/`, with compression and interceptors
  deliberately absent and the gaps listed.
- **No compression, no interceptors, no transport-level deadline.** Listed above
  with what each would take.
- **gRPC over HTTP/2 trailers need Java 18.** Reached reflectively so the floor
  stays 17; on 17 a gRPC stream reports it has no trailers rather than reporting
  success.
- **The hybrid query builder, the bulk API and `listAll` are not implemented.**
  They are blocked on API1 Tasks 2–4. `sdks/java` **refuses** them rather than
  offering a stub that would look like a working feature: there is no
  `Paginator` over a live RPC, no bulk method, and no `listAll`. Nothing in this
  SDK compiles that pretends otherwise.
- **`decisions`.** `docs/design/13-decision-log.md` is outside this task's
  paths, so no D7xx/Q7xx row was added. The deviations above — the hand-written
  facade and the hand-written transport — are the things an owner's row should
  record when the paths are open.

## Layout

```
sdks/java/
  build.sh              the only build; javac + curl + JUnitCore
  buf.gen.yaml          the protoc-gen-java template (D604)
  gen/                  GENERATED stubs, committed (236 files)
  src/dev/loams/        the runtime: connect/, facade/, and the SDK itself
  test/dev/loams/       the six conformance tests, runtime-contract tests, RunTests
  DEPENDENCIES.md       versions, licences, and the connect-java deviation
  NOTICE, LICENSE
```