#!/usr/bin/env bash
# HE0 (plan HE1 Task 1): run one mined ClickHouse application.
#
#   scripts/house/apps.sh <app> up      start it (compose up --wait)
#   scripts/house/apps.sh <app> drive   run its driver against the running stack
#   scripts/house/apps.sh <app> dump    write query_log.tsv (with the settings
#                                       its logging proxy saw, R1.1), async_insert_log.tsv,
#                                       part_log.tsv and its migration files
#                                       under $HE0_OUT/<app>/
#   scripts/house/apps.sh <app> down    stop it and remove its volumes
#
# <app> is a `name` in conformance/clickhouse/apps/manifest.toml. The run
# refuses an app with no `licence` (apps_manifest_requires_licence holds the
# same rule statically). Dumps are raw and never committed: they hold query
# text, so only `loams-house-mine scrub` output leaves $HE0_OUT.
#
# Environment:
#   HE0_OUT               dump directory (default conformance/clickhouse/apps/.out)
#   HE0_CLICKHOUSE_IMAGE  another pinned clickhouse-server (the 24.3 floor)
#   HE0_TARGET            clickhouse (default) or house: the app's
#                         compose.house.yaml points it at a running House
#                         (scripts/house/app-on-house.sh starts one)
#   COMPOSE               the compose command (default: docker compose, else
#                         podman compose)
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
APPS="$ROOT/conformance/clickhouse/apps"
MANIFEST="$APPS/manifest.toml"
OUT_ROOT=${HE0_OUT:-$APPS/.out}

usage() {
  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
  exit 2
}

[ $# -eq 2 ] || usage
APP=$1
VERB=$2

# One field of one [[app]] from the manifest (a flat TOML subset: `key = "v"`,
# `key = n`, `key = true`). Python's tomllib keeps this exact.
field() {
  python3 -I - "$MANIFEST" "$APP" "$1" <<'PY'
import sys, tomllib
path, app, key = sys.argv[1:4]
with open(path, "rb") as f:
    doc = tomllib.load(f)
for entry in doc.get("app", []):
    if entry.get("name") == app:
        value = entry.get(key, "")
        print(value if not isinstance(value, bool) else str(value).lower())
        sys.exit(0)
sys.exit(3)
PY
}

if ! field name >/dev/null 2>&1; then
  echo "apps.sh: no [[app]] named '$APP' in $MANIFEST" >&2
  exit 2
fi
LICENCE=$(field licence)
if [ -z "$LICENCE" ]; then
  echo "apps.sh: '$APP' has no licence in $MANIFEST; it does not run (§75 §18)" >&2
  exit 2
fi

COMPOSE_FILE="$APPS/$APP/compose.yaml"
[ -f "$COMPOSE_FILE" ] || { echo "apps.sh: missing $COMPOSE_FILE" >&2; exit 2; }

if [ -n "${COMPOSE:-}" ]; then
  read -r -a COMPOSE_CMD <<<"$COMPOSE"
elif command -v docker >/dev/null 2>&1; then
  COMPOSE_CMD=(docker compose)
else
  COMPOSE_CMD=(podman compose)
fi
# HE0_TARGET=house (Task 9) layers <app>/compose.house.yaml, which points the
# application at a running House instead of its clickhouse-server.
COMPOSE_FILES=(-f "$COMPOSE_FILE")
case "${HE0_TARGET:-clickhouse}" in
  clickhouse) ;;
  house)
    [ -f "$APPS/$APP/compose.house.yaml" ] || {
      echo "apps.sh: '$APP' has no compose.house.yaml" >&2; exit 2; }
    COMPOSE_FILES+=(-f "$APPS/$APP/compose.house.yaml")
    ;;
  *) echo "apps.sh: HE0_TARGET is clickhouse or house" >&2; exit 2 ;;
esac
compose() { "${COMPOSE_CMD[@]}" "${COMPOSE_FILES[@]}" "$@"; }

# The service running clickhouse-server: `clickhouse` or `op-ch`.
ch_service() {
  compose config --services | grep -E '^(clickhouse|op-ch)$' | head -n1
}

