# Java SDK dependencies

Pinned by version in [`build.sh`](build.sh). Everything here is Apache-2.0 or
BSD-3-Clause: **no AGPL, and no PgDog** (the licences
`docs/design/13-decision-log.md` D64 and D600 rule out).

| Artifact | Version | Licence | Why it is here |
|---|---|---|---|
| `com.google.protobuf:protobuf-java` | 4.29.3 | BSD-3-Clause | The protobuf runtime: the generated messages, `Message.Builder`, and the descriptor walk `Idempotency.declaresIdempotencyKey` and `PageIterator` use to read `idempotency_key`, `approvals` and `next_page_token` off a **schema** rather than off an object (R3, R6). It is the only runtime dependency the SDK's own code has — asserted by `RuntimeContractTest.theRuntimeNeedsOnlyProtobuf`. |
| `com.google.protobuf:protobuf-java-util` | 4.29.3 | BSD-3-Clause | `JsonFormat`, the proto3 JSON mapping. A **test-and-caller** dependency: the SDK's default wire format is binary protobuf (D612), so a program that only ever makes binary calls never loads it. Kept because a caller that asks for `application/json` needs it, and because the corpus carries `_json` fixture cases. |
| `com.google.guava:guava` | 33.3.1-jre | Apache-2.0 | Indirect. Required by `protobuf-java` itself; it must be on the classpath alongside it or `protobuf-java` fails to initialise. Listed for the licence audit, not chosen. |
| `com.google.guava:failureaccess` | 1.0.2 | Apache-2.0 | Indirect. Required by `guava`. Listed for the licence audit, not chosen. |
| `junit:junit` | 4.13.2 | EPL-1.0 | The test framework. **JUnit 4 rather than 5** so the suite runs from `org.junit.runner.JUnitCore` off a plain JDK with no launcher jar and no module path — which is what makes `javac`/`java` a sufficient build (see below). Test-scope only; it is not a dependency of the published artifact. |
| `org.hamcrest:hamcrest-core` | 1.3 | BSD-3-Clause | Indirect, required by JUnit 4. Listed for the licence audit, not chosen. |

## connect-java is **not** a dependency, and that is a deliberate deviation

Design §44 §9 row 2 says Java SDKs use
[`connect-java`](https://github.com/connectrpc/connect-java), and this SDK does
**not**. It implements the Connect wire protocol itself, in
`src/dev/loams/connect/`. Two reasons, both about this repository rather than
about Java:

1. **The artifacts are not resolvable from Maven Central from this environment.**
   `connectrpc/connect-api` and `com/connectrpc/connect-api` both answer **404**
   there, while `com/google/protobuf/protobuf-java`, `io/grpc/grpc-core` and the
   official Google mirror of Central all resolve. Probed 2026-10-04; the mirror is
   current (protobuf-java 4.29.3 and grpc-core 1.71.0, both 2025 releases,
   resolve), so this is the artifact coordinates being unavailable rather than a
   stale cache.
2. **Neither Maven nor Gradle is installed** on the machines this SDK is built
   on, so there is no resolver to pull a transitive tree with.

### What the hand-written transport does and does not do

It is small on purpose, and it is honest about its edges:

| | |
|---|---|
| Connect unary (`application/proto`) | Yes. A bare HTTP `POST` with the message as the body — the same bytes `curl` sends. |
| Connect server streaming (`application/connect+proto`) | Yes, including reading the **end-of-stream envelope**, which is where a refusal arrives on a 200. |
| gRPC-Web (`application/grpc-web+proto`) | Yes, including the trailer frame and the `google.rpc.Status` in `grpc-status-details-bin`, which is the only place the `ErrorInfo` travels on that protocol. |
| gRPC (`application/grpc+proto`) | Partly. HTTP trailers are read through `HttpResponse.trailers()`, which arrived in **Java 18**; this SDK's floor is Java 17, so the call is made reflectively. On 17 a gRPC stream reports that it has no trailers rather than declaring itself successful. See `ConnectTransport.grpcTrailers`. |
| Request/response compression | **No.** Nothing negotiates it, and the transport does not send `Accept-Encoding` — asking for compression and then not decoding it would be a way to produce an unreadable body. |
| Interceptors | **No.** The runtime's retry, credentials and error mapping are the extension points instead. |
| Per-call deadlines | No transport-level deadline. The caller's own thread bounds a call, and `CallInvoker` documents how that composes with the retry loop. |

**What to check when this is replaced** with `connect-java`: the end-of-stream
error, the gRPC-Web `grpc-status-details-bin` decode, the `%20`-percent-decoded
`grpc-message`, and that `Connect-Protocol-Version` is still sent. Those four are
where a naive swap silently loses a `reason`.

## Why the JSON is hand-rolled

`src/dev/loams/internal/Json.java` is ~200 lines rather than a dependency. The
SDK reads JSON in exactly two places — a Connect error body and an
`/oauth/token` response — and taking a JSON library for that would put its
version and its security history into every Loams application's dependency tree
for the sake of a few hundred bytes of parsing. It is **internal** (not public
API, not covered by the six canonical names) and it is unit-tested against the
shapes it is actually fed, including the ones it must *refuse*
(`RuntimeContractTest.theJsonReaderRefusesWhatItDoesNotUnderstand`).

## Building without a resolver

`build.sh` fetches the six jars above with `curl` into `.build/lib/`, skipping
any already present. To build offline, copy `.build/lib/` from a machine with
network access; after that `build.sh` needs no network at all.

## Provenance of the versions

Verified 2026-10-04 with `javac 25.0.4.1`, compiling with `--release 17`.

- **`protobuf-java` is 4.29.3, not 5.x.** Protobuf Java 5 raised its own floor to
  Java 8+ *and* changed generated-code compatibility; taking it would change what
  `protoc-gen-java` emits and so invalidate the committed `gen/` against the
  pinned plugin. 4.29.3 is the newest release the pinned
  `buf.build/protocolbuffers/java:v29.3` plugin is paired with.
- **The `protoc-gen-java` remote plugin is pinned to `v29.3`** in
  `buf.gen.yaml`, for the reason every generated SDK here pins its generator: a
  generation difference must be a version bump in a diff, not whatever the
  runner happened to have.