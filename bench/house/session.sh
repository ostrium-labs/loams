#!/usr/bin/env bash
# One measured session on a fresh single-node House: start, load (timed), then
# `read` (read.sh), `insert` (insert.sh) or `both`; stop; summarise to JSON.
# Usage: benchslot.sh bq1-house bench/house/session.sh <tag> <read|insert|both> [workers]
source "$(dirname "$0")/lib.sh"
tag=$1; what=$2; workers=${3:-}
run="$DATA/runs/$tag"
rm -rf "$DATA/main" "$run"
mkdir -p "$run"
pid=$(start_house main "$SERVER_CORES" 28123 29000 28125 "$workers")
trap 'stop_house "$pid"' EXIT
"$HERE/load.sh" main 28123 | tee "$run/load.txt"
case $what in
    read | both) "$HERE/read.sh" main 28123 29000 "$run" "${SECS:-10}" "${CONCS:-1 8 32 64}" ;;
esac
case $what in
    batch) "$HERE/inserts-batch.sh" main 28123 "$run" "${INS_SECS:-10}" "${INS_CONC:-4}" ;;
esac
case $what in
    insert | both) "$HERE/insert.sh" main 28123 29000 "$run" "${INS_SECS:-15}" "${INS_CONCS:-1 4 8}" ;;
esac
stop_house "$pid"
trap - EXIT
cp "$DATA/main/house.log" "$run/house.log"
python3 "$HERE/summarize.py" "$run" "$RESULTS/$tag.json" --label "$tag" \
    --meta "server_cores=$SERVER_CORES" --meta "client_cores=$CLIENT_CORES" \
    --meta "workers=${workers:-default (cores/2)}" --meta "kv_int=${KV_INT:-UInt64}" --meta "commit=$(git -C "$REPO" rev-parse --short HEAD)" \
    --meta "load=$(tr '\n' ' ' < "$run/load.txt")" > /dev/null
echo "results: $RESULTS/$tag.json"
