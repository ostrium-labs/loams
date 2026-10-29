#!/usr/bin/env bash
# CB1 loadgen (plan §5): runs every scenario against one target, in the same order
# every time, and writes raw results to $BENCH_OUT. Nothing is dropped or rerun
# selectively; a failed scenario is recorded as failed and the run goes on.
#
# Runs on the in-region loadgen VM (cloud-init calls it) or locally for the dry run.
# Required: BENCH_CLOUD (aws|gcp|azure|local), BENCH_RUN_ID, BENCH_ROW, BENCH_URL,
# BENCH_OUT. Optional sizes below. The cloud's hooks are cold-<cloud>.sh
# (force_cold <n>) and upload-<cloud>.sh (upload <dir>).
set -uo pipefail
export TZ=UTC # vmstat's timestamps and every log line in UTC
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

: "${BENCH_CLOUD:?}" "${BENCH_RUN_ID:?}" "${BENCH_ROW:?}" "${BENCH_URL:?}" "${BENCH_OUT:?}"
OHA="${BENCH_OHA:-oha}"
C="${BENCH_CONCURRENCY:-10}"
REPS="${BENCH_REPS:-3}"
WARMUP_N="${BENCH_WARMUP_N:-1000}"
PING_N="${BENCH_PING_N:-20000}"
READ_N="${BENCH_READ_N:-10000}"
WRITE_N="${BENCH_WRITE_N:-5000}"
RATE="${BENCH_RATE:-20}"
RATE_S="${BENCH_RATE_SECONDS:-60}"
QUEUE_RATE="${BENCH_QUEUE_RATE:-10}"
QUEUE_S="${BENCH_QUEUE_SECONDS:-60}"
COLD_PROBES="${BENCH_COLD_PROBES:-10}"
KEYS="${BENCH_KEYS:-1000}"
VALUE_BYTES="${BENCH_VALUE_BYTES:-1024}"
TIMEOUT="${BENCH_TIMEOUT:-30s}"
SCENARIOS="${BENCH_SCENARIOS:-preload cold ping read write read-rate write-rate queue}"

url="${BENCH_URL%/}"
out="$BENCH_OUT"
mkdir -p "$out"
log() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$out/run.log" >&2; }

# shellcheck source=/dev/null
[[ -f "$here/cold-$BENCH_CLOUD.sh" ]] && . "$here/cold-$BENCH_CLOUD.sh"
# shellcheck source=/dev/null
[[ -f "$here/upload-$BENCH_CLOUD.sh" ]] && . "$here/upload-$BENCH_CLOUD.sh"

