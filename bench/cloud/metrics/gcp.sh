#!/usr/bin/env bash
# CB1 metrics export, Google Cloud (plan §5.3): Cloud Monitoring figures for the
# run window (Cloud Run requests, billable instance time and startup latencies;
# Firestore operations; Cloud Tasks operations) and Cloud Run's "Starting new
# instance" system log lines for the cold scenario. Writes raw responses and
# usage.json.
#
#   metrics/gcp.sh <outputs.json> <start ISO> <end ISO> <outdir>
set -uo pipefail
outputs="$1" start="$2" end="$3" dir="$4"
mkdir -p "$dir"
o() { jq -r ".$1.value // empty" "$outputs"; }
project="$(o project_id)" service="$(o service_name)"
token="$(gcloud auth print-access-token)" || exit 1
rc=0

series() { # name filter aligner [reducer]
  local url="https://monitoring.googleapis.com/v3/projects/$project/timeSeries"
  local args=(--data-urlencode "filter=$2" --data-urlencode "interval.startTime=$start" --data-urlencode "interval.endTime=$end"
    --data-urlencode "aggregation.alignmentPeriod=60s" --data-urlencode "aggregation.perSeriesAligner=$3")
  [[ -n "${4:-}" ]] && args+=(--data-urlencode "aggregation.crossSeriesReducer=$4")
  curl -sf -G -H "Authorization: Bearer $token" "${args[@]}" "$url" > "$dir/$1.json" || { echo "series $1 failed"; rc=1; echo '{}' > "$dir/$1.json"; }
}
svc="resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"$service\""
series run-requests "metric.type=\"run.googleapis.com/request_count\" AND $svc" ALIGN_DELTA REDUCE_SUM
series run-requests-by-class "metric.type=\"run.googleapis.com/request_count\" AND $svc" ALIGN_DELTA
series run-billable "metric.type=\"run.googleapis.com/container/billable_instance_time\" AND $svc" ALIGN_DELTA REDUCE_SUM
series run-startup "metric.type=\"run.googleapis.com/container/startup_latencies\" AND $svc" ALIGN_DELTA
series run-instances "metric.type=\"run.googleapis.com/container/instance_count\" AND $svc" ALIGN_MAX
# Firestore's operation counts (verify the metric names on the first run: the
# read_ops_count family replaced document/read_count in 2025).
series fs-reads 'metric.type="firestore.googleapis.com/document/read_ops_count"' ALIGN_DELTA REDUCE_SUM
series fs-writes 'metric.type="firestore.googleapis.com/document/write_ops_count"' ALIGN_DELTA REDUCE_SUM
series fs-deletes 'metric.type="firestore.googleapis.com/document/delete_ops_count"' ALIGN_DELTA REDUCE_SUM
series tasks-api 'metric.type="cloudtasks.googleapis.com/api/request_count"' ALIGN_DELTA REDUCE_SUM
series tasks-attempts 'metric.type="cloudtasks.googleapis.com/queue/task_attempt_count"' ALIGN_DELTA REDUCE_SUM

# Cloud Run's system log marks each new instance.
jq -n --arg p "projects/$project" --arg f "resource.type=\"cloud_run_revision\" AND resource.labels.service_name=\"$service\" AND textPayload:\"Starting new instance\" AND timestamp>=\"$start\" AND timestamp<=\"$end\"" \
  '{resourceNames:[$p],filter:$f,orderBy:"timestamp asc",pageSize:1000}' |
  curl -sf -X POST -H "Authorization: Bearer $token" -H 'content-type: application/json' --data @- \
    https://logging.googleapis.com/v2/entries:list > "$dir/run-new-instances.json" || { echo "log query failed"; rc=1; echo '{}' > "$dir/run-new-instances.json"; }

total() { # file: the sum of every point's int64 or double value
  jq '[.timeSeries[]?.points[]?.value | (.int64Value // .doubleValue // 0) | tonumber] | add // 0' "$dir/$1.json"
}
cpu="$(jq -r '.cpu // "1"' "$(dirname "$dir")/inputs.json" 2>/dev/null || echo 1)"
jq -n --arg start "$start" --arg end "$end" \
  --argjson req "$(total run-requests)" --argjson billable "$(total run-billable)" \
  --argjson reads "$(total fs-reads)" --argjson writes "$(total fs-writes)" --argjson deletes "$(total fs-deletes)" \
  --argjson tasks "$(total tasks-api)" --argjson attempts "$(total tasks-attempts)" \
  --argjson newinst "$(jq '[.entries[]?] | length' "$dir/run-new-instances.json")" --argjson cpu "${cpu:-1}" \
  '{cloud:"gcp",window:{start:$start,end:$end},
    usage:{
      run_requests:$req,
      run_vcpu_s:($billable * $cpu),
      run_gib_s:($billable * 0.5),
      firestore_reads:$reads, firestore_writes:$writes, firestore_deletes:$deletes,
      tasks_operations:($tasks + $attempts)},
    cold_platform:{source:"Cloud Run system log: Starting new instance; container/startup_latencies", new_instances:$newinst},
    notes:["run_gib_s assumes the row'"'"'s 512 MiB instances; billable_instance_time is request-based billing'"'"'s billed time"]}' \
  > "$dir/usage.json"
exit $rc
