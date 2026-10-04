# Go SDK dependencies

Pinned by `go.mod` and `go.sum`. Everything here is Apache-2.0 or BSD-3-Clause:
**no AGPL, and no PgDog** (the licenses `docs/design/13-decision-log.md` D64 and
D600 rule out).

| Module | Version | Licence | Why it is here |
|---|---|---|---|
| [`connectrpc.com/connect`](https://github.com/connectrpc/connect-go) | v1.19.2 | Apache-2.0 | The Connect client (D604, D612: Go is Connect, not gRPC). It is the one library the Go SDK is built on: the protocol, the typed `*connect.Request`/`*connect.Response`, the server-stream reader, and the error decoding that makes `reason` reachable. |
| [`google.golang.org/protobuf`](https://github.com/protocolbuffers/protobuf-go) | v1.36.9 | BSD-3-Clause | The protobuf runtime: the generated messages, `proto.Clone`, and the descriptor walk `DeclaresIdempotencyKey` uses to read `idempotency_key` off a **schema** rather than off an object (R3). A direct dependency because the SDK reads descriptors; connect-go depends on it too. |
| [`golang.org/x/net`](https://pkg.go.dev/golang.org/x/net) | v0.33.0 | BSD-3-Clause | `http2.Transport`, for `TransportConfig.UseHTTP2` — the in-cluster caller that wants gRPC or Connect over one multiplexed connection. Optional in the sense that the default HTTP/1.1 path does not touch it. |
| `golang.org/x/text` | v0.21.0 | BSD-3-Clause | Indirect, required by `golang.org/x/net`. Listed for the licence audit, not chosen. |

## Versions, and why they are not the latest

Verified 2026-10-04 on go1.24.5 with `GOTOOLCHAIN=local`.

- **`connect-go` is pinned to v1.19.2, not v1.20.0 or v1.21.0.** Those require
  `go >= 1.25.0`, and the language matrix row 3 and the plan's floor are
  **Go >= 1.24**. A module whose `go.mod` says `go 1.25.0` cannot be consumed by
  a 1.24 toolchain, so taking v1.20 would silently raise the floor past what the
  design states. **Raising the floor is an owner's decision**, recorded as Q732.
- **`golang.org/x/net` is pinned to v0.33.0.** v0.59.0 requires `go >= 1.26.0`.
- **`protoc-gen-go` is v1.36.9**, the version the checked-in stubs were generated
  with, so `buf generate` reproduces the committed output rather than a diff.

## What is deliberately *not* a dependency

- **`github.com/google/uuid`** — a UUIDv7 is thirty lines and the SDK ships it
  (`UUIDv7`). A dependency for thirty lines is a licence to audit forever, and
  the alternatives (`gofrs/uuid`, `oklog/ulid`) would each make the thirteen SDKs
  produce different keys for the same logical call.
- **An Arrow library (`apache/arrow-go/v2`)** — `loams.bulk` over Arrow Flight is
  not implemented, because the write RPCs it would bind to arrive with API1 Tasks
  3 and 4 and there is no Flight SQL proto in this repository to generate from.
  Adding a large dependency for an API that does not exist yet would be the worst
  of both.
- **An HTTP/2 wrapper (`imroc/req)`** — `golang.org/x/net` and the standard
  library are enough for what a Connect client does.