# The key space as an oha URL regex: dots in the host must not be regex wildcards.
url_re="$(printf '%s' "$url" | sed -e 's/[.]/\\./g')"
key_width=${#KEYS}; key_width=$(( key_width > 3 ? key_width - 1 : 3 ))
key_re="k[0-9]{$key_width}"
head -c "$VALUE_BYTES" /dev/zero | tr '\0' 'v' > "$out/value.bin"

status=()
record() { status+=("\"$1\":\"$2\""); log "scenario $1: $2"; }

meta() {
  jq -n --arg cloud "$BENCH_CLOUD" --arg row "$BENCH_ROW" --arg run "$BENCH_RUN_ID" --arg url "$url" \
    --arg started "$(date -u +%FT%TZ)" --arg host "$(uname -n)" --arg kernel "$(uname -r)" \
    --arg oha "$("$OHA" --version 2>/dev/null)" --argjson nproc "$(nproc)" \
    --arg region "${BENCH_REGION:-}" --arg instance_type "${BENCH_INSTANCE_TYPE:-}" --arg commit "${BENCH_COMMIT:-}" \
    --argjson cfg "$(jq -n --argjson c "$C" --argjson reps "$REPS" --argjson warmup "$WARMUP_N" --argjson ping "$PING_N" \
      --argjson read "$READ_N" --argjson write "$WRITE_N" --argjson rate "$RATE" --argjson rate_s "$RATE_S" \
      --argjson qrate "$QUEUE_RATE" --argjson qs "$QUEUE_S" --argjson cold "$COLD_PROBES" --argjson keys "$KEYS" \
      --argjson vb "$VALUE_BYTES" --arg scen "$SCENARIOS" \
      '{concurrency:$c,reps:$reps,warmup_n:$warmup,ping_n:$ping,read_n:$read,write_n:$write,rate_rps:$rate,rate_seconds:$rate_s,queue_rate:$qrate,queue_seconds:$qs,cold_probes:$cold,keys:$keys,value_bytes:$vb,scenarios:$scen}')" \
    '{cloud:$cloud,row:$row,run_id:$run,url:$url,started:$started,region:$region,loadgen:{host:$host,kernel:$kernel,nproc:$nproc,instance_type:$instance_type},oha:$oha,commit:$commit,config:$cfg}' \
    > "$out/meta.json"
}

# One oha invocation: JSON summary, per-request SQLite (--db-url, plan §5.3) and
# its start and end in reps.jsonl, so the summary can match it to the loadgen's
# CPU samples.
oha_run() { # name, then oha arguments
  local name="$1"; shift
  local start rc
  start="$(date -u +%FT%T.%3NZ)"
  "$OHA" --no-tui --output-format json -o "$out/$name.json" --db-url "$out/$name.db" -t "$TIMEOUT" "$@" 2> "$out/$name.err"
  rc=$?
  jq -cn --arg name "$name" --arg start "$start" --arg end "$(date -u +%FT%T.%3NZ)" --argjson rc "$rc" \
    '{name:$name,start:$start,end:$end,rc:$rc}' >> "$out/reps.jsonl"
  return $rc
}

# The target answers /ping before anything is measured (a fresh deploy can take
# minutes to route). Not a measurement: kept in run.log only.
wait_ready() {
  local deadline=$((SECONDS + ${BENCH_READY_SECONDS:-600})) code
  while ((SECONDS < deadline)); do
    code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$url/ping")
    [[ $code == 200 ]] && { log "target ready"; return 0; }
    sleep 5
  done
  log "target not ready after ${BENCH_READY_SECONDS:-600}s (last $code)"
  return 1
}

preload() {
  # Each key exactly once, 10 at a time; a throttled write is retried after a pause
  # (preload is setup, not a measurement).
  seq -f "k%0${key_width}g" 0 $((KEYS - 1)) | xargs -P "$C" -I{} bash -c '
    for i in 1 2 3 4 5 6 7 8; do
      code=$(curl -s -o /dev/null -w "%{http_code}" -X PUT --data-binary @"$2" "$1/items/{}")
      [[ $code == 200 ]] && { echo "{} $code"; exit 0; }
      sleep $((i * 2))
    done; echo "{} $code"' _ "$url" "$out/value.bin" > "$out/preload.txt"
  local ok; ok=$(awk '$2==200' "$out/preload.txt" | wc -l)
  log "preload: $ok/$KEYS keys written"
  [[ $ok -eq $KEYS ]]
}

cold() {
  : > "$out/cold.jsonl"
  local i
  for ((i = 1; i <= COLD_PROBES; i++)); do
    local forced_at; forced_at="$(date -u +%FT%T.%3NZ)"
    if declare -F force_cold >/dev/null; then force_cold "$i" >> "$out/cold-force.log" 2>&1 || log "force_cold $i failed"; fi
    local sent_at; sent_at="$(date -u +%FT%T.%3NZ)"
    local body timing
    timing=$(curl -s -o "$out/.cold-body" -w '{"code":%{http_code},"ttfb_s":%{time_starttransfer},"total_s":%{time_total},"connect_s":%{time_connect},"tls_s":%{time_appconnect}}' "$url/ping")
    body=$(cat "$out/.cold-body" 2>/dev/null); jq -e . >/dev/null 2>&1 <<<"$body" || body='null'
    jq -cn --argjson n "$i" --arg forced "$forced_at" --arg sent "$sent_at" --argjson t "$timing" --argjson b "$body" \
      '{probe:$n,forced_at:$forced,sent_at:$sent} + $t + {app:$b}' >> "$out/cold.jsonl"
    # a warm request on a new connection right after, the baseline for this probe
    timing=$(curl -s -o /dev/null -w '{"code":%{http_code},"ttfb_s":%{time_starttransfer},"total_s":%{time_total}}' "$url/ping")
    jq -cn --argjson n "$i" --argjson t "$timing" '{probe:$n,warm_after:$t}' >> "$out/cold.jsonl"
  done
  rm -f "$out/.cold-body"
}

closed() { # scenario: ping|read|write
  local s="$1" n args=() rep
  case "$s" in
    ping)  n=$PING_N;  args=("$url/ping") ;;
    read)  n=$READ_N;  args=(--rand-regex-url "$url_re/items/$key_re") ;;
    write) n=$WRITE_N; args=(-m PUT -D "$out/value.bin" -T application/octet-stream --rand-regex-url "$url_re/items/$key_re") ;;
  esac
  local rc=0
  for ((rep = 1; rep <= REPS; rep++)); do
    oha_run "$s-r$rep-warmup" -n "$WARMUP_N" -c "$C" "${args[@]}" || rc=1
    oha_run "$s-r$rep" -n "$n" -c "$C" "${args[@]}" || rc=1
  done
  return $rc
}

