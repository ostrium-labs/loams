#!/usr/bin/env bash
# CB1 metrics export, Azure (plan §5.3): Azure Monitor figures for the run window
# (Flex on-demand executions and execution units, Cosmos DB request units and
# 429s, queue transactions) and Application Insights' first request per
# instance for the cold scenario. Writes raw responses and usage.json.
#
#   metrics/azure.sh <outputs.json> <start ISO> <end ISO> <outdir>
set -uo pipefail
outputs="$1" start="$2" end="$3" dir="$4"
mkdir -p "$dir"
o() { jq -r ".$1.value // empty" "$outputs"; }
app_id="$(o function_app_id)" rg="$(o resource_group)" storage="$(o storage_account)"
rc=0

metric() { # name resource-id metrics [filter]
  local args=(--resource "$2" --metrics $3 --start-time "$start" --end-time "$end" --interval PT1M --aggregation Total -o json)
  [[ -n "${4:-}" ]] && args+=(--filter "$4")
  az monitor metrics list "${args[@]}" > "$dir/$1.json" || { echo "metric $1 failed"; rc=1; echo '{}' > "$dir/$1.json"; }
}
metric flex "$app_id" "OnDemandFunctionExecutionCount OnDemandFunctionExecutionUnits"
cosmos_id="$(az cosmosdb list -g "$rg" --query '[0].id' -o tsv)"
metric cosmos-ru "$cosmos_id" "TotalRequestUnits"
metric cosmos-requests "$cosmos_id" "TotalRequests" "StatusCode eq '*'"
storage_id="$(az storage account show -g "$rg" -n "$storage" --query id -o tsv)"
metric queue-transactions "$storage_id/queueServices/default" "Transactions"
metric storage-transactions "$storage_id" "Transactions"

# Application Insights: the first request each instance served.
ai_id="$(az resource list -g "$rg" --resource-type microsoft.insights/components --query '[0].id' -o tsv)"
if [[ -n $ai_id ]]; then
  appid="$(az resource show --ids "$ai_id" --query properties.AppId -o tsv)"
  jq -n --arg q "requests | where timestamp between (datetime($start) .. datetime($end)) | summarize first=min(timestamp), requests=count() by cloud_RoleInstance | order by first asc" \
    '{query:$q}' > "$dir/.ai-query.json"
  az rest --method post --url "https://api.applicationinsights.io/v1/apps/$appid/query" \
    --resource https://api.applicationinsights.io --body @"$dir/.ai-query.json" > "$dir/appinsights-instances.json" ||
    { echo "app insights query failed"; rc=1; echo '{}' > "$dir/appinsights-instances.json"; }
  rm -f "$dir/.ai-query.json"
else
  echo '{}' > "$dir/appinsights-instances.json"
fi

total() { # file metric-name
  jq --arg m "$2" '[.value[]? | select(.name.value == $m) | .timeseries[]?.data[]?.total // 0] | add // 0' "$dir/$1.json"
}
status() { # 429s from TotalRequests split by StatusCode
  jq '[.value[]?.timeseries[]? | select(any(.metadatavalues[]?; .name.value == "statuscode" and .value == "429")) | .data[]?.total // 0] | add // 0' "$dir/cosmos-requests.json"
}
jq -n --arg start "$start" --arg end "$end" \
  --argjson execs "$(total flex OnDemandFunctionExecutionCount)" --argjson units "$(total flex OnDemandFunctionExecutionUnits)" \
  --argjson ru "$(total cosmos-ru TotalRequestUnits)" --argjson c429 "$(status)" \
  --argjson qtx "$(total queue-transactions Transactions)" --argjson stx "$(total storage-transactions Transactions)" \
  --argjson inst "$(jq '[.tables[0].rows[]?] | length' "$dir/appinsights-instances.json" 2>/dev/null || echo 0)" \
  '{cloud:"azure",window:{start:$start,end:$end},
    usage:{
      flex_executions:$execs,
      flex_gb_s:($units / 1024 / 1000),
      cosmos_request_units:$ru,
      storage_transactions:$stx,
      queue_transactions:$qtx},
    throttles:{cosmos_429:$c429},
    cold_platform:{source:"Application Insights requests: first request per cloud_RoleInstance", instances:$inst},
    notes:["flex_gb_s converts OnDemandFunctionExecutionUnits from MB-milliseconds (verify the unit on the first run)",
           "Cosmos DB free tier: 1,000 RU/s provisioned; consumed RUs are not billed separately"]}' \
  > "$dir/usage.json"
exit $rc
