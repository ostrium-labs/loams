#!/usr/bin/env bash
# Consume capacity (egress) of CLUSTER: READERS_PER_CKU x CKU Flink readers scan the
# 1 KB topic from the earliest offset and write nothing (a filter no record passes).
#   consume.sh POOL CLUSTER CKU [MINUTES=8]
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state="${CFB_STATE:-$HOME/.cfb-state}"; day="${CFB_DAY:-2026-10-28}"
pool="$1" c="$2" cku="$3" m="${4:-8}"; short="${c#loams-cfb-}"; f="$here/flink.sh"
out="$here/results/$day/dedicated"; id="$(jq -r .id "$state/clusters/$c/cluster.json")"
phase() { printf '%s\t%s\n' "$(date -u +%FT%TZ)" "$1" >>"$out/$c-phases.tsv"; echo "$(date -u +%T) $c phase $1"; }
r=$(( ${READERS_PER_CKU:-2} * cku ))
start="$(date -u -d '-2 min' +%Y-%m-%dT%H:%M:00Z)"
phase "C2-start readers=$r"
for i in $(seq 1 "$r"); do
  "$f" run "$pool" "$c" "cfb-read-$short-c2r$i" "INSERT INTO cfb_counts SELECT \$rowtime, $i, CAST(1 AS BIGINT) FROM cfb_sink /*+ OPTIONS('scan.startup.mode' = 'earliest-offset') */ WHERE val = 'never'" >/dev/null
done
sleep 20; "$f" status "$pool" "cfb-read-$short-c2r1" | cut -c1-200
sleep $(( m * 60 - 20 ))
for i in $(seq 1 "$r"); do "$f" stop "$pool" "cfb-read-$short-c2r$i" >/dev/null; done
phase "C2-stop"
sleep 180
"$here/collect.sh" series "$id" "$start" "$(date -u +%Y-%m-%dT%H:%M:00Z)" PT1M "$out/$c-consume-$(date -u +%H%M).json"
phase "C2-done"