rate() { # read-rate|write-rate
  local s="$1" args=()
  case "$s" in
    read-rate)  args=(--rand-regex-url "$url_re/items/$key_re") ;;
    write-rate) args=(-m PUT -D "$out/value.bin" -T application/octet-stream --rand-regex-url "$url_re/items/$key_re") ;;
  esac
  oha_run "$s" -z "${RATE_S}s" -q "$RATE" -c "$C" --latency-correction "${args[@]}"
}

queue() {
  oha_run queue-send -z "${QUEUE_S}s" -q "$QUEUE_RATE" -c 2 -m POST "$url/enqueue?run=$BENCH_RUN_ID" || return 1
  local sent; sent=$(jq '[.statusCodeDistribution["202"] // 0] | add' "$out/queue-send.json")
  local deadline=$((SECONDS + 180)) got=0
  while ((SECONDS < deadline)); do
    curl -s "$url/queue/stats?run=$BENCH_RUN_ID" -o "$out/queue-stats.json"
    got=$(jq '.received // 0' "$out/queue-stats.json" 2>/dev/null || echo 0)
    ((got >= sent)) && break
    sleep 5
  done
  jq --argjson sent "$sent" --argjson waited "$SECONDS" '. + {sent:$sent}' "$out/queue-stats.json" > "$out/queue-stats.tmp" && mv "$out/queue-stats.tmp" "$out/queue-stats.json"
  log "queue: sent $sent, received $got"
  ((got >= sent))
}

meta
vmstat -n -t 1 > "$out/loadgen-vmstat.log" 2>&1 & sampler=$!
log "run $BENCH_RUN_ID row $BENCH_ROW against $url"
: > "$out/reps.jsonl"
ready=1
wait_ready || ready=0
for s in $SCENARIOS; do
  if ((ready == 0)); then record "$s" skipped-not-ready; continue; fi
  log "scenario $s: start"
  case "$s" in
    preload) preload ;;
    cold) cold ;;
    ping | read | write) closed "$s" ;;
    read-rate | write-rate) rate "$s" ;;
    queue) queue ;;
    *) false ;;
  esac && record "$s" ok || record "$s" failed
done
kill "$sampler" 2>/dev/null
jq -n --arg finished "$(date -u +%FT%TZ)" "{finished:\$finished,scenarios:{$(IFS=,; echo "${status[*]}")}}" > "$out/done.json"
log "done"
if declare -F upload >/dev/null; then upload "$out" && log "uploaded" || log "upload failed"; fi
