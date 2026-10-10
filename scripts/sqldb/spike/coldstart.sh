#!/usr/bin/env bash
# tidb-server start to the first `SELECT 1` (SQ1 Task 1, measurement 2), split
# into container start (podman run returns), port open (TCP accept on 24000)
# and first query (mysql answers SELECT 1). Each case runs with
# force-init-stats = true (the v8.5.8 default) and false.
#   (a) first bootstrap on a fresh keyspace;
#   (b) warm restart of a keyspace holding 0, 100 and 1000 tables;
#   then the TiDB's RSS idle and after 60 s of mixed read/write load.
# Writes target-spike/coldstart.jsonl and target-spike/rss.jsonl.
# Env: TABLE_SETS ("0 100 1000"), BOOT_RUNS (5), WARM_RUNS (20), WARM_RUNS_1000 (5), LOAD_SECS (60).
# jq and awk programs are single-quoted on purpose.
# shellcheck disable=SC2016
set -euo pipefail
# shellcheck source=lib.sh source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

BOOT_RUNS="${BOOT_RUNS:-5}"
WARM_RUNS="${WARM_RUNS:-20}"
WARM_RUNS_1000="${WARM_RUNS_1000:-5}"
LOAD_SECS="${LOAD_SECS:-60}"
PORT="$(tidb_port 0)"
run="c$(date +%H%M%S)"
: > "$SPIKE_OUT/coldstart.jsonl"
: > "$SPIKE_OUT/rss.jsonl"
trap 'tidb_stop 0' EXIT

# measure <phase> <keyspace> <tables> <force> <run>
measure() {
  local t0 t1 t2 t3
  t0="$(now_ms)"
  tidb_start 0 "$2" false "$4"
  t1="$(now_ms)"
  wait_port "$PORT" 600 || { log "port never opened"; return 1; }
  t2="$(now_ms)"
  wait_query "$PORT" 120 || { log "SELECT 1 never answered"; return 1; }
  t3="$(now_ms)"
  emit coldstart.jsonl --arg phase "$1" --arg ks "$2" --argjson tables "$3" \
    --argjson force "$4" --argjson run "$5" \
    --argjson start $((t1 - t0)) --argjson port $((t2 - t1)) --argjson query $((t3 - t2)) \
    '{phase:$phase, keyspace:$ks, tables:$tables, force_init_stats:$force, run:$run,
      start_ms:$start, port_ms:$port, query_ms:$query, total_ms:($start+$port+$query)}'
  log "$1 tables=$3 force=$4 run=$5: start $((t1 - t0)) port $((t2 - t1)) query $((t3 - t2)) ms"
}

# (a) first bootstrap, fresh keyspace every run.
for force in true false; do
  for r in $(seq "$BOOT_RUNS"); do
    ks="${run}b${force:0:1}$r"
    ks_create "$ks" >/dev/null
    measure bootstrap "$ks" 0 "$force" "$r"
    tidb_stop 0
  done
done

# populate <n>: n tables of 20 rows each, analyzed, so stats exist to load.
populate() {
  local n="$1" f="$SPIKE_OUT/.populate.sql" i
  {
    echo "CREATE DATABASE IF NOT EXISTS app; USE app;"
    for i in $(seq "$n"); do
      echo "CREATE TABLE t$i (id BIGINT PRIMARY KEY, k INT, v VARCHAR(64), KEY (k));"
      echo "INSERT INTO t$i VALUES (1,1,'a'),(2,2,'b'),(3,3,'c'),(4,4,'d'),(5,5,'e'),(6,6,'f'),(7,7,'g'),(8,8,'h'),(9,9,'i'),(10,10,'j'),(11,1,'a'),(12,2,'b'),(13,3,'c'),(14,4,'d'),(15,5,'e'),(16,6,'f'),(17,7,'g'),(18,8,'h'),(19,9,'i'),(20,10,'j');"
    done
    for i in $(seq "$n"); do echo "ANALYZE TABLE t$i;"; done
  } > "$f"
  sql "$PORT" < "$f" >/dev/null
  rm -f "$f"
}

# (b) warm restarts.
for tables in ${TABLE_SETS:-0 100 1000}; do
  ks="${run}w$tables"
  ks_create "$ks" >/dev/null
  tidb_start 0 "$ks" false false
  wait_port "$PORT" 600 && wait_query "$PORT" 120
  if (( tables > 0 )); then
    t0="$(now_ms)"; populate "$tables"
    log "populated $tables tables in $(( ($(now_ms) - t0) / 1000 )) s"
  fi
  tidb_stop 0
  runs="$WARM_RUNS"; (( tables == 1000 )) && runs="$WARM_RUNS_1000"
  for force in true false; do
    for r in $(seq "$runs"); do
      measure warm "$ks" "$tables" "$force" "$r"
      tidb_stop 0
    done
  done
