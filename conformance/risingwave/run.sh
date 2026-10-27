#!/usr/bin/env bash
# RisingWave conformance against a managed instance (design 81, plan RW1, lane RW-b).
#
# Runs (1) Loams' own cases under loams/ and (2) the subset of RisingWave's own
# e2e sqllogictest files listed in subset.txt, read from the source tarball of
# the pinned tag, with sqllogictest-rs. Results are JUnit, checked strictly
# against xfail.toml by scripts/compute/xfail_check.py.
#
# Status: written, not yet run (owner rule 2026-10-27: no local runs; the
# managed profile is not up yet). Nothing here starts RisingWave: point it at an
# instance.
#
#   RW_HOST=127.0.0.1 RW_PORT=4566 RW_USER=loams RW_PASSWORD=<Loams API key> \
#     conformance/risingwave/run.sh [--loams-only] [--work-dir DIR]
#
# Environment:
#   RW_HOST, RW_PORT, RW_DB, RW_USER   the frontend (default: engine-gateway on 127.0.0.1:4566, db dev)
#   RW_PASSWORD                         a Loams API key (engine-gateway checks it, design 81 R8)
#   RW_CONFORMANCE_RECORD_PIN=1         print the tarball digest when pins.env has none, and continue
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
# shellcheck source=/dev/null
. "$here/pins.env"

loams_only=0
work="${RW_CONFORMANCE_WORK:-$repo/target/conformance-risingwave}"
while [ $# -gt 0 ]; do
  case "$1" in
    --loams-only) loams_only=1 ;;
    --work-dir) work="$2"; shift ;;
    *) echo "run.sh: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
case "$work" in
  /tmp/*) echo "run.sh: never under /tmp (a small RAM tmpfs); pick another --work-dir" >&2; exit 2 ;;
esac

host="${RW_HOST:-127.0.0.1}"
port="${RW_PORT:-4566}"
db="${RW_DB:-dev}"
user="${RW_USER:-loams}"
password="${RW_PASSWORD:-${LOAMS_API_KEY:-}}"

mkdir -p "$work/bin" "$work/junit"

# 1. The runner, pinned, in the harness's own root.
slt="$work/bin/bin/sqllogictest"
if [ ! -x "$slt" ] || ! "$slt" --version 2>/dev/null | grep -q "$SQLLOGICTEST_BIN"; then
  cargo install --locked --quiet --root "$work/bin" sqllogictest-bin --version "=$SQLLOGICTEST_BIN"
fi

run_slt() { # <junit-name> <files...>
  local name="$1"; shift
  (
    cd "$work/junit"
    PGPASSWORD="$password" "$slt" \
      --engine postgres \
      --host "$host" --port "$port" --db "$db" --user "$user" --pass "$password" \
      --junit "$name" \
      "$@"
  ) || true # failures are judged by xfail_check below, not by the exit code
}

# 2. Loams' own cases.
run_slt loams "$here"/loams/*.slt

# 3. RisingWave's e2e subset from the pinned tag's tarball (a download, not a clone).
if [ "$loams_only" = 0 ]; then
  src="$work/src-$RISINGWAVE_TAG"
  if [ ! -d "$src/e2e_test" ]; then
    tarball="$work/risingwave-$RISINGWAVE_TAG.tar.gz"
    curl -fsSL -o "$tarball" \
      "https://codeload.github.com/risingwavelabs/risingwave/tar.gz/refs/tags/$RISINGWAVE_TAG"
    digest="$(sha256sum "$tarball" | cut -d' ' -f1)"
    if [ -z "$RISINGWAVE_SRC_SHA256" ]; then
      if [ "${RW_CONFORMANCE_RECORD_PIN:-0}" != 1 ]; then
        echo "run.sh: pins.env has no RISINGWAVE_SRC_SHA256; rerun with RW_CONFORMANCE_RECORD_PIN=1 to record $digest" >&2
        exit 2
      fi
      echo "run.sh: record RISINGWAVE_SRC_SHA256=$digest in pins.env" >&2
    elif [ "$digest" != "$RISINGWAVE_SRC_SHA256" ]; then
      echo "run.sh: tarball digest $digest does not match pins.env" >&2
      exit 2
    fi
    mkdir -p "$src"
    tar -xzf "$tarball" -C "$src" --strip-components=1 --wildcards '*/e2e_test/*'
    rm -f "$tarball"
  fi
  files=()
  while IFS= read -r pattern; do
    case "$pattern" in ''|'#'*) continue ;; esac
    shopt -s nullglob
    # shellcheck disable=SC2206
    matched=( "$src"/e2e_test/$pattern )
    shopt -u nullglob
    if [ "${#matched[@]}" -eq 0 ]; then
      echo "run.sh: subset.txt pattern '$pattern' matches no file at $RISINGWAVE_TAG" >&2
      exit 2
    fi
    files+=( "${matched[@]}" )
  done < "$here/subset.txt"
  run_slt upstream "${files[@]}"
fi

# 4. Strict expected-failure check.
python3 "$repo/scripts/compute/xfail_check.py" "$work"/junit/*.xml --xfail "$here/xfail.toml"
