#!/bin/bash
# LC0 Task 3 setup: one free Starter instance, the schema, the Worker and its two secrets.
# Credentials are sourced only inside the commands that need them and never printed.
#   LC0_SCRATCH=<private dir, 0700> ./setup.sh
set -euo pipefail
cd "$(dirname "$0")"
: "${LC0_SCRATCH:?set LC0_SCRATCH to a private directory}"
TI=~/.ti/bin/ti
tidb() { bash -c 'set -a; . ~/tidbcloud-lc0.env; set +a; exec "$@"' _ "$@"; }
cf() { bash -c 'set -a; . ~/cloudflare-lc0.env; set +a; export WRANGLER_SEND_METRICS=false; exec "$@"' _ "$@"; }

ID=$(tidb $TI db create-db-cluster --db-cluster-type starter --db-cluster-name lc0-worker --wait | jq -r .id)
echo "cluster $ID" > "$LC0_SCRATCH/cluster-id"
tidb $TI db create-db-sql-users --db-cluster-id "$ID" > /dev/null
grep -v '^--' sql/schema.sql | grep -v '^$' | while IFS= read -r s; do
  tidb $TI db execute-sql-statement --db-cluster-id "$ID" --admin --sql "$s" > /dev/null
done
npm ci --no-audit --no-fund
cf ./node_modules/.bin/wrangler deploy
tidb $TI db format-db-connection-string --db-cluster-id "$ID" --read-write --database lc0 \
  | jq -r .connection_string | sed -E 's/[?].*$//' | tr -d '\n' \
  | cf ./node_modules/.bin/wrangler secret put DATABASE_URL > /dev/null
( umask 077; head -c 32 /dev/urandom | base64 | tr -d '/+=\n' > "$LC0_SCRATCH/bearer" )
tr -d '\n' < "$LC0_SCRATCH/bearer" | cf ./node_modules/.bin/wrangler secret put LC0_BEARER > /dev/null
echo "done: export LC0_URL=https://loams-cloud-lc0.<subdomain>.workers.dev LC0_BEARER_FILE=$LC0_SCRATCH/bearer"
