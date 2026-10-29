#!/usr/bin/env bash
# CB1 metrics export, AWS (plan §5.3): the cloud's own usage figures for the run
# window, and Lambda's Init Duration for the cold scenario. Runs before destroy
# (the log group goes with it). Writes raw responses and usage.json.
#
#   metrics/aws.sh <outputs.json> <start ISO> <end ISO> <outdir>
set -uo pipefail
outputs="$1" start="$2" end="$3" dir="$4"
mkdir -p "$dir"
o() { jq -r ".$1.value // empty" "$outputs"; }
fn="$(o function_name)" table="$(o items_table)" queue_url="$(o queue_url)" log_group="$(o log_group)" mem="$(o memory_mb)"
queue="${queue_url##*/}"
rc=0

# --- CloudWatch metrics, one GetMetricData call, 60 s periods ---------------------------
q() { # id namespace metric dim-name dim-value stat
  jq -cn --arg id "$1" --arg ns "$2" --arg m "$3" --arg dn "$4" --arg dv "$5" --arg s "$6" \
    '{Id:$id,MetricStat:{Metric:{Namespace:$ns,MetricName:$m,Dimensions:[{Name:$dn,Value:$dv}]},Period:60,Stat:$s},ReturnData:true}'
}
queries="$(jq -s . <<EOF
$(q invocations AWS/Lambda Invocations FunctionName "$fn" Sum)
$(q duration AWS/Lambda Duration FunctionName "$fn" Sum)
$(q errors AWS/Lambda Errors FunctionName "$fn" Sum)
$(q throttles AWS/Lambda Throttles FunctionName "$fn" Sum)
$(q concurrency AWS/Lambda ConcurrentExecutions FunctionName "$fn" Maximum)
$(q rcu AWS/DynamoDB ConsumedReadCapacityUnits TableName "$table" Sum)
$(q wcu AWS/DynamoDB ConsumedWriteCapacityUnits TableName "$table" Sum)
$(q read_throttles AWS/DynamoDB ReadThrottleEvents TableName "$table" Sum)
$(q write_throttles AWS/DynamoDB WriteThrottleEvents TableName "$table" Sum)
$(q sqs_sent AWS/SQS NumberOfMessagesSent QueueName "$queue" Sum)
$(q sqs_received AWS/SQS NumberOfMessagesReceived QueueName "$queue" Sum)
$(q sqs_deleted AWS/SQS NumberOfMessagesDeleted QueueName "$queue" Sum)
$(q sqs_empty AWS/SQS NumberOfEmptyReceives QueueName "$queue" Sum)
EOF
)"
aws cloudwatch get-metric-data --start-time "$start" --end-time "$end" --metric-data-queries "$queries" \
  --output json > "$dir/cloudwatch.json" || rc=1

# --- Logs Insights: billed duration and Init Duration per REPORT line ---------------------
insights() { # name query
  local id status
  id="$(aws logs start-query --log-group-name "$log_group" --start-time "$(date -d "$start" +%s)" \
    --end-time "$(date -d "$end" +%s)" --query-string "$2" --query queryId --output text)" || return 1
  for _ in $(seq 1 60); do
    aws logs get-query-results --query-id "$id" --output json > "$dir/$1.json" || return 1
    status="$(jq -r .status "$dir/$1.json")"
    [[ $status == Complete ]] && return 0
    [[ $status == Running || $status == Scheduled ]] || return 1
    sleep 2
  done
  return 1
}
insights logs-report 'filter @type = "REPORT"
| stats count(*) as invocations, sum(@billedDuration) as billed_ms, count(@initDuration) as cold,
  avg(@initDuration) as init_avg, pct(@initDuration, 50) as init_p50, pct(@initDuration, 99) as init_p99, max(@initDuration) as init_max' || rc=1
insights logs-init 'filter @type = "REPORT" and ispresent(@initDuration)
| fields @timestamp, @initDuration, @billedDuration | sort @timestamp asc | limit 1000' || rc=1

# --- usage.json: the figures cost.py prices ----------------------------------------------------
sum() { jq "[.MetricDataResults[] | select(.Id == \"$1\") | .Values[]] | add // 0" "$dir/cloudwatch.json" 2>/dev/null || echo 0; }
field() { jq -r "[.results[0][]? | select(.field == \"$1\") | .value][0] // \"0\"" "$dir/logs-report.json" 2>/dev/null || echo 0; }
mode="$(jq -r '.items_capacity_mode // "provisioned"' "$(dirname "$dir")/inputs.json" 2>/dev/null || echo provisioned)"
jq -n --arg start "$start" --arg end "$end" --argjson mem "${mem:-256}" \
  --argjson inv "$(sum invocations)" --argjson dur "$(sum duration)" --argjson billed "$(field billed_ms)" \
  --argjson rcu "$(sum rcu)" --argjson wcu "$(sum wcu)" --argjson rt "$(sum read_throttles)" --argjson wt "$(sum write_throttles)" \
  --argjson sent "$(sum sqs_sent)" --argjson recv "$(sum sqs_received)" --argjson del "$(sum sqs_deleted)" --argjson empty "$(sum sqs_empty)" \
  --argjson cold "$(field cold)" --arg mode "$mode" \
  '{cloud:"aws",window:{start:$start,end:$end},
    usage:{
      lambda_requests:$inv,
      lambda_gb_s:((if $billed > 0 then $billed else $dur end) / 1000 * $mem / 1024),
      dynamodb_read_units:$rcu, dynamodb_write_units:$wcu, dynamodb_mode:$mode,
      sqs_requests:($sent + $recv + $del + $empty)},
    throttles:{lambda:null, dynamodb_read:$rt, dynamodb_write:$wt},
    cold_platform:{source:"Lambda REPORT Init Duration", cold_invocations:$cold},
    notes:["lambda_gb_s uses Billed Duration from the REPORT lines when present (it includes the billed INIT phase), else the Duration metric",
           "sqs_requests counts sends, receives, deletes and empty receives; the event-source mapping polls (empty receives) are billed"]}' \
  > "$dir/usage.json"
exit $rc
