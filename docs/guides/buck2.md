# Building the Rust workspaces with Buck2

Buck2 builds, lints and tests Loams' Rust code (owner, 2026-10-24). Cargo stays the source of
truth: `Cargo.toml` and `Cargo.lock` decide every crate, version and feature, and the Buck2 files
are generated from them. The JavaScript side stays on Nx.

**Status (2026-10-24): pilot.** Ten crates of the root workspace build, pass clippy and run their
tests under Buck2 with the same counts as Cargo. The rest of the root workspace and `fabric/` are
next; CI still runs Cargo.

## Setup

```sh
scripts/buck/bootstrap.sh          # buck2 + reindeer into .tools/bin (once per machine)
export PATH="$PWD/.tools/bin:$PATH"
```

The pins are in `scripts/buck/versions.env`: an upstream Buck2 release (checksummed) with the
prelude bundled in it, and reindeer at a fixed commit, built with `cargo install --locked`. The
canonical copy is `pins/buck2.env` in [ostrium-labs/loams-build](https://github.com/ostrium-labs/loams-build)
(branch `loams/main`).

Buck2 writes to `buck-out/` at the repository root, and it must be a real directory: Buck2 links
sources into it with relative symlinks, which break if `buck-out` is a symlink to another disk. To
keep build output off a small disk, put the checkout or worktree on the large one.

## Everyday commands

| Cargo | Buck2 |
|---|---|
| `cargo build -p loams-common` | `buck2 build //crates/loams-common:loams-common` |
| every target of a crate | `buck2 build //crates/loams-common:` |
| `cargo clippy -p loams-common --all-targets -- -D warnings` | `scripts/buck/clippy.sh //crates/loams-common:` |
| `cargo test -p loams-common` | `scripts/buck/test.sh //crates/loams-common:` (or `buck2 test`) |
| `cargo run -p loams-upgrade --bin loams-upgrade` | `buck2 run //crates/loams-upgrade:loams-upgrade-bin-loams-upgrade` |
| the whole workspace | `//crates/...`, or `//...` |

Each crate's `BUCK` file is one `cargo_package` call (`buck/rust.bzl`), which makes:

| Target | What |
|---|---|
| `:<package>` | the library |
| `:<package>-bin-<bin>` | each binary |
| `:<package>-unittest`, `:<package>-bin-<bin>-unittest` | the `#[cfg(test)]` tests of the library and of each binary |
| `:<package>-test-<name>` | each integration test in `tests/` |
| `:<package>-example-<name>`, `:<package>-bench-<name>` | examples and benches (built, not run) |
| `:<package>-clippy` | clippy's output for every target of the crate |
| `:<package>-build-script-run` | the crate's `build.rs`, if it has one |

Limit parallelism on a small machine with `-j 4`.

### Tests

`buck2 test` runs each test binary as one test, so its summary counts binaries.
`scripts/buck/test.sh` adds up libtest's own `test result:` lines, which is what Cargo reports. Test
binaries run from the repository root. `CARGO_MANIFEST_DIR` is the crate's path relative to the
root (both at compile time and at run time), so `env!("CARGO_MANIFEST_DIR")` joined with
`../..` or `tests/fixtures` reaches the checkout, and a binary built on another machine (a remote
cache hit) still points at the right files. Integration tests get `CARGO_BIN_EXE_<bin>` and
`CARGO_TARGET_TMPDIR` the same way.

### Clippy

`buck2 build ':<package>-clippy'` keeps clippy's diagnostics as files instead of failing, so a
cached run still reports them. `scripts/buck/clippy.sh` prints every non-empty one and exits 1,
like `-D warnings`. Lints come from each crate's `[lints]` (or the workspace's), and `clippy.toml`
from the crate or the workspace, as Cargo picks them.

## After changing Cargo.toml or Cargo.lock

```sh
scripts/buck/gen.py
```

The first run installs the pinned buck2 and reindeer (`scripts/buck/bootstrap.sh`: a download and
a one-off `cargo install` of a few minutes); later runs take about a minute. It runs `cargo metadata` on each workspace, writes `third-party/rust/Cargo.toml` (every crates.io
and git package the crates use directly, at its locked version with the features Cargo resolves),
runs reindeer to turn it into `third-party/rust/BUCK`, and rewrites every crate's `BUCK`. Commit
the result. Adding a crate, a dependency or a feature, or bumping a version, changes generated
files.

CI's **Buck2: generated files** job (required) runs `scripts/buck/gen.py --check` and fails if any
generated file is stale. Its log says so, and the job uploads the fix as the `buck-gen.patch`
artifact: download it and `git apply buck-gen.patch` if you would rather not run `gen.py` locally.

A dependency with a build script needs a reindeer fixup in
`third-party/rust/fixups/<crate>/fixups.toml`. When a new one appears, `gen.py` writes the default
(run `build.rs` as Cargo does, keep what it links) and says so; that covers almost every crate,
including those that compile C with the `cc` crate. Edit it only if the crate then fails to build:

```toml
[buildscript.run]
rustc_link_lib = true
rustc_link_search = true
```

What `Cargo.toml` cannot say about a Loams crate (files outside it that `build.rs` reads, test
labels, test environment) goes in `scripts/buck/crates.toml`.

## How it fits together

| Path | Role |
|---|---|
| `.buckconfig`, `.buckroot` | cells (`root`, `prelude` bundled with buck2, `toolchains`) |
| `toolchains/BUCK`, `toolchains/rust.bzl` | the Rust toolchain: rustup's tools at the `rust-toolchain.toml` channel, through `toolchains/rust/wrap.sh`; clang and lld for linking |
| `buck/rust.bzl` | `cargo_package`: one Cargo package to Buck2 targets |
| `scripts/buck/gen.py` | writes everything generated |
| `third-party/rust/` | reindeer's input (`Cargo.toml`, `Cargo.lock`, `reindeer.toml`), output (`BUCK`) and fixups |
| `scripts/buck/clippy.sh`, `scripts/buck/test.sh` | clippy and test wrappers with Cargo's semantics |
