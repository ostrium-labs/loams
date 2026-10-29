# The Rust SDK leg (lane TSDK-rust, D2155-D2158), sourced by run.sh when
# SDK_LEGS names `rust`. The Temporal Rust SDK (pins.env TEMPORAL_SDK_RUST:
# crates temporalio-sdk/-client 1.1.0 on sdk-core TEMPORAL_SDK_CORE_RUST
# 0.10.0, locked in corpus/workers/rust/Cargo.lock), built with cargo in the
# pinned RUST_IMAGE: its unit tests, then the worker and the starter run the
# scenarios against the gateway, the Go exporter writes the histories (as
# for TypeScript and Python), and sdk-core's replayer (the SDK's
# WorkflowReplayer) checks them. The Rust SDK is prerelease (see the
# worker's Cargo.toml). Uses run.sh's leg, go_run, $rt, $addr, ${net[@]},
# $corpus, $out, $SCENARIOS. Not yet run (honest status: CI's first run).
# shellcheck shell=bash

# rust_run <name> <container args...> -- <command in workers/rust>
# The registry and the target directory live in named volumes, so the
# second leg of a run (and the next run) reuses the build.
rust_run() {
  local name=$1
  shift
  local -a args=()
  while [ "$1" != -- ]; do args+=("$1"); shift; done
  shift
  leg "$name" "${args[@]}" -v "$corpus:/c:ro" \
    -v dx1-cargo-registry:/usr/local/cargo/registry -v dx1-rust-target:/target \
    -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" \
    -e CARGO_TERM_COLOR=never -e CARGO_INCREMENTAL=0 -e RUST_BACKTRACE=1 "$RUST_IMAGE" \
    sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/rust && $*"
}

leg_rust_generate() {
  "$rt" volume create dx1-cargo-registry >/dev/null 2>&1 || true
  "$rt" volume create dx1-rust-target >/dev/null 2>&1 || true
  rm -f "$out/corpus-rust"
  rust_run rust-generate "${net[@]}" -v "$out:/out" -- \
    "cargo test --locked --quiet &&
     cargo build --locked &&
     cp /target/debug/corpus-rust /out/corpus-rust &&
     /out/corpus-rust generate $addr default /out/generate-rust.json $SCENARIOS" &&
    go_run rust-export "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-rust- -scenarios $SCENARIOS -out /out/histories/rust"
}

# The replayer never dials the server: the binary the generate leg built,
# without a network.
leg_rust_replay() {
  [ -x "$out/corpus-rust" ] || { echo "legs/rust.sh: no corpus-rust binary (the generate leg did not build it)"; return 1; }
  leg rust-replay --network none -v "$out:/out:ro" -e CORPUS_HISTORIES=/out/histories/rust -e RUST_BACKTRACE=1 \
    "$RUST_IMAGE" /out/corpus-rust replay
}
