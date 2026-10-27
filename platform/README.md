# platform

The `platform/` Cargo workspace: code from the former private `ostrium-labs/loams-platform` repository, imported into the monorepo as a snapshot on 2026-10-19 and open source under Apache-2.0 like the rest of Loams. Its designs, plans and runbooks are in [`docs/platform`](../docs/platform/README.md); its tailnet deploy files are in [`deploy/tailnet`](../deploy/tailnet).

| Crate | What it does |
|---|---|
| [`loams-meter`](crates/loams-meter) | The node metering agent (design [01](../docs/platform/design/01-metering-and-billing.md), track MB0): reads cgroup v2 counters from outside the tenant sandboxes, turns them into per-window deltas, resolves the tenant, writes CloudEvents meter events to a durable on-disk spool and forwards them to the Loams stream `_system/_meter`. Every event carries the idempotency key `<node>/<source>/<seq>` |

It is a workspace of its own, as [`fabric/`](../fabric) is, so that its dependency set (reqwest 0.12, base64 0.22) stays out of the engine's lockfile. Design 63 (Loams Meter, [D1135](../docs/design/13-decision-log.md)) keeps the agent, renamed `loams-meter-agent`, and moves it into the root workspace, after which this workspace and its CI job go ([ME1](../docs/plans/2026-10-19-me1-loams-meter.md) Task 0b). The engine does not depend on it.

## Development

The toolchain, `rustfmt.toml`, `clippy.toml` and lints match the root workspace. Builds use the shared target directory from the user-level Cargo config; do not set `CARGO_TARGET_DIR`.

```bash
cd platform
cargo test --workspace --locked -j 4
cargo clippy --workspace --all-targets --locked -j 4 -- -D warnings
cargo fmt --all --check
cargo deny check
```

CI runs the same in [`.github/workflows/platform.yml`](../.github/workflows/platform.yml), together with the tailnet policy lint and Headscale's policy check.

## License

Apache-2.0; see the repository's [LICENSE](../LICENSE) and [NOTICE](../NOTICE).
