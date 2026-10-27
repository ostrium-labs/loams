#!/usr/bin/env bash
# Generate the Rust SDK's histories (lane TSDK-rust, D2157) against upstream
# Temporal on the stack's network, into histories/rust/, and record them in
# manifest.json (the Rust entries only; the other languages' are kept).
#
#   generate-rust.sh [--out DIR]   # default: this directory's histories/ (the committed corpus)
#
# The oracle is the same as generate.sh's: CORPUS_SERVER=compat-mode (the
# stock server on Loams Postgres; `run.sh` or `oc_up temporal`), or
# start-dev (the CLI's dev server with the alias temporal-frontend; see
# README.md "Regenerating"). TEMPORAL_ADDRESS defaults to
# temporal-frontend:7233. Each generation uses a fresh namespace, which the
# Rust generator registers itself (CORPUS_REGISTER=1, with the keyword
# attribute CorpusKeyword), so it does not need generate.sh to run first.
#
# The worker is built with cargo inside RUST_IMAGE (pins.env), never on the
# host; the Go exporter writes <scenario>[.run<k>].{json,binpb}.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../../lib.sh"

out=$here/histories
while [ $# -gt 0 ]; do
  case $1 in
    --out) out=${2:?--out DIR}; shift 2 ;;
    *) echo "usage: generate-rust.sh [--out DIR]" >&2; exit 2 ;;
  esac
done
: "${TEMPORAL_ADDRESS:=temporal-frontend:7233}" "${CORPUS_SERVER:=compat-mode}" "${COMPUTE_PG_MAJOR:=16}"
rt=$(oc_container_cmd)
ns=corpus-rust-$(date +%s)
mkdir -p "$out/rust"
rm -f "$out"/rust/*.json "$out"/rust/*.binpb "$out/generate-rust.json"
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true
"$rt" volume create dx0-cargo-registry >/dev/null 2>&1 || true
"$rt" volume create dx0-rust-target >/dev/null 2>&1 || true

# The Rust worker and starters: build, unit tests, generate.
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c:ro" -v "$out:/out" \
  -v dx0-cargo-registry:/usr/local/cargo/registry -v dx0-rust-target:/target \
  -e CARGO_TARGET_DIR=/target -e CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" -e CARGO_TERM_COLOR=never \
  -e CARGO_INCREMENTAL=0 -e CORPUS_REGISTER=1 "$RUST_IMAGE" \
  sh -ec "cp -r /c /tmp/c && cd /tmp/c/workers/rust && cargo test --locked --quiet && cargo build --locked &&
    /target/debug/corpus-rust generate $TEMPORAL_ADDRESS $ns /out/generate-rust.json"
list=$(python3 -c "import json,sys; print(','.join(s['scenario'] for s in json.load(open(sys.argv[1]))['scenarios']))" "$out/generate-rust.json")
# The Go exporter, as for TypeScript and Python.
"$rt" run --rm --network "$OC_NETWORK" -v "$here:/c" -v "$out:/out" -v dx0-gocache:/go/pkg \
  -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 "$GO_IMAGE" \
  sh -ec "cd /c/workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $TEMPORAL_ADDRESS -namespace $ns -prefix corpus-rust- -scenarios $list -out /out/rust"

if [ "$out" = "$here/histories" ]; then
  case $CORPUS_SERVER in
    compat-mode) version=$TEMPORAL_VERSION image=$TEMPORAL_SERVER_IMAGE pg=$COMPUTE_PG_MAJOR ;;
    start-dev) version=1.32.0 image="temporal server start-dev (CLI $TEMPORAL_CLI in $TEMPORAL_ADMIN_TOOLS_IMAGE)" pg=start-dev ;;
    *) echo "generate-rust.sh: CORPUS_SERVER is compat-mode or start-dev" >&2; exit 2 ;;
  esac
  python3 "$here/manifest.py" build-lang rust --server-version "$version" --server-image "$image" --pg-major "$pg"
fi
echo "generate-rust.sh: namespace $ns, $(find "$out/rust" -name "*.json" | wc -l) Rust histories in $out/rust"
