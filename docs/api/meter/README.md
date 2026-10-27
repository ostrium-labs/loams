# Loams Meter: usage event schemas

The JSON Schemas of Loams' usage events (design [§63 §6](../../design/63-loams-meter.md), ME1 Task 4). Each schema's `$id` is the `dataschema` a Loams producer puts on its events: `urn:loams:json:<type>`.

- [`io.loams.dev.usage.event.json`](io.loams.dev.usage.event.json) is the envelope: a CloudEvent 1.0 in JSON structured mode, in the Loams usage profile (§63 §6.1). OpenMeter's `POST /api/v1/events` accepts it unchanged. `subject` is required, `data` is a JSON object, and `loamslease` is the only extension Loams adds; exporters strip it.
- The files below describe each type's `data`. Measured values are non-negative integers in the unit their name gives (`_ms`, `_bytes`, `_tokens`, `_usec`). Dimensions are strings. A new optional field may join a `v1` schema. A changed meaning or unit needs a new major version (`.v2`) under a new type.

Every event, from every producer, lands in the stream `_meter` of the `_system` namespace. `loams migrate run` creates it with 32 partitions and 14 days' retention, and records are keyed by the event's `subject` (§63 §7.1).

| Type | What | Producer |
|---|---|---|
| [`io.loams.dev.usage.house.query.v1`](io.loams.dev.usage.house.query.v1.json) | A House query, per worker lease | UsageObserver (House front) |
| [`io.loams.dev.usage.house.insert.v1`](io.loams.dev.usage.house.insert.v1.json) | A House insert | UsageObserver (House front) |
| [`io.loams.dev.usage.house.storage.v1`](io.loams.dev.usage.house.storage.v1.json) | A House table's stored bytes, daily (a gauge, §63 §8.3) | UsageObserver (House storage round) |
| [`io.loams.dev.usage.house.compaction.v1`](io.loams.dev.usage.house.compaction.v1.json) | House compaction work, billed to the namespace (Q692) | UsageObserver (House front) |
| [`io.loams.dev.usage.postgres.compute.v1`](io.loams.dev.usage.postgres.compute.v1.json) | A running Postgres compute, every 60 s and at suspend | ComputeLifecycleObserver plus a ticker (pg-control) |
| [`io.loams.dev.usage.postgres.storage.v1`](io.loams.dev.usage.postgres.storage.v1.json) | A Postgres branch's logical size, hourly (a gauge) | pg-control storage round |
| [`io.loams.dev.usage.gateway.connection.v1`](io.loams.dev.usage.gateway.connection.v1.json) | A gateway connection, on close | ConnectionObserver (gateway pg module) |
| [`io.loams.dev.usage.gateway.request.v1`](io.loams.dev.usage.gateway.request.v1.json) | Gateway HTTP requests (§57 §12.1), batched per namespace | gateway http module |
| [`io.loams.dev.usage.llm.tokens.v1`](io.loams.dev.usage.llm.tokens.v1.json) | An LLM call through the gateway (§57 §12.2) | gateway llm module |
| [`io.loams.dev.usage.mcp.call.v1`](io.loams.dev.usage.mcp.call.v1.json) | An MCP tool call through the gateway (§57 §12.3) | gateway mcp module |
| [`io.loams.dev.usage.function.invocation.v1`](io.loams.dev.usage.function.invocation.v1.json) | A function invocation (D180: CPU time plus requests) | InvocationObserver (runners) |
| [`io.loams.dev.usage.jobs.run.v1`](io.loams.dev.usage.jobs.run.v1.json) | A job run | Jobs engine (JB1 hooks) |
| [`io.loams.dev.usage.stream.ingest.v1`](io.loams.dev.usage.stream.ingest.v1.json) | Stream ingest, per namespace per flush (D103) | Log role |
| [`io.loams.dev.usage.storage.logical.v1`](io.loams.dev.usage.storage.logical.v1.json) | A namespace's logical storage, hourly (a gauge, D103) | Worker role |
| [`io.loams.dev.usage.git.storage.v1`](io.loams.dev.usage.git.storage.v1.json) | A repository's stored bytes (a gauge, D393) | Loams Git |
| [`io.loams.dev.usage.git.transfer.v1`](io.loams.dev.usage.git.transfer.v1.json) | Bytes transferred by Git operations (D393) | Loams Git |
| [`io.loams.dev.usage.sandbox.session.v1`](io.loams.dev.usage.sandbox.session.v1.json) | A sandbox session | Sandbox stats() (D995) |
| [`io.loams.dev.usage.node.cpu.v1`](io.loams.dev.usage.node.cpu.v1.json) | CPU of one tenant resource over one window, read from its cgroup (the node agent) | loams-meter-agent |
| [`io.loams.dev.usage.node.memory.v1`](io.loams.dev.usage.node.memory.v1.json) | Memory of one tenant resource over one window (the node agent) | loams-meter-agent |
| [`io.loams.dev.usage.node.network.v1`](io.loams.dev.usage.node.network.v1.json) | Network of one tenant resource over one window (the node agent) | loams-meter-agent |
| [`io.loams.dev.usage.function.fuel.v1`](io.loams.dev.usage.function.fuel.v1.json) | Wasm fuel of one invocation (the host report, platform doc 06) | wasmtime hosts |

Tenant producers (user meters, §63 §9.6) bring their own types outside `io.loams.dev.*`, and their own schemas if they want them; the profile requires `dataschema` only for Loams types.
