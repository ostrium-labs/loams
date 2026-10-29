#!/usr/bin/env bash
# HE0 Task 9 (plan HE1, ruling R9.6): one mined application on the chDB House,
# checked against the pinned clickhouse-server. The CI jobs
# `openpanel_on_house_smoke` and `langfuse_on_house_smoke` run
# `scripts/house/app-on-house.sh openpanel` and `… langfuse-v4`;
# `signoz_on_house_smoke` (HE1 R9.64, nightly) runs `… signoz`.
#
#   1. The oracle: `apps.sh <app> up` and `drive` on clickhouse-server.
#   2. A House on this machine (`loams-fabric house --single-node`), its TLS
#      listener on HE0_HOUSE_TLS_PORT and its native TLS listener on
#      HE0_HOUSE_NATIVE_TLS_PORT for the application's containers.
#   3. The replay: the oracle's query_log against the House
#      (scripts/house/replay.py): every DDL statement, the oracle's rows, and
#      every distinct SELECT compared, in each of the application's
#      databases. A difference fails the run.
#   4. The House emptied, then the application itself on it
#      (`HE0_TARGET=house apps.sh <app> up` and `drive`, its own project), so
#      its migrations, its ingest and its pages run on the House. A failed
#      migration, a lost event or a page that errs fails the run. For SigNoz,
#      each API answer is compared with the oracle's (signoz/compare.py).
#      Langfuse v4 runs its own ClickHouse migrations at start, golang-migrate
#      over the House's native TLS listener (native gate 2, D1575), and its
#      steps 1 and 3 are skipped (REPLAY=0 below; HE1 R9.65): its gate is that
#      it migrates itself, ingests and serves every read and page on the House.
#   5. Everything stopped; the House's data and the dumps stay under $HE0_OUT.
#
# Environment:
#   HE0_HOUSE_BIN   the directory holding loams-fabric and loams-house-worker
#                   (default: the fabric workspace's target/debug)
#   HE0_OUT         where dumps, reports and the House's data go
#                   (default conformance/clickhouse/apps/.out)
#   HE0_HOUSE_SANDBOX  the workers' sandbox (default netns)
#   HE0_KEEP=1      leave the stacks and the House running
#   HE0_HOUSE_{HTTP,TLS,ADMIN,NATIVE,NATIVE_TLS}_PORT  the House's ports
#                   (28124, 28443, 28125, 29000, 29440)
set -euo pipefail

APP=${1:?usage: app-on-house.sh <app>}
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
APPS="$ROOT/conformance/clickhouse/apps"
OUT_ROOT=${HE0_OUT:-$APPS/.out}
WORK="$OUT_ROOT/$APP-on-house"
# A driver keeps its sign-up and ingest state per stack: each run starts clean.
rm -rf "$WORK/oracle" "$WORK/app"
mkdir -p "$WORK"

# The application's databases (the first is the replay's default), and
# whether the House creates them (SigNoz's migrator creates its own).
CREATE_DATABASE=1
case "$APP" in
  openpanel)
    DATABASES=(openpanel)
    ORACLE_HTTP_PORT=${HE0_OPENPANEL_CH_PORT:-28126}
    HOUSE_APP_PORT=3201
    REPLAY=1
    ;;
  langfuse-v4)
    DATABASES=(default)
    HOUSE_APP_PORT=3202
    REPLAY=0
    ;;
  signoz)
    DATABASES=(signoz_traces signoz_logs signoz_metrics signoz_meter signoz_metadata signoz_analytics)
    ORACLE_HTTP_PORT=${HE0_SIGNOZ_CH_PORT:-28146}
    HOUSE_APP_PORT=3501
    REPLAY=1
    CREATE_DATABASE=0
    ;;
  *) echo "app-on-house.sh: no House recipe for '$APP'" >&2; exit 2 ;;
esac

if [ -z "${HE0_HOUSE_BIN:-}" ]; then
  HE0_HOUSE_BIN="$(cd "$ROOT/fabric" && cargo metadata --format-version 1 --no-deps \
    | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug"
fi
for bin in loams-fabric loams-house-worker; do
  [ -x "$HE0_HOUSE_BIN/$bin" ] || { echo "app-on-house.sh: no $HE0_HOUSE_BIN/$bin (cargo build -p $bin)" >&2; exit 2; }
done

HOUSE_HTTP=127.0.0.1:${HE0_HOUSE_HTTP_PORT:-28124}
HOUSE_TLS_PORT=${HE0_HOUSE_TLS_PORT:-28443}
HOUSE_NATIVE_TLS_PORT=${HE0_HOUSE_NATIVE_TLS_PORT:-29440}
HOUSE_DATA="$WORK/house"
HOUSE_USER=$APP
HOUSE_PASSWORD=$(python3 -I -c 'import secrets; print(secrets.token_hex(16))')
HOUSE_PID=""

