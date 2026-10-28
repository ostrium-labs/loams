# Project layout

## Crates

| Crate | What it does |
|---|---|
| [`loams`](../crates/loams) | The server binary: the native HTTP API, Flight SQL, the gateways, and the `dev`, `standalone` and `cluster` commands |
| [`loams-common`](../crates/loams-common) | Identifier and schema types shared by all crates |
| [`loams-store`](../crates/loams-store) | Object-storage access: conditional writes, range reads and fault injection |
| [`loams-cache`](../crates/loams-cache) | Read-through RAM + NVMe byte-range cache over immutable objects |
| [`loams-meta`](../crates/loams-meta) | The embedded metastore on Raft: namespaces, streams, the sequencer, leases and pointers |
| [`loams-meta-tikv`](../crates/loams-meta-tikv) | The metastore on TiKV |
| [`loams-meta-conformance`](../crates/loams-meta-conformance) | A backend-agnostic conformance suite for the metastore (test only) |
| [`loams-tikv`](../crates/loams-tikv) | The TiKV client layer: transactions, the timestamp oracle, keyspace bootstrap and the cluster GC loop |
| [`loams-tuple`](../crates/loams-tuple) | The order-preserving tuple codec of index keys, shared by `loams-kv` and `loams-tikv` |
| [`loams-log`](../crates/loams-log) | The internal log: WAL objects, segments, the write and fetch paths, and retention |
| [`loams-worker`](../crates/loams-worker) | Lease-fenced background tasks with priorities and fair share |
| [`loams-link`](../crates/loams-link) | Links: exactly-once apply of streams into targets |
| [`loams-pk`](../crates/loams-pk) | The primary-key index on SlateDB |
| [`loams-collection`](../crates/loams-collection) | Collections: documents, the catalog, and Lance + Tantivy storage under one manifest |
| [`loams-text`](../crates/loams-text) | Tantivy integration: analyzers, splits on object storage and delete bitmaps |
| [`loams-quickwit`](../crates/loams-quickwit) | Quickwit's split, directory, query and merge-policy code, vendored and adapted |
| [`loams-query`](../crates/loams-query) | The read side: the search IR, hybrid query planning, SQL and the tail |
| [`loams-hnsw`](../crates/loams-hnsw) | HNSW index traits, an exact flat engine and the qdrant-edge engine |
| [`loams-hot`](../crates/loams-hot) | The hot tier: HNSW artifacts, pinned splits, budgets, placement and read forwarding |
| [`loams-qdrant`](../crates/loams-qdrant) | The Qdrant-compatible REST and gRPC gateway |
| [`loams-es`](../crates/loams-es) | The Elasticsearch-compatible REST gateway |
| [`loams-durable`](../crates/loams-durable) | Loams Durable: the Resonate server embedded in process |
| [`loams-kv`](../crates/loams-kv) | Loams Live's store seam: transactions, snapshots and timestamps over an embedded backend or TiKV |
| [`loams-live`](../crates/loams-live) | Loams Live: the reactive document database on TiKV |
| [`loams-live-proto`](../crates/loams-live-proto) | Loams Live's `loams.live.v1` protos and generated service code |
| [`loams-sim`](../crates/loams-sim) | Seeded cluster simulation and a linearizability checker (test only) |
| [`loams-console-mock`](../crates/loams-console-mock) | The console API's contract and seed data, served over REST by `loams-apps-mock` |

## Other directories

| Path | Contents |
|---|---|
| [`docs/design`](design/README.md) | The design documents and the decision log |
| [`docs/plans`](plans/README.md) | Implementation plans, task by task |
| [`web`](../web/README.md) | The web console and the `@loams/ui` design system |
| [`sdks`](../sdks) | Client SDKs: TypeScript, Python, Go, Rust, C#, C++ and Swift supported; more coming |
| [`proto`](../proto), [`api`](../api) | Protobuf and OpenAPI contracts |
| [`fabric`](../fabric) | The `fabric/` workspace: Loams Flow's event fabric and its connectors (design §32, §33) |
| [`connectors`](../connectors) | The connector registry: the 200-row catalog, the manifests and their JSON Schemas (§33) |
| [`conformance`](../conformance) | External client conformance suites |
| [`deploy`](../deploy) | Local compose files for TiKV and companion services |
| [`scripts`](../scripts) | Development and CI helper scripts |

## Apps in the monorepo

This repository also contains [Loams Desktop](../apps/desktop-electron/README.md) with its agent daemon ([`crates/loams-agentd`](design/50-loams-desktop-daemon.md)),
[native Android/iOS apps](../apps/mobile/README.md), and [plugin services and dashboard](../plugins/README.md).
Nx coordinates Cargo, Gradle, Xcode/SwiftPM, Go, and the root pnpm workspace; it does not replace those build tools.

```sh
pnpm install --frozen-lockfile
pnpm projects
pnpm build
pnpm test
```

The default JavaScript checks do not require native toolchains. Build native apps separately with
`pnpm build:desktop`, `pnpm build:android`, or `pnpm build:ios` on a supported host.
See [the monorepo guide](monorepo.md) for prerequisites, commands, migration provenance,
contract ownership, and current validation limits. Scoped licenses remain beside imported components.
