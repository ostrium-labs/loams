# C++ SDK dependencies

Two, both found on the system, both licence-compatible with Apache-2.0:
**no AGPL, and no PgDog** (the licences `docs/design/13-decision-log.md` D64 and
D600 rule out).

| Dependency | Version verified here | Licence | Why it is here |
|---|---|---|---|
| [protobuf](https://github.com/protocolbuffers/protobuf) | 36.1.0 | BSD-3-Clause | The message codec and the runtime the committed stubs link against. It is not a choice: `gen/loams/**` is protoc's C++ output, and that output's `.pb.cc` **is** protobuf's implementation of the wire format. On top of that the SDK reads descriptors — `DeclaresIdempotencyKey` asks the **schema** whether a request declares `idempotency_key` rather than guessing from an object (R3) — and the generated factory is how the conformance driver builds a request message from a recording without a hand-written table of twenty-odd types. |
| [libcurl](https://curl.se/libcurl/) | 8.22.0 | curl-licence (MIT-like) | The HTTP layer behind `loams::HttpTransport`. `HttpTransport` is an interface, so a caller that already links an HTTP stack substitutes its own and this dependency disappears; the shipped implementation exists because the four encodings the corpus records (`application/json`, `application/proto`, `application/grpc-web+proto`, `application/grpc-web+json`) are all HTTP/1.1 POSTs, and hand-rolling TLS, proxies, keep-alive and happy-eyeballs is not what an SDK should be doing. |

## Abseil, which is not chosen but is linked

`google/protobuf/util/json_util.h` — which the SDK uses to read a Connect error
body and to encode a proto3 JSON request — returns an `absl::Status` in protobuf
**5.27 and later**. So anything linking this SDK links Abseil: `libprotobuf.so`
depends on `libabsl_status.so`, `libabsl_strings.so` and the rest, and a linker
that was not told about them fails at the last step with "DSO missing from
command line" for a library the caller never named.

`sdks/cpp/CMakeLists.txt` therefore takes its protobuf link line from
**`pkg-config`** rather than from the `protobuf::libprotobuf` imported target,
which names `libprotobuf` alone. Abseil is Apache-2.0, and it is listed here for
the licence audit rather than chosen.

## The protobuf version pin is load-bearing

`buf.gen.yaml` pins `buf.build/protocolbuffers/cpp:v36.1`, and the generated
headers carry

```c
#if PROTOBUF_VERSION != 7036001
#error "Protobuf C++ gencode is built with an incompatible version of"
```

That is protobuf's cross-version runtime guarantee, not a choice here: a gencode
file from 29.3 does not compile against a 36.x runtime, and the header's `#error`
says so by name. **The plugin version and the installed `libprotobuf` must be the
same protobuf release**, so the pin moves in the same commit as a protobuf bump.
Regenerating with a different pin produces a diff, and CI fails on it.

## What is deliberately *not* a dependency

- **grpc++ / gRPC C++.** Design §44 §4 (D600) puts the Connect protocol, gRPC and
  gRPC-Web on one port, and the C++ SDK speaks the two **HTTP-shaped** ones. A
  client that needs a full gRPC stack to make an HTTP POST is a heavier install
  for the same bytes, and grpc++ is not installed on the machines or the CI
  runners that develop this repository. Issue #295's title says grpc++; the wire
  does not, and §9 row 12's "gRPC **where no official Connect exists**" leaves the
  HTTP transports as the answer for a language that has neither. Recorded as a
  ruling.
- **A JSON library.** The only JSON on the wire is the Connect error body and the
  three proto3-JSON encodings, and protobuf already ships a correct proto3-JSON
  reader and writer for both. `DEPENDENCIES.md`'s Abseil note is the cost of that
  reuse; a hand-rolled JSON parser would have been a larger cost.
- **A UUID library.** A UUIDv7 is forty lines and the SDK ships it
  (`loams::UuidV7`). Thirteen SDKs minting thirteen different keys for the same
  logical call would defeat R3, and a dependency for forty lines is a licence to
  audit for ever.
- **A test framework.** gtest, Catch2 or doctest would be a fourth dependency for
  `tests/support.hpp`'s assertions. The suite's cost is that each test is a `main`
  that returns non-zero, which ctest already knows how to run.
- **An HTTP/2 wrapper.** libcurl does HTTP/2 where it is available; a server
  stream this SDK opens is framed over HTTP/1.1 or HTTP/2 and the SDK does not
  care which.