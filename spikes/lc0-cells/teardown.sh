#!/usr/bin/env bash
# teardown_leaves_nothing (LC0 Task 1): delete the spike's Starter instance,
# its local ti SQL credentials, and confirm no lc0-cells cluster remains.
# Usage: teardown.sh <cluster-id>   (TiDB Cloud API keys in the environment)
set -euo pipefail
ID=$1
export PATH="$HOME/.ti/bin:$PATH" TI_TELEMETRY=off
name=$(ti db describe-db-cluster --db-cluster-id "$ID" --query display_name --output text)
[ "$name" = "lc0-cells" ] || { echo "refusing: cluster $ID is '$name', not lc0-cells"; exit 1; }
ti db delete-db-cluster --db-cluster-id "$ID" --wait --query state --output text
rm -rf "$HOME/.ti/db_users/$ID"
left=$(ti db list-db-clusters --db-cluster-type starter --query "length(clusters[?display_name=='lc0-cells'])" --output text)
[ -d "$HOME/.ti/db_users/$ID" ] && { echo "FAIL: local credentials remain"; exit 1; }
[ "$left" = "0" ] || { echo "FAIL: $left lc0-cells cluster(s) remain"; exit 1; }
echo "teardown_leaves_nothing: PASS"
