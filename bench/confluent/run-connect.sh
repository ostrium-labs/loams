#!/usr/bin/env bash
# Loams' own path against Confluent Cloud: a Kafka topic on SRC -> `loams-fabric
# connect` source -> a Loams stream (local engine on this laptop) -> the
# exactly-once sink -> a topic on DST (another cloud), with a kill -9 of connect
# halfway (CFB_KILL=1) and a clean run (CFB_KILL=0).
#
#   run-connect.sh SRC DST [N=2000] [RATE=100]
#
# Binaries: LOAMS_BIN / FABRIC_BIN (default: the cfb-connect worktree's release builds
# with the connect chunking fix, PR #1496). Results: results/$CFB_DAY/connect/.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
state="${CFB_STATE:-$HOME/.cfb-state}"
day="${CFB_DAY:-2026-10-28}"
src="$1" dst="$2" n="${3:-2000}" rate="${4:-100}"
S="$state/clusters/$src" D="$state/clusters/$dst"
export LOAMS_BIN="${LOAMS_BIN:-$HOME/Documents/Ostriumlabs/loams-wt/cfb-connect/bin/loams}"
export FABRIC_BIN="${FABRIC_BIN:-$HOME/Documents/Ostriumlabs/loams-wt/cfb-connect/bin/loams-fabric}"
export KAFKA_BOOTSTRAP="$(jq -r .bootstrap "$S/cluster.json")" KAFKA_KEY_FILE="$S/key"
export SINK_KAFKA_BOOTSTRAP="$(jq -r .bootstrap "$D/cluster.json")" SINK_KAFKA_KEY_FILE="$D/key"
export CFB_TOPIC_IN="${CFB_TOPIC_IN:-loams-cfb-connect-in}" CFB_TOPIC_OUT="${CFB_TOPIC_OUT:-loams-cfb-connect-out}"
export CFB_N="$n" CFB_RATE="$rate" CFB_TIMEOUT_S="${CFB_TIMEOUT_S:-600}"
out="$here/results/$day/connect"; mkdir -p "$out"
cd "$here/loams-connect"
./up.sh
for kill in 0 1; do
  f="$out/${src#loams-cfb-}-to-${dst#loams-cfb-}-kill$kill"
  CFB_KILL=$kill ./e2e.sh >"$f.log" 2>&1 || true
  latest="$(ls -td .run/e2e/*/ | head -1)"
  cp "$latest/summary.json" "$f.json" 2>/dev/null && jq -c '.' "$f.json" | cut -c1-800 || tail -n 5 "$f.log"
done
./down.sh --purge
