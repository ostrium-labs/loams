# Loams Scripts

This directory contains development, verification, benchmarking, and CI scripts for the Loams platform.

## Script Categories

| Directory | Purpose | Primary Scripts |
| --- | --- | --- |
| **[`ci/`](ci/)** | Continuous integration checks | `check-dco.sh`, `auto-promote.py`, `no-metering.sh`, `publishable-crates.py`, `signpath-artifacts.py` |
| **[`sdk/`](sdk/)** | SDK generation and verification | `gen.sh` (code generation), `drift.sh` (API drift check), `check-pins.sh` |
| **[`connectors/`](connectors/)** | Connector tooling & generation | `gen_registry.sh`, `camel_catalog.py`, `kestra_catalog.py`, `matrix.py` |
| **[`loams-pg-bench/`](loams-pg-bench/)** | Postgres benchmark & gating | `run.sh`, `workload.sh`, `gate.sh`, `compare.py`, `stats.py` |
| **[`durable/`](durable/)** | Loams Durable test harness | `conformance.sh`, `tidb.sh`, `porc-503.sh` |
| **[`tikv/`](tikv/)** | TiKV dev cluster management | `playground.sh`, `wait-ready.sh` |
| **[`spec/`](spec/)** | Formal verification checks | `check.sh` (Lean/TLA+ specs), `provenance.sh` |
| **[`release/`](release/)** | Release packaging | `build-artifacts.py`, `install-nfpm.sh`, `make-repo.py` |
| **[`router/`](router/)** | Router inventory & hashing | `gen-pg-hash-vectors.sh` |
| **[`docs/`](docs/)** | Documentation validation | `check-decision-ids.sh` |

## Root Scripts

- **[`sync-labels.sh`](sync-labels.sh)**: Synchronizes GitHub issue and pull request labels against project taxonomy.
- **[`test-cluster-startup-stress.sh`](test-cluster-startup-stress.sh)**: Stress tests multi-node Loams cluster startup and discovery.
