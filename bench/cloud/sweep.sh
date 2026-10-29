#!/usr/bin/env bash
# CB1 sweep (plan §7.3): lists what still carries loams-bench=<run-id> on a
# cloud after destroy, and exits 1 if anything does. The tag listings lag, so the
# services the rows use are also listed directly by the run's name prefix.
#
#   bench/cloud/sweep.sh <aws|gcp|azure> <run-id> [outputs.json]
#
# Uses the credentials already in the environment (run.sh loads them). Prints a
# timestamped report; the known no-cost leftovers (plan §7.6) are not counted.
set -uo pipefail
cloud="${1:?cloud}" run_id="${2:?run id}" outputs="${3:-}"
name="b-$run_id"
found=0
echo "sweep $cloud loams-bench=$run_id at $(date -u +%FT%TZ)"

hit() { # what, listing (empty means nothing found)
  if [[ -n "$2" ]]; then
    found=1
    printf 'LEFT  %s:\n%s\n' "$1" "$(sed 's/^/      /' <<<"$2")"
  else
    printf 'none  %s\n' "$1"
  fi
}
err() { found=1; printf 'ERROR %s: listing failed (counted as not clean)\n' "$1"; }
list() { # what, command...
  local what="$1" out; shift
  if out="$("$@" 2>&1)"; then hit "$what" "$(grep -v -E '^[[:space:]]*(None)?[[:space:]]*$' <<<"$out" || true)"; else err "$what"; echo "$out" | sed 's/^/      /' | head -5; fi
}

case "$cloud" in
  aws)
    list "tagged resources (Resource Groups Tagging API)" \
      aws resourcegroupstaggingapi get-resources --tag-filters "Key=loams-bench,Values=$run_id" \
      --query 'ResourceTagMappingList[].ResourceARN' --output text
    list "Lambda functions" aws lambda list-functions \
      --query "Functions[?starts_with(FunctionName, 'loams-app-$name')].FunctionName" --output text
    list "DynamoDB tables" bash -c "aws dynamodb list-tables --output json | jq -r '.TableNames[] | select(startswith(\"loams-app-$name\"))'"
    list "SQS queues" bash -c "aws sqs list-queues --queue-name-prefix 'loams-app-$name' --output json | jq -r '.QueueUrls // [] | .[]'"
    list "log groups" aws logs describe-log-groups --log-group-name-prefix "/aws/lambda/loams-app-$name" \
      --query 'logGroups[].logGroupName' --output text
    list "EC2 instances (not terminated)" aws ec2 describe-instances \
      --filters "Name=tag:loams-bench,Values=$run_id" "Name=instance-state-name,Values=pending,running,stopping,stopped,shutting-down" \
      --query 'Reservations[].Instances[].InstanceId' --output text
    list "security groups" aws ec2 describe-security-groups --filters "Name=group-name,Values=$name-loadgen" \
      --query 'SecurityGroups[].GroupId' --output text
    list "S3 buckets" bash -c "aws s3api list-buckets --output json | jq -r '.Buckets[].Name | select(startswith(\"loams-bench-$run_id-\"))'"
    list "IAM roles" bash -c "aws iam list-roles --output json | jq -r '.Roles[].RoleName | select(. == \"loams-app-$name\" or . == \"$name-loadgen\" or . == \"loams-app-$name-scheduler\")'"
    list "budgets" bash -c "aws budgets describe-budgets --account-id \"\$(aws sts get-caller-identity --query Account --output text)\" --output json | jq -r '.Budgets // [] | .[].BudgetName | select(startswith(\"loams-app-$name-\"))'"
    ;;
  gcp)
    project="${CLOUDSDK_CORE_PROJECT:-${GCP_PROJECT:-}}"
    [[ -n $project ]] || { echo "no project (CLOUDSDK_CORE_PROJECT)"; exit 1; }
    region="${BENCH_REGION:-us-central1}"
    list "labelled resources (Cloud Asset Inventory)" gcloud asset search-all-resources --scope="projects/$project" \
      --query="labels.loams-bench=$run_id" --format='value(name)'
    list "Cloud Run services" gcloud run services list --region "$region" --filter="metadata.name~^loams-$name" --format='value(metadata.name)'
    list "Cloud Tasks queues" gcloud tasks queues list --location "$region" --filter="name~loams-$name" --format='value(name)'
    list "Artifact Registry repositories" gcloud artifacts repositories list --location "$region" --filter="name~loams-$name" --format='value(name)'
    list "Compute instances" gcloud compute instances list --filter="labels.loams-bench=$run_id" --format='value(name)'
    list "Compute networks" gcloud compute networks list --filter="name=$name-loadgen" --format='value(name)'
    list "service accounts" gcloud iam service-accounts list --filter="email~^(loams-$name|$name-lg)@" --format='value(email)'
    list "buckets" gcloud storage buckets list --filter="name~loams-bench-$run_id" --format='value(name)'
    # The (default) Firestore database is deleted on destroy when the run created it;
    # its name is reusable (plan §7.6). Report it, do not count it.
    if [[ -n $outputs && -f $outputs ]] && [[ "$(jq -r '.free_tier.value.free | tostring' "$outputs")" == *Firestore* ]]; then
      echo "note  Firestore databases: $(gcloud firestore databases list --format='value(name)' 2>/dev/null | tr '\n' ' ')"
    fi
    ;;
  azure)
    list "tagged resources" az resource list --tag "loams-bench=$run_id" --query '[].id' -o tsv
    rg="rg-loams-$name"
    if [[ "$(az group exists -n "$rg" 2>/dev/null)" == true ]]; then hit "resource group" "$rg"; else hit "resource group" ""; fi
    ;;
  *) echo "unknown cloud $cloud"; exit 2 ;;
esac

if ((found)); then echo "result: NOT CLEAN"; exit 1; fi
echo "result: clean"
