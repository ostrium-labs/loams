# shellcheck shell=bash
# Shared argument handling of BQ1-ALL's first-version native-load scripts
# (CONTRACT.md). Each engine script sets ENGINE, DEFAULT_TARGET, CONNS and a
# `build_args` function, sources this file, and calls `native_main "$@"`.
# They drive an engine that bench/all/engines.sh started (--target); none
# starts its own server, so --server-cores is refused.
set -euo pipefail

BQ1_ALL_BIN=${BQ1_ALL_BIN:-/mnt/Projects/bench-data/bq1-all/bin}
NATIVE=${BQ1_NATIVE:-$BQ1_ALL_BIN/bq1-native}

count_cores() {
  local n=0 part a b
  IFS=, read -r -a parts <<< "$1"
  for part in "${parts[@]}"; do
    a=${part%-*}; b=${part#*-}
    n=$(( n + b - a + 1 ))
  done
  echo "$n"
}

native_main() {
  duration=30 cores=0 depth=1 target=$DEFAULT_TARGET start_at="" ops=""
  while [ $# -gt 0 ]; do
    case $1 in
      --duration) duration=${2%s}; shift 2 ;;
      --cores) cores=$2; shift 2 ;;
      --depth) depth=$2; shift 2 ;;
      --target) target=$2; shift 2 ;;
      --start-at) start_at=$2; shift 2 ;;
      --ops) ops=$2; shift 2 ;;
      --conns) CONNS=$2; shift 2 ;;
      --server-cores) echo "$ENGINE: this script drives a running engine (engines.sh); use --target" >&2; exit 2 ;;
      *) echo "$ENGINE: unknown argument $1" >&2; exit 2 ;;
    esac
  done
  [ -x "$NATIVE" ] || { echo "$ENGINE: no $NATIVE (cargo build --release -p bq1-native)" >&2; exit 1; }
  local n; n=$(count_cores "$cores")
  local -a common=(--engine "$ENGINE" --duration "$duration" --depth "$depth" --conns "$CONNS"
    --threads "$n" --cores "$n")
  [ -n "$start_at" ] && common+=(--start-at "$start_at")
  [ -n "$ops" ] && common+=(--ops "$ops")
  build_args "$target"
  taskset -c "$cores" "$NATIVE" "${ARGS[@]}" "${common[@]}" | rename_ops
}

# Engine scripts may override this to rename ops in the output.
declare -F rename_ops > /dev/null || rename_ops() { cat; }
