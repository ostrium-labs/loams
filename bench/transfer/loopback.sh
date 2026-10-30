#!/usr/bin/env bash
# DQ1 Task 7 (design 85 §13): upload and download throughput of the desktop
# sidecar over loopback, HTTP/3 against HTTP/2+TLS, with `loams-transfer
# serve` on a local-folder bucket. A smoke measurement for the record, not
# CT1 Task 11's benchmark.
#
# Usage: bench/transfer/loopback.sh [size-GiB] [work-dir]
#   size-GiB  default 4
#   work-dir  default <cargo target>/tmp/dq1-bench (never /tmp: it is RAM)
# Needs `cargo build --release -p loams-transfer` first. Deletes its data at
# the end.
set -euo pipefail
size_gib=${1:-4}
root=$(cd "$(dirname "$0")/../.." && pwd)
target=$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')
bin="$target/release/loams-transfer"
[ -x "$bin" ] || { echo "build it: cargo build --release -p loams-transfer" >&2; exit 1; }
work=${2:-$target/tmp/dq1-bench}
rm -rf "$work"; mkdir -p "$work"
trap 'kill "${srv:-0}" 2>/dev/null || true; rm -rf "$work"' EXIT

head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$work/token"
"$bin" serve --data-dir "$work/srv" --token-file "$work/token" --listen 127.0.0.1:0 > "$work/serve.json" &
srv=$!
for _ in $(seq 100); do [ -s "$work/serve.json" ] && break; sleep 0.1; done
addr=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["addr"])' "$work/serve.json")
pin=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["cert_sha256"])' "$work/serve.json")

# A dense file of pseudo-random bytes (no sparse shortcuts on either side).
python3 - "$work/src.bin" "$size_gib" <<'PY'
import os, sys
path, gib = sys.argv[1], int(sys.argv[2])
block = os.urandom(1 << 20)
with open(path, "wb") as f:
    for i in range(gib << 10):
        f.write(block[i % 7:] + block[: i % 7])
PY

run() { # mode direction
  local mode=$1 dir=$2 start end
  local common=(--server "https://$addr" --pin "$pin" --mode "$mode" --ns bench
    --token-file "$work/token" --state "$work/state-$mode")
  start=$(date +%s.%N)
  if [ "$dir" = up ]; then
    "$bin" put "${common[@]}" "$work/src.bin" | tail -1 > "$work/put-$mode.json"
  else
    tid=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["tid"])' "$work/put-$mode.json")
    "$bin" get "${common[@]}" --tid "$tid" --dest "$work/dst-$mode.bin" | tail -1 > "$work/get-$mode.json"
    cmp "$work/src.bin" "$work/dst-$mode.bin"
    rm -f "$work/dst-$mode.bin"
  fi
  end=$(date +%s.%N)
  python3 - "$mode" "$dir" "$start" "$end" "$size_gib" "$work/$( [ "$dir" = up ] && echo put || echo get )-$mode.json" <<'PY'
import json, sys
mode, d, s, e, gib, f = sys.argv[1:]
secs = float(e) - float(s)
mb = int(gib) * (1 << 30) / 1e6
rss = json.load(open(f)).get("vm_hwm_kib")
print(f"{mode:3} {d:4} {int(gib)} GiB in {secs:6.1f} s = {mb / secs:7.1f} MB/s, peak RSS {rss} KiB")
PY
}
for mode in h3 h2; do run "$mode" up; run "$mode" down; done