house_start() {
  rm -rf "$HOUSE_DATA"
  mkdir -p "$HOUSE_DATA"
  local hash
  hash=$(printf '%s' "$HOUSE_PASSWORD" | sha256sum | cut -d' ' -f1)
  printf '[[house.users]]\nuser = "%s"\npassword_sha256 = "%s"\nnamespace = 1\n' \
    "$HOUSE_USER" "$hash" >"$WORK/house.toml"
  "$HE0_HOUSE_BIN/loams-fabric" house --single-node --config "$WORK/house.toml" \
    --data-dir "$HOUSE_DATA" --house-listen "$HOUSE_HTTP" \
    --house-tls-listen "0.0.0.0:$HOUSE_TLS_PORT" --admin-listen "127.0.0.1:${HE0_HOUSE_ADMIN_PORT:-28125}" \
    --native-listen "127.0.0.1:${HE0_HOUSE_NATIVE_PORT:-29000}" --native-tls-listen "0.0.0.0:$HOUSE_NATIVE_TLS_PORT" \
    --worker-binary "$HE0_HOUSE_BIN/loams-house-worker" \
    --sandbox "${HE0_HOUSE_SANDBOX:-netns}" >"$WORK/house.log" 2>&1 &
  HOUSE_PID=$!
  for _ in $(seq 1 120); do
    curl -fsS "http://$HOUSE_HTTP/ping" >/dev/null 2>&1 && break
    kill -0 "$HOUSE_PID" 2>/dev/null || { cat "$WORK/house.log" >&2; exit 1; }
    sleep 1
  done
  if [ "$CREATE_DATABASE" = 1 ]; then
    for db in "${DATABASES[@]}"; do
      curl -fsS -u "$HOUSE_USER:$HOUSE_PASSWORD" "http://$HOUSE_HTTP/" \
        --data-binary "CREATE DATABASE IF NOT EXISTS $db" >/dev/null
    done
  fi
}

house_stop() {
  if [ -n "$HOUSE_PID" ]; then
    kill "$HOUSE_PID" 2>/dev/null || true
    wait "$HOUSE_PID" 2>/dev/null || true
    HOUSE_PID=""
  fi
}

house_app() {
  HE0_TARGET=house COMPOSE_PROJECT_NAME="he0-$APP-house" HE0_OPENPANEL_PORT=$HOUSE_APP_PORT \
    HE0_LANGFUSE_PORT=$HOUSE_APP_PORT HE0_SIGNOZ_PORT=$HOUSE_APP_PORT HE0_SIGNOZ_OTLP_PORT=$((HOUSE_APP_PORT + 18)) \
    HE0_SIGNOZ_RESULTS="$WORK/house-results.json" \
    HE0_HOUSE_USER=$HOUSE_USER HE0_HOUSE_PASSWORD=$HOUSE_PASSWORD \
    HE0_HOUSE_TLS_PORT=$HOUSE_TLS_PORT HE0_HOUSE_TLS_DIR="$HOUSE_DATA/tls" \
    HE0_HOUSE_NATIVE_TLS_PORT=$HOUSE_NATIVE_TLS_PORT \
    HE0_OUT="$WORK/app" "$ROOT/scripts/house/apps.sh" "$APP" "$1"
}

cleanup() {
  if [ "${HE0_KEEP:-0}" != 1 ]; then
    house_app down >/dev/null 2>&1 || true
    HE0_OUT="$WORK/oracle" "$ROOT/scripts/house/apps.sh" "$APP" down >/dev/null 2>&1 || true
    house_stop
  fi
}
trap cleanup EXIT

status=0
if [ "$REPLAY" = 1 ]; then
  echo "app-on-house: 1. the oracle"
  HE0_OUT="$WORK/oracle" "$ROOT/scripts/house/apps.sh" "$APP" up
  HE0_SIGNOZ_RESULTS="$WORK/oracle-results.json" HE0_OUT="$WORK/oracle" \
    "$ROOT/scripts/house/apps.sh" "$APP" drive

  echo "app-on-house: 2. the House"
  house_start

  echo "app-on-house: 3. the replay"
  DATABASE_ARGS=()
  for db in "${DATABASES[@]}"; do DATABASE_ARGS+=(--database "$db"); done
  HE0_HOUSE_PASSWORD=$HOUSE_PASSWORD python3 -I "$ROOT/scripts/house/replay.py" \
    --oracle "http://127.0.0.1:$ORACLE_HTTP_PORT" --house "http://$HOUSE_HTTP" \
    --house-user "$HOUSE_USER" "${DATABASE_ARGS[@]}" --out "$WORK/replay.json" || status=1
  # SigNoz's oracle stack is not needed past the replay: stop it to free the
  # machine for the House's (HE1 R9.64).
  if [ "$APP" = signoz ] && [ "${HE0_KEEP:-0}" != 1 ]; then
    HE0_OUT="$WORK/oracle" "$ROOT/scripts/house/apps.sh" "$APP" down >/dev/null 2>&1 || true
  fi
  house_stop
else
  echo "app-on-house: 1-3. no oracle and replay for $APP"
fi

echo "app-on-house: 4. $APP on the House"
house_start
if house_app up; then
  house_app drive || status=1
  if [ "$APP" = signoz ]; then
    python3 -I "$APPS/signoz/compare.py" "$WORK/oracle-results.json" "$WORK/house-results.json" \
      || status=1
    # The views the House runs in the background (SigNoz's materialized and
    # refreshable views, R9.64) say so in its log when they fail.
    if grep -E "loams-house: a (refreshable|materialized) view .* failed" "$WORK/house.log"; then
      echo "app-on-house: a view failed on the House" >&2
      status=1
    fi
  fi
else
  echo "app-on-house: $APP did not start on the House" >&2
  HE0_TARGET=house COMPOSE_PROJECT_NAME="he0-$APP-house" HE0_HOUSE_USER=$HOUSE_USER \
    HE0_HOUSE_PASSWORD=$HOUSE_PASSWORD HE0_HOUSE_TLS_DIR="$HOUSE_DATA/tls" \
    docker compose -f "$APPS/$APP/compose.yaml" -f "$APPS/$APP/compose.house.yaml" \
    logs --tail 80 2>/dev/null | grep -v "Applying migration" | tail -60 || true
  status=1
fi

if [ "$status" = 0 ]; then
  if [ "$REPLAY" = 1 ]; then
    echo "app-on-house: $APP runs on the House and answers as the oracle"
  else
    echo "app-on-house: $APP migrates itself and runs on the House (no oracle replay)"
  fi
else
  echo "app-on-house: $APP on the House FAILED (see $WORK)" >&2
fi
exit "$status"
