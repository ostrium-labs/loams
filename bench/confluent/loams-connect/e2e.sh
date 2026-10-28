#!/usr/bin/env bash
# End to end through the stack up.sh started:
#   producer -> $CFB_TOPIC_IN -> [connect source] -> Loams stream $CFB_STREAM
#            -> [connect exactly-once sink] -> $CFB_TOPIC_OUT -> read_committed consumer
# Produces CFB_N records at CFB_RATE/s, kill -9s the connect process when
# CFB_KILL_AT of them are sent (CFB_KILL=0 turns it off) and starts it again
# after CFB_KILL_DOWN_S, then waits for every record on both sides and prints
# a JSON summary (counts, duplicates, missing, p50/p99 per leg). Exit 1 when
# anything is lost or duplicated.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

: "${CFB_N:=10000}"
: "${CFB_RATE:=1000}"
: "${CFB_KILL:=1}"
: "${CFB_KILL_AT:=0.5}"
: "${CFB_KILL_DOWN_S:=2}"
: "${CFB_TIMEOUT_S:=180}"

pid_alive "$CFB_RUN/engine.pid" || die "the engine is not running (up.sh)"
pid_alive "$CFB_RUN/connect.pid" || die "connect is not running (up.sh or connect.sh restart)"
write_client_config

RUN="r$(date +%s)-$$"
W="$CFB_RUN/e2e/$RUN"; mkdir -p "$W"
export ENGINE_URL ENGINE_KEY_FILE="$SECRETS/engine-key" CFB_NAMESPACE CFB_STREAM CFB_PARTITIONS
PY=(python3 -I "$HERE/cfb.py")
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; }
trap cleanup EXIT

log "run $RUN: $CFB_N records at ${CFB_RATE}/s, kill -9 connect: $([[ $CFB_KILL == 1 ]] && echo "at ${CFB_KILL_AT}" || echo no)"

# Readers first: the Loams stream from its current end, the output topic
# (read_committed, LogAppendTime stamps) from the beginning, filtered by run.
start=$("${PY[@]}" watermarks)
dlq_hw0() { engine_call FetchCloudEvents "{\"namespace\":\"$CFB_NAMESPACE\",\"stream\":\"$1\",\"partition\":0,\"offset\":\"0\",\"maxBytes\":1}" 2>/dev/null \
  | python3 -I -c 'import json,sys; print(int(json.load(sys.stdin).get("highWatermark",0)))' 2>/dev/null || echo 0; }
dlq_in0=$(dlq_hw0 cfb-in-dlq); dlq_out0=$(dlq_hw0 cfb-out-dlq)
"${PY[@]}" poll "$RUN" "$CFB_N" "$W/loams.jsonl" "$start" 2>"$W/poll.err" & pids+=($!)
kafka_tool kafka-console-consumer.sh --topic "$CFB_TOPIC_OUT" --from-beginning \
  --isolation-level read_committed --formatter-property print.timestamp=true \
  >"$W/out.tsv" 2>"$W/consumer.err" & pids+=($!)
sleep 3

t0=$(date +%s%3N)
"${PY[@]}" produce "$RUN" "$CFB_N" "$CFB_RATE" | kafka_tool kafka-console-producer.sh --topic "$CFB_TOPIC_IN" \
  --reader-property parse.key=true --reader-property key.separator=$'\t' \
  --command-property linger.ms=5 --command-property acks=all >/dev/null 2>"$W/producer.err" &
prod=$!; pids+=($prod)

killed_at=null; restart_ms=null
if [[ $CFB_KILL == 1 ]]; then
  sleep "$(python3 -I -c "print($CFB_N*$CFB_KILL_AT/$CFB_RATE)")"
  killed_at=$(( $(date +%s%3N) - t0 ))
  log "kill -9 connect (pid $(cat "$CFB_RUN/connect.pid")) at +${killed_at} ms"
  kill -KILL "$(cat "$CFB_RUN/connect.pid")"
  sleep "$CFB_KILL_DOWN_S"
  r0=$(date +%s%3N)
  start_connect
  wait_instances_running
  restart_ms=$(( $(date +%s%3N) - r0 ))
  log "connect running again after ${restart_ms} ms"
fi
wait "$prod" || die "the producer failed: $(tail -n 3 "$W/producer.err")"
t_sent=$(( $(date +%s%3N) - t0 ))
log "all $CFB_N sent in ${t_sent} ms; waiting for both sides (timeout ${CFB_TIMEOUT_S}s)"

deadline=$(( $(date +%s) + CFB_TIMEOUT_S ))
count_out() { grep -c "\"run\":\"$RUN\"" "$W/out.tsv" 2>/dev/null || true; }
while (( $(date +%s) < deadline )); do
  c=$(count_out)
  (( c >= CFB_N )) && break
  sleep 1
done
sleep 3   # let late duplicates, if any, arrive
touch "$W/loams.jsonl.stop"
t_done=$(( $(date +%s%3N) - t0 ))
cleanup; pids=()

dlq_hw() { engine_call FetchCloudEvents "{\"namespace\":\"$CFB_NAMESPACE\",\"stream\":\"$1\",\"partition\":0,\"offset\":\"0\",\"maxBytes\":1}" 2>/dev/null \
  | python3 -I -c 'import json,sys; print(int(json.load(sys.stdin).get("highWatermark",0)))' 2>/dev/null || echo 0; }
dlq_in=$(( $(dlq_hw cfb-in-dlq) - dlq_in0 )); dlq_out=$(( $(dlq_hw cfb-out-dlq) - dlq_out0 ))
meta=$(printf '{"kafka":"%s","rate_per_s":%s,"partitions":%s,"kill9_at_ms":%s,"kill9_down_s":%s,"connect_restart_ms":%s,"dlq_events":{"cfb-in-dlq":%s,"cfb-out-dlq":%s},"send_ms":%s,"wall_ms":%s,"workdir":"%s"}' \
  "$KAFKA_BOOTSTRAP$([[ $KAFKA_SASL == 1 ]] && echo ' (SASL_SSL)')" "$CFB_RATE" "$CFB_PARTITIONS" \
  "$killed_at" "$([[ $CFB_KILL == 1 ]] && echo "$CFB_KILL_DOWN_S" || echo null)" "$restart_ms" "$dlq_in" "$dlq_out" "$t_sent" "$t_done" "$W")
"${PY[@]}" report "$RUN" "$CFB_N" "$W/loams.jsonl" "$W/out.tsv" "$meta" | tee "$W/summary.json"