done

# RSS idle and after load, on a fresh keyspace (force-init-stats = false).
ks="${run}rss"
ks_create "$ks" >/dev/null
tidb_start 0 "$ks" false false
wait_port "$PORT" 600 && wait_query "$PORT" 120
sleep 10
name="$(tidb_name 0)"
emit rss.jsonl --arg at idle --argjson rss "$(rss_kib "$name")" --argjson cg "$(cgroup_mib "$name")" \
  '{at:$at, rss_kib:$rss, cgroup_mib:$cg}'

if command -v sysbench >/dev/null; then
  driver=sysbench
  sql "$PORT" -e 'CREATE DATABASE sbtest'
  sb=(sysbench oltp_read_write --mysql-host=127.0.0.1 --mysql-port="$PORT" --mysql-user=root
      --mysql-db=sbtest --tables=10 --table-size=10000 --threads=8)
  "${sb[@]}" prepare >/dev/null
  "${sb[@]}" --time="$LOAD_SECS" run > "$SPIKE_OUT/.sysbench.txt"
  ops="$(awk '/transactions:/ { print $2 }' "$SPIKE_OUT/.sysbench.txt")"
else
  # No sysbench: 10 tables x 10 000 rows, then 8 mysql clients each running
  # batches of 10 point selects, 1 range select, 1 update and 1 insert per
  # transaction until LOAD_SECS pass. Counts transactions.
  driver=mysql-loop
  {
    echo "CREATE DATABASE bench; USE bench; SET SESSION cte_max_recursion_depth = 20000;"
    for i in $(seq 10); do
      echo "CREATE TABLE s$i (id BIGINT PRIMARY KEY, k INT, c CHAR(120), pad CHAR(60), KEY (k));"
      echo "INSERT INTO s$i WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x < 10000) SELECT x, FLOOR(RAND()*10000), REPEAT('c',120), REPEAT('p',60) FROM n;"
    done
  } | sql "$PORT"
  client() {
    local end=$(( $(now_ms) + LOAD_SECS * 1000 )) txns=0 b j t id
    while (( $(now_ms) < end )); do
      b="USE bench;"
      for j in $(seq 20); do
        t=$(( RANDOM % 10 + 1 )); id=$(( RANDOM % 10000 + 1 ))
        b+="BEGIN;"
        for _ in 1 2 3 4 5 6 7 8 9 10; do b+="SELECT c FROM s$t WHERE id=$(( RANDOM % 10000 + 1 ));"; done
        b+="SELECT SUM(k) FROM s$t WHERE id BETWEEN $id AND $(( id + 100 ));"
        b+="UPDATE s$t SET k=k+1 WHERE id=$id;"
        b+="INSERT INTO s$t VALUES ($(( 100000 + RANDOM * 32768 + RANDOM + j )), 1, 'x', 'y') ON DUPLICATE KEY UPDATE k=k+1;"
        b+="COMMIT;"
      done
      sql "$PORT" -e "$b" >/dev/null
      txns=$(( txns + 20 ))
    done
    echo "$txns"
  }
  for c in $(seq 8); do client > "$SPIKE_OUT/.client$c" & done
  wait
  ops="$(cat "$SPIKE_OUT"/.client* | awk '{ s += $1 } END { print s }')"
  rm -f "$SPIKE_OUT"/.client*
fi
pid="$(podman inspect -f '{{.State.Pid}}' "$name")"
hwm="$(awk '/^VmHWM:/ { print $2 }' "/proc/$pid/status")"
emit rss.jsonl --arg at after_load --arg driver "$driver" --argjson secs "$LOAD_SECS" \
  --argjson txns "${ops:-0}" --argjson rss "$(rss_kib "$name")" --argjson hwm "$hwm" \
  --argjson cg "$(cgroup_mib "$name")" \
  '{at:$at, driver:$driver, secs:$secs, txns:$txns, rss_kib:$rss, hwm_kib:$hwm, cgroup_mib:$cg}'
tidb_stop 0
log "done: $SPIKE_OUT/coldstart.jsonl, $SPIKE_OUT/rss.jsonl"
