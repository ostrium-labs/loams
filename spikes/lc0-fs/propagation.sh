#!/usr/bin/env bash
# Drain latency and time-until-B-sees-a-change (same host clock for A and B).
#   For N rounds: B starts polling (its mount, and the API via read-file),
#   A writes a new file in its base mount, optionally drains, and we record
#   when B first sees the new content each way.
#   Then a loaded drain: 20 small files + one 20 MiB file, drain at once.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
OWNER=$LC0_SECRETS/owner.tok
ROUNDS=${ROUNDS:-3}
mkdir -p "$MNT_A/probe"
for mode in nodrain drain; do
  for r in $(seq 1 "$ROUNDS"); do
    f=prop-$mode-$r-$(date +%s).txt
    # B: poll its own mount and the API in parallel; print epoch on first sight.
    tiB_sh "
      ( until grep -q v1 ~/mnt/probe/$f 2>/dev/null; do sleep 0.2; done; echo mount \$(date +%s.%N) ) &
      ( until ti fs read-file --path /probe/$f 2>/dev/null | grep -q v1; do sleep 0.2; done; echo api \$(date +%s.%N) ) &
      wait" "$OWNER" > "$LC0_ROOT/prop.$f.out" &
    bpid=$!
    sleep 3
    t0=$(now)
    echo v1 > "$MNT_A/probe/$f"
    if [ "$mode" = drain ]; then
      tiA fs drain-file-system --mount-path "$MNT_A" >/dev/null; td=$(now)
      echo -n "drain_secs=$(elapsed "$t0" "$td") "
    fi
    if ! timeout 180 tail --pid=$bpid -f /dev/null; then echo "timeout"; kill $bpid; fi
    m=$(awk '/^mount/{print $2}' "$LC0_ROOT/prop.$f.out"); a=$(awk '/^api/{print $2}' "$LC0_ROOT/prop.$f.out")
    echo "$mode round $r: B mount sees after $(elapsed "$t0" "$m")s, API after $(elapsed "$t0" "$a")s"
  done
done
echo "== loaded drain"
mkdir -p "$MNT_A/probe/load"
t0=$(now)
for i in $(seq 1 20); do echo "load $i $(date +%s.%N)" > "$MNT_A/probe/load/f$i.txt"; done
head -c $((20*1024*1024)) /dev/urandom > "$MNT_A/probe/load/big.bin"
t1=$(now)
tiA fs drain-file-system --mount-path "$MNT_A" --timeout 300s >/dev/null
t2=$(now)
echo "write_secs=$(elapsed "$t0" "$t1") drain_after_load_secs=$(elapsed "$t1" "$t2")"
