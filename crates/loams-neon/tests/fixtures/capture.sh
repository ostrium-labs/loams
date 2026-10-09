#!/usr/bin/env bash
# Re-record loams-neon's fixtures (PG2 Task 2) against deploy/neon at its pinned
# digests. Request fixtures (*.request.json) are what loams-neon sends; this
# script sends them as they are, so a fixture the pageserver refuses fails here.
# Responses go to *.response.json and their HTTP statuses to statuses.txt.
#
#   (cd deploy/neon && docker compose up -d rustfs create-bucket storage_broker pageserver)
#   crates/loams-neon/tests/fixtures/capture.sh [pageserver URL] [loams-wal URL] [compute_ctl URL]
#
# With a compute_ctl URL and COMPUTE_JWT (README.md: compute-jwt.py and
# compose.capture.yaml), compute_ctl's /status is recorded too, with and
# without the token.
#
# With a loams-wal HTTP URL (for example a `loams-wal --listen-http
# 127.0.0.1:57701` started for the purpose), WalClient's fixtures are
# recorded too (wal_*).
#   (cd deploy/neon && docker compose down -v)
#
# The ids are fixed ("LoamsNeonTenant1", "LoamsNeonTimeln1", "LoamsNeonBranch1"
# in hex), so the request fixtures do not change between captures; the branch
# point is the main timeline's last_record_lsn at capture time.
set -euo pipefail
cd "$(dirname "$0")"
PS=${1:-http://127.0.0.1:9898}
WAL=${2:-}
CTL=${3:-}
T=4c6f616d734e656f6e54656e616e7431
TL=4c6f616d734e656f6e54696d656c6e31
BR=4c6f616d734e656f6e4272616e636831
BR2=4c6f616d734e656f6e4272616e636832
: > statuses.txt
call() { # name method path [body-file] [base URL] [extra curl args...]
  local name=$1 method=$2 path=$3 body=${4:-} base=${5:-$PS} code
  shift $(( $# < 5 ? $# : 5 ))
  if [ -n "$body" ]; then
    code=$(curl -s -o "$name.response.json" -w '%{http_code}' -X "$method" "$@" \
      -H 'Content-Type: application/json' --data-binary "@$body" "$base$path")
  else
    code=$(curl -s -o "$name.response.json" -w '%{http_code}' -X "$method" "$@" "$base$path")
  fi
  echo "$name $method $path $code" >> statuses.txt
  echo "$name: $code" >&2
}
json() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))$2)" "$1"; }

call attach PUT "/v1/tenant/$T/location_config" attach.request.json
call create_timeline POST "/v1/tenant/$T/timeline" create_timeline.request.json
# The timeline's WAL starts at initdb's end; wait until it is readable.
sleep 2
call timeline_get GET "/v1/tenant/$T/timeline/$TL"
lsn=$(json timeline_get.response.json "['last_record_lsn']")
printf '{"new_timeline_id":"%s","ancestor_timeline_id":"%s","ancestor_start_lsn":"%s"}' \
  "$BR" "$TL" "$lsn" > branch.request.json
call branch POST "/v1/tenant/$T/timeline" branch.request.json
call timeline_list GET "/v1/tenant/$T/timeline"
call lsn_by_timestamp GET "/v1/tenant/$T/timeline/$TL/get_lsn_by_timestamp?timestamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
call tenant_config PATCH "/v1/tenant/config" tenant_config.request.json
# The same new id with another ancestor: a conflict.
printf '{"new_timeline_id":"%s","pg_version":17}' "$BR" > conflict.request.json
call conflict POST "/v1/tenant/$T/timeline" conflict.request.json
call not_found GET "/v1/tenant/$T/timeline/00000000000000000000000000000000"
# A branch of the branch below the branch's own start: 406.
below=$(python3 -c "import sys; h,l=sys.argv[1].split('/'); v=(int(h,16)<<32|int(l,16))-8; print(f'{v>>32:X}/{v&0xffffffff:X}')" "$lsn")
printf '{"new_timeline_id":"%s","ancestor_timeline_id":"%s","ancestor_start_lsn":"%s"}' \
  "$BR2" "$BR" "$below" > branch_below_ancestor.request.json
call branch_below_ancestor POST "/v1/tenant/$T/timeline" branch_below_ancestor.request.json
# The main timeline has a child: 412. An unknown tenant: 412 too.
call delete_with_children DELETE "/v1/tenant/$T/timeline/$TL"
call delete_tenant_missing DELETE "/v1/tenant/00000000000000000000000000000000/timeline/$TL"
call delete_branch DELETE "/v1/tenant/$T/timeline/$BR"

if [ -n "$WAL" ]; then
  call wal_create_timeline POST "/v1/tenant/timeline" wal_create_timeline.request.json "$WAL"
  call wal_timeline_status GET "/v1/tenant/$T/timeline/$TL" "" "$WAL"
  call wal_create_conflict POST "/v1/tenant/timeline" conflict_wal.request.json "$WAL"
  call wal_not_found GET "/v1/tenant/$T/timeline/00000000000000000000000000000000" "" "$WAL"
fi

if [ -n "$CTL" ]; then
  : "${COMPUTE_JWT:?set COMPUTE_JWT (compute-jwt.py)}"
  : "${CAPTURE_CONFIG:?set CAPTURE_CONFIG (the config compute-jwt.py wrote)}"
  auth=(-H "Authorization: Bearer $COMPUTE_JWT")
  # compute1 attaches to the timeline created above: start it now (README.md).
  echo "waiting for compute_ctl at $CTL (start compute1 now)" >&2
  for _ in $(seq 1 120); do
    curl -sf "${auth[@]}" "$CTL/status" | grep -q '"status":"running"' && break
    sleep 2
  done
  call compute_status GET "/status" "" "$CTL" "${auth[@]}"
  call compute_unauthorized GET "/status" "" "$CTL" -H "Authorization: Bearer not.a.token"
  call prewarm_state GET "/lfc/prewarm" "" "$CTL" "${auth[@]}"
  # compute1 is a primary, so compute_ctl refuses to promote it (its spec,
  # with the capture's tenant and timeline): a real PromoteState::Failed.
  python3 -I -c "import json,sys; c=json.load(open(sys.argv[1])); s=c['spec']; s['tenant_id']=sys.argv[2]; s['timeline_id']=sys.argv[3]; json.dump({'spec':s,'wal_flush_lsn':'0/0'}, open(sys.argv[4],'w'))" \
    "$CAPTURE_CONFIG" "$T" "$TL" promote.tmp.json
  call promote_primary POST "/promote" promote.tmp.json "$CTL" "${auth[@]}"
  rm -f promote.tmp.json
fi
