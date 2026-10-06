# Loams Core Crates

This directory contains the root Cargo workspace members for the Loams core engine, retrieval subsystems, metastore, protocols, and reactive services.

## Overview of Crates

| Subsystem | Crates | Description |
| --- | --- | --- |
| **Daemon & CLI** | [`loams`](loams/) | Main server daemon executable and command-line entry point |
| **Retrieval & Storage** | [`loams-store`](loams-store/), [`loams-cache`](loams-cache/), [`loams-hnsw`](loams-hnsw/), [`loams-text`](loams-text/), [`loams-query`](loams-query/), [`loams-hot`](loams-hot/), [`loams-collection`](loams-collection/) | Object storage abstraction, Lance/Tantivy integration, HNSW vector graphs, BM25 indexing, and DataFusion queries |
| **Metastore & Cluster** | [`loams-meta`](loams-meta/), [`loams-meta-tikv`](loams-meta-tikv/), [`loams-meta-conformance`](loams-meta-conformance/), [`loams-tikv`](loams-tikv/) | Raft embedded metastore and distributed TiKV metastore backends |
| **Protocols & Gateways** | [`loams-es`](loams-es/), [`loams-qdrant`](loams-qdrant/), [`loams-sqlrouter`](loams-sqlrouter/), [`loams-stream-grpc`](loams-stream-grpc/), [`loams-proto`](loams-proto/), [`loams-live-proto`](loams-live-proto/) | Drop-in Elasticsearch, Qdrant, SQL wire protocols, and gRPC streaming APIs |
| **Reactive & Execution** | [`loams-live`](loams-live/), [`loams-durable`](loams-durable/), [`loams-worker`](loams-worker/), [`loams-safekeeper`](loams-safekeeper/), [`loams-link`](loams-link/) | Reactive document database on TiKV, embedded Resonate durable workflow server, and link consumers |
| **Common & Helpers** | [`loams-common`](loams-common/), [`loams-compat`](loams-compat/), [`loams-pk`](loams-pk/), [`loams-log`](loams-log/), [`loams-sim`](loams-sim/), [`loams-web-bridge`](loams-web-bridge/), [`loams-facade-gen`](loams-facade-gen/) | Error types, identifiers, simulation harness, web toolbox bridge, and SDK generator |
| **Mock Servers** | [`loams-apps-mock`](loams-apps-mock/), [`loams-console-mock`](loams-console-mock/) | Mock servers providing test backends for mobile and web console development |

## Building and Testing

```sh
# Build the root Cargo workspace
cargo build --workspace

# Run core engine tests
cargo test --workspace

# Run via Nx
pnpm nx run loams-engine:build
pnpm nx run loams-engine:test
```