# A query against the app's clickhouse-server, tagged so dumps skip it.
ch() {
  compose exec -T "$(ch_service)" clickhouse-client \
    --user "${HE0_CH_USER:-$(ch_user)}" --password "${HE0_CH_PASSWORD:-$(ch_password)}" \
    --log_comment he0-harness --query "$1"
}
ch_user() { compose config --format json | python3 -I -c '
import json, sys
s = json.load(sys.stdin)["services"]
env = (s.get("clickhouse") or s.get("op-ch"))["environment"] or {}
print(env.get("CLICKHOUSE_USER", "default"))'; }
ch_password() { compose config --format json | python3 -I -c '
import json, sys
s = json.load(sys.stdin)["services"]
env = (s.get("clickhouse") or s.get("op-ch"))["environment"] or {}
print(env.get("CLICKHOUSE_PASSWORD", ""))'; }

# The dump contract of loams-house-mine extract --query-log (HE1 R2.5):
# TSVWithNames with query_id and query, plus type, is_initial_query and
# Settings; the other columns are context the extractor may ignore. Only
# finished or failed statements an application sent (not the harness's own).
QUERY_LOG_SQL="SELECT
    query_id, type, is_initial_query, event_time_microseconds, query_kind, current_database,
    interface, http_user_agent, exception_code,
    used_functions, used_aggregate_functions, used_aggregate_function_combinators,
    used_table_functions, used_formats, used_data_type_families,
    used_database_engines, used_storages, Settings, query
FROM system.query_log
WHERE type != 'QueryStart' AND is_initial_query AND log_comment != 'he0-harness'
ORDER BY event_time_microseconds, query_id
FORMAT TSVWithNames"

# Async inserts (async_insert = 1, wait_for_async_insert = 0) reach query_log
# only as the server's own flush, not as initial queries (HE1 R4.5), so the
# statements an application sent that way come from asynchronous_insert_log,
# in the same TSVWithNames shape (query_id, query).
ASYNC_INSERT_LOG_SQL="SELECT
    query_id, status, event_time_microseconds, database, table, format,
    bytes, rows, flush_query_id, exception, query
FROM system.asynchronous_insert_log
ORDER BY event_time_microseconds, query_id
FORMAT TSVWithNames"

PART_LOG_SQL="SELECT event_type, merge_reason, database, table, partition_id, rows, count() AS n
FROM system.part_log
WHERE database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema')
GROUP BY ALL ORDER BY database, table, event_type, merge_reason, partition_id
FORMAT TSVWithNames"

case "$VERB" in
  up)
    compose up -d --wait --quiet-pull
    ;;
  drive)
    DRIVER=$(field driver)
    [ -n "$DRIVER" ] || { echo "apps.sh: '$APP' has no driver" >&2; exit 2; }
    HE0_APP=$APP bash "$APPS/$DRIVER"
    ;;
  dump)
    OUT="$OUT_ROOT/$APP"
    mkdir -p "$OUT"
    ch "SYSTEM FLUSH LOGS"
    ch "$QUERY_LOG_SQL" >"$OUT/query_log.tsv"
    ch "$ASYNC_INSERT_LOG_SQL" >"$OUT/async_insert_log.tsv"
    # The logging proxy's access log (R1.1): the settings each request sent,
    # defaults included, merged into query_log.tsv by query_id.
    CHLOG=$(compose config --services | grep -E '^(chlog|op-chlog)$' | head -n1 || true)
    if [ -n "$CHLOG" ]; then
      compose cp "$CHLOG:/var/log/chlog/access.log" "$OUT/http_log.json"
      python3 -I "$ROOT/scripts/house/http_settings.py" \
        "$OUT/query_log.tsv" "$OUT/async_insert_log.tsv" "$OUT/http_log.json"
    fi
    ch "$PART_LOG_SQL" >"$OUT/part_log.tsv"
    ch "SELECT version()" >"$OUT/clickhouse_version.txt"
    MIGRATIONS=$(field migrations)
    if [ -n "$MIGRATIONS" ]; then
      # The service that runs the manifest's `image` holds the migrations.
      SERVICE=$(compose config --format json | python3 -I -c '
import json, sys
image = sys.argv[1]
for name, s in sorted(json.load(sys.stdin)["services"].items()):
    if s.get("image") == image:
        print(name)
        break' "$(field image)")
      # Copy the directory's contents into a fresh directory, then swap it in:
      # `cp svc:/dir dest` nests or skips when dest exists (a second dump).
      rm -rf "$OUT/migrations" "$OUT/.migrations.tmp"
      mkdir -p "$OUT/.migrations.tmp"
      if compose cp "$SERVICE:${MIGRATIONS%/}/." "$OUT/.migrations.tmp"; then
        mv "$OUT/.migrations.tmp" "$OUT/migrations"
      else
        rm -rf "$OUT/.migrations.tmp"
        echo "apps.sh: warning: no migrations at $SERVICE:$MIGRATIONS" >&2
      fi
    fi
    echo "apps.sh: $APP dumped $(($(wc -l <"$OUT/query_log.tsv") - 1)) statements to $OUT"
    ;;
  down)
    compose down --volumes --remove-orphans
    ;;
  *)
    usage
    ;;
esac
