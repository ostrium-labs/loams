#!/bin/bash
# LC0 Task 3 teardown: delete the Worker (its Durable Object namespaces go with it), delete only
# the Starter instance named lc0-worker with the recorded id, remove ti's local SQL users for it,
# then run teardown_leaves_nothing.
#   LC0_SCRATCH=<dir> LC0_CLUSTER_ID=<id> ./teardown.sh
set -euo pipefail
cd "$(dirname "$0")"
: "${LC0_CLUSTER_ID:?}"
TI=~/.ti/bin/ti
tidb() { bash -c 'set -a; . ~/tidbcloud-lc0.env; set +a; exec "$@"' _ "$@"; }
cf() { bash -c 'set -a; . ~/cloudflare-lc0.env; set +a; export WRANGLER_SEND_METRICS=false; exec "$@"' _ "$@"; }

cf ./node_modules/.bin/wrangler delete --name loams-cloud-lc0 --force

# Never touch another agent's instance: the id must match AND the name must be lc0-worker.
name=$(tidb $TI db list-db-clusters --db-cluster-type starter | jq -r --arg id "$LC0_CLUSTER_ID" '.clusters[] | select(.id == $id) | .display_name')
if [ "$name" = "lc0-worker" ]; then
  tidb $TI db delete-db-cluster --db-cluster-id "$LC0_CLUSTER_ID" --wait > /dev/null
elif [ -n "$name" ]; then
  echo "refusing: cluster $LC0_CLUSTER_ID is named $name" >&2; exit 1
fi
rm -rf ~/.ti/db_users/"$LC0_CLUSTER_ID"
bash -c 'set -a; . ~/cloudflare-lc0.env; . ~/tidbcloud-lc0.env; set +a; LC0_TEARDOWN=1 exec node --test test/' 
