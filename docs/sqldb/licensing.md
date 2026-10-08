# Loams SQL licensing

Loams SQL (design §47, plan SQ1) serves MySQL from TiDB compute over the Loams TiKV. This page records what is linked, what is only run, and how that is checked (D11, D126).

## Run as images, never linked

These components are Go (TiDB, PD, BR, TiCDC) or Rust built by PingCAP (TiKV). Loams pulls them as container images pinned by digest in [`release/sqldb-images.toml`](../../release/sqldb-images.toml). No Loams binary links them.

| Image | Version | Licence | Used for |
|---|---|---|---|
| `pingcap/tidb` | v8.5.8 | Apache-2.0 | The compute pool of each branch (`tidb-server`, keyspace mode) |
| `pingcap/tikv` | v8.5.8 | Apache-2.0 | The spike and IT stack. Production runs the Loams TiKV |
| `pingcap/pd` | v8.5.8 | Apache-2.0 | The spike and IT stack, plus the keyspace API |
| `pingcap/br` | v8.5.8 | Apache-2.0 | Backup, log backup and restore jobs (SQ1c) |
| `pingcap/ticdc` | v8.5.8 | Apache-2.0 | Pinned only. Classic TiCDC cannot read keyspaces (§47 §10), so CDC is the Rust `loams-sqlcdc` |

- **How the images reach a host.** The runtimes (`LocalRuntime`, `KubernetesRuntime`) pull `<image>@<digest>` from Docker Hub. Loams does not redistribute these images.
- **If an image is ever mirrored or bundled** (for example, an offline desktop installer), its Apache-2.0 `LICENSE` and `NOTICE` files ship with it.
- **Patches.** Any TiDB patch (§47 §5.4) stays in the `ostrium-labs/tidb` fork under Apache-2.0, and its image is pinned the same way.

## Linked Rust code

`crates/loams-sqldb` and the crates that follow it (`loams-sqlgate`, `loams-sqlcdc`) depend only on crates the workspace [`deny.toml`](../../deny.toml) allows: permissive licences only, with no GPL, LGPL, AGPL, SSPL, BUSL or ELv2.

- **The check.** `no_gpl_crate_in_dependency_graph` (`crates/loams-sqldb/tests/it/licences.rs`) runs this command, scoped to the crate's own graph (dev-dependencies included):

  ```sh
  cargo deny --manifest-path crates/loams-sqldb/Cargo.toml --config deny.toml check licenses
  ```

- **Where cargo-deny is missing.** The test skips with a message. CI sets `LOAMS_REQUIRE_CARGO_DENY=1`, which turns the skip into a failure.
- **New crates.** Each new SQ1 crate adds the same test.
- **Test-only crates.** `mysql_async`, which the tests use, is MIT OR Apache-2.0. It is built with `minimal-rust`, so it pulls in no native TLS or C library.

## GPL tools

- **sysbench and mydumper** are GPL. They run only as separate tools or images in benchmarks (`scripts/sqldb/bench`) and conformance runs.
- They are never linked into Loams, never bundled with it, and never needed at runtime.
- `mysql:8.4`, the reference engine in conformance (§47 §13.2), is used the same way: it is a test image only.
