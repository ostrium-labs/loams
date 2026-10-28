#!/usr/bin/env bash
# The BQ1-kv runs, by phase, each phase short enough for one bench-lock hold
# (≤ 15 minutes). Run as:
#   .he0-shared/benchslot.sh bq1-kv bench/kv/suite.sh <phase>
#
# Cores (Intel Core Ultra 5 225H: 0-3 P-cores 4.9 GHz, 4-11 E-cores 4.4 GHz,
# 12-13 low-power E-cores 2.5 GHz; no SMT):
# - single node: gateway on 0-5 (6 cores), load generator on 6-11 (6 cores);
# - scaling: shard i on E-core 4+i (one core per shard), load generator on 0-3,8-11;
# - TiKV: gateway(s) on 0-3, PD + TiKV pinned to 4-9 (by the caller), load
#   generator on 10-13.
set -euo pipefail
H=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
R=$H/run.sh
phase=$1
D=${D:-15}
common=(--duration "$D" --warmup 3 --keys 100000 --value-size 64)

case $phase in
  single) # A: one gateway, 6 cores, memory and embedded stores
    for store in ${STORES:-memory embedded}; do
      $R "single-$store-ping-p1" "$store" 1 0-5 6-11 "${common[@]}" --conns 256 --pipeline 1 --threads 6 --ping
      $R "single-$store-get-p1" "$store" 1 0-5 6-11 "${common[@]}" --conns 256 --pipeline 1 --threads 6 --prefill
      $R "single-$store-get-p16" "$store" 1 0-5 6-11 "${common[@]}" --conns 256 --pipeline 16 --threads 6 --prefill
      $R "single-$store-set-p1" "$store" 1 0-5 6-11 "${common[@]}" --conns 256 --pipeline 1 --threads 6 --set-ratio 1
      $R "single-$store-mix90-p1" "$store" 1 0-5 6-11 "${common[@]}" --conns 256 --pipeline 1 --threads 6 --set-ratio 0.1 --prefill
    done
    ;;
  scale) # B: 1, 2, 4 shards, one E-core each, 90% GET / 10% SET
    for store in ${STORES:-memory embedded}; do
      $R "scale-$store-1" "$store" 1 4 0-3,8-11 "${common[@]}" --conns 64 --pipeline 1 --threads 8 --set-ratio 0.1 --prefill
      $R "scale-$store-2" "$store" 2 4/5 0-3,8-11 "${common[@]}" --conns 128 --pipeline 1 --threads 8 --set-ratio 0.1 --prefill
      $R "scale-$store-4" "$store" 4 4/5/6/7 0-3,8-11 "${common[@]}" --conns 256 --pipeline 1 --threads 8 --set-ratio 0.1 --prefill
    done
    ;;
  tikv) # C: one gateway on 0-3 over TiKV (PD + TiKV pinned to 4-9 by the caller)
    $R tikv-ping-p1 tikv 1 0-3 10-13 "${common[@]}" --conns 128 --pipeline 1 --threads 4 --ping
    $R tikv-get-p1 tikv 1 0-3 10-13 "${common[@]}" --conns 128 --pipeline 1 --threads 4 --prefill
    $R tikv-get-p16 tikv 1 0-3 10-13 "${common[@]}" --conns 128 --pipeline 16 --threads 4 --prefill
    $R tikv-set-p1 tikv 1 0-3 10-13 "${common[@]}" --conns 128 --pipeline 1 --threads 4 --set-ratio 1
    $R tikv-mix90-p1 tikv 1 0-3 10-13 "${common[@]}" --conns 128 --pipeline 1 --threads 4 --set-ratio 0.1 --prefill
    ;;
  tikv-scale) # D: 1, 2, 4 gateways (one P-core each) over the same TiKV
    $R tikv-scale-1 tikv 1 0 10-13 "${common[@]}" --conns 64 --pipeline 1 --threads 4 --set-ratio 0.1 --prefill
    $R tikv-scale-2 tikv 2 0/1 10-13 "${common[@]}" --conns 128 --pipeline 1 --threads 4 --set-ratio 0.1 --prefill
    $R tikv-scale-4 tikv 4 0/1/2/3 10-13 "${common[@]}" --conns 256 --pipeline 1 --threads 4 --set-ratio 0.1 --prefill
    ;;
  *) echo "phases: single scale tikv tikv-scale" >&2; exit 2 ;;
esac
