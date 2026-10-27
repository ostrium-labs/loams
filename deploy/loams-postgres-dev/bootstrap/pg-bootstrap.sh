#!/bin/sh
# pg-bootstrap: makes sure the tenant and the timeline compute1 attaches to exist on the
# pageserver, then exits 0. It is a one-shot service (compose.yaml); compute1 starts after it.
#
# The ids, in this order:
#   1. TENANT_ID and TIMELINE_ID from the environment (.env), when both are set. They are only
#      ensured to exist; the state file is left as it is.
#   2. The ids an earlier run stored in $STATE (the pg-bootstrap volume), so a rerun, or an
#      `up` after a `down` without -v, reuses them.
#   3. New random ids, written to $STATE before anything is created, so a run that fails half
#      way is finished by the next one with the same ids.
#
# Each step is idempotent: an existing tenant is left as it is (no new attach, so its generation
# is not touched) and an existing timeline is not created again. The pageserver is reached on the
# compose network only (http://pageserver:9898). POSIX sh and curl only: it runs in the storage
# image, which has curl (create-bucket uses it) but no jq.
set -eu

PAGESERVER=${PAGESERVER:-http://pageserver:9898}
STATE=${STATE:-/var/lib/pg-bootstrap/ids.env}
PG_VERSION=${PG_VERSION:-17}

new_id() { od -An -tx1 -N16 /dev/urandom | tr -d ' \n'; }

valid_id() {
	case $1 in
	*[!0-9a-f]*) return 1 ;;
	esac
	[ ${#1} -eq 32 ]
}

# HTTP status of a request; 000 when the pageserver did not answer.
status() { curl -s -o /dev/null -w '%{http_code}' "$@" || true; }

tenant=${TENANT_ID:-}
timeline=${TIMELINE_ID:-}
if [ -n "$tenant" ] || [ -n "$timeline" ]; then
	if [ -z "$tenant" ] || [ -z "$timeline" ]; then
		echo "pg-bootstrap: set both TENANT_ID and TIMELINE_ID in .env, or neither" >&2
		exit 1
	fi
	echo "pg-bootstrap: using TENANT_ID and TIMELINE_ID from the environment"
elif [ -s "$STATE" ]; then
	tenant=$(sed -n 's/^TENANT_ID=//p' "$STATE")
	timeline=$(sed -n 's/^TIMELINE_ID=//p' "$STATE")
	echo "pg-bootstrap: reusing the ids in $STATE"
else
	tenant=$(new_id)
	timeline=$(new_id)
	mkdir -p "$(dirname "$STATE")"
	printf 'TENANT_ID=%s\nTIMELINE_ID=%s\n' "$tenant" "$timeline" >"$STATE.tmp"
	mv "$STATE.tmp" "$STATE"
	echo "pg-bootstrap: new ids, stored in $STATE"
fi
for id in "$tenant" "$timeline"; do
	valid_id "$id" || { echo "pg-bootstrap: '$id' is not a 32-character hex id" >&2; exit 1; }
done

tries=0
until [ "$(status "$PAGESERVER/v1/status")" = 200 ]; do
	tries=$((tries + 1))
	[ "$tries" -le 120 ] || { echo "pg-bootstrap: the pageserver did not answer at $PAGESERVER" >&2; exit 1; }
	echo "pg-bootstrap: waiting for the pageserver"
	sleep 1
done

# The tenant: attach it (create it) only when the pageserver does not have it.
code=$(status "$PAGESERVER/v1/tenant/$tenant")
case $code in
200) echo "pg-bootstrap: tenant $tenant exists" ;;
404)
	echo "pg-bootstrap: creating tenant $tenant"
	curl -sS -f -X PUT -H 'Content-Type: application/json' \
		-d '{"mode":"AttachedSingle","generation":1,"tenant_conf":{}}' \
		"$PAGESERVER/v1/tenant/$tenant/location_config" >/dev/null
	;;
*)
	echo "pg-bootstrap: GET /v1/tenant/$tenant answered $code" >&2
	exit 1
	;;
esac

# The timeline. A tenant that was just attached may still be activating (503), so the create is
# retried; the same id with the same parameters answers the existing timeline. An existing
# timeline is never touched: the GET finds it first.
tries=0
while :; do
	code=$(status "$PAGESERVER/v1/tenant/$tenant/timeline/$timeline")
	[ "$code" = 200 ] && break
	echo "pg-bootstrap: creating timeline $timeline (Postgres $PG_VERSION)"
	code=$(status -X POST -H 'Content-Type: application/json' \
		-d "{\"new_timeline_id\":\"$timeline\",\"pg_version\":$PG_VERSION}" \
		"$PAGESERVER/v1/tenant/$tenant/timeline/")
	# 409 is a create still in progress: the GET above finds the timeline on a later pass.
	case $code in
	200 | 201) break ;;
	esac
	tries=$((tries + 1))
	[ "$tries" -le 60 ] || { echo "pg-bootstrap: creating the timeline answered $code" >&2; exit 1; }
	sleep 1
done

echo "pg-bootstrap: ready: TENANT_ID=$tenant TIMELINE_ID=$timeline"
