# Swift SDK dependencies

**There are none.** `Package.swift` declares an empty `dependencies:` array, and
this file explains why that is a decision rather than an oversight — and what is
planned in its place.

## Why the transport is written against the protocol instead

The language matrix (design §44 §9, wave 2) says Swift is **Connect over
`connect-swift`, stable**, and that remains the intent. `connect-swift` is the right
library: it is stable, follows semantic versioning, works over `URLSession` for the
Connect and gRPC-Web protocols, and needs no third-party networking dependency for
either.

It is not a dependency **yet**, for one reason:

> **The Swift toolchain was not available on the machine that wrote this SDK.**
> `command -v swift swiftc` returns nothing, so no API of `connect-swift` could be
> checked against a compiler.

Writing a runtime against an unverifiable dependency surface would make
`swift build` fail during **dependency resolution** — before compiling a line of
this SDK's own code — and the resulting failure would say something about
`connect-swift` rather than about the 3,000 lines next to it. A reader would then
have no way to tell whether the SDK is wrong or the manifest is.

So `Sources/Loams/Transport.swift` implements the Connect and gRPC-Web wire
protocol directly over `URLSession`, behind a protocol:

```swift
public protocol HTTPTransport: Sendable {
    var config: TransportConfig { get }
    func unary(_ request: WireRequest, bearer: String?) async throws -> WireResponse
    func serverStream(_ request: WireRequest, bearer: String?) async throws -> WireStream
}
```

`CallInvoker` is written against `HTTPTransport` and nothing else.
`connect-swift` exposes its own `HTTPClientInterface`, so adopting it is a
conformance plus a change of one factory call in `Loams.init(_:)` — not a rewrite
of the runtime.

### What adopting it would change

| File | Change |
|---|---|
| `Package.swift` | add `.package(url: "https://github.com/connectrpc/connect-swift.git", from: "1.2.3")` |
| `Transport.swift` | add a `ConnectSwiftTransport: HTTPTransport` wrapping `ProtocolClient`; keep `ConnectTransport` for the conformance fixture server, whose recorded bodies are byte-exact and easier to control directly |
| `Client.swift` | one factory line |

Nothing in `Errors.swift`, `Retry.swift`, `Idempotency.swift`, `TokenSource.swift`,
`Streams.swift`, `Pagination.swift`, `Consistency.swift`, `System.swift`,
`Facade.swift` or `Reason.swift` changes. That is the point of the seam.

`connect-swift` 1.2.3 requires `swift-protobuf` from 1.31.0, and both are
Apache-2.0, so neither is excluded by the licence policy D64 and D600 set (no
AGPL, no BSL, no SSPL).

## What is deliberately *not* a dependency

- **`swift-protobuf` (yet).** The runtime parses exactly one protobuf message —
  `loams.errors.v1.ErrorInfo`, out of an `Any`, because that is where R8's `reason`
  lives — and does it with a thirty-line reader in `ErrorInfoCodec.swift`. Taking
  `swift-protobuf` for one message, before the generated Swift stubs that would need
  it exist, buys a large dependency ahead of its use. The reader is total and
  refuses anything it does not understand rather than guessing; see that file's
  header for what it does and does not decode.
- **A UUID library.** A UUIDv7 is thirty lines and the SDK ships it
  (`loamsUUIDv7()`). Swift has no standard-library UUIDv7. A dependency for thirty
  lines is a licence to audit forever, and `UUID()` is a **v4** — unique but
  unordered, which loses the "sorts by creation time" property that lets an
  operator correlate keys in a log.
- **An HTTP/2 or networking wrapper.** `URLSession` is enough for the Connect and
  gRPC-Web protocols, and it is the only networking API in the SDK — which is also
  what keeps R10's "one client object runs in a browser" true without a second entry
  point.
- **An HTTP server for the fixture replay.** See the README's "Running the suite":
  Swift's standard library has no in-process HTTP server, and hand-writing one that
  never runs because the machine has no toolchain is the failure mode this SDK is
  already at risk of.

## Versions

None pinned, because none are depended upon. When `connect-swift` is added, pin
`from: "1.2.3"` — the version current on 2026-10-04, which is stable and follows
semantic versioning.