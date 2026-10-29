#!/usr/bin/env bash
# CB1: one benchmark run of one row, end to end (plan §5 and §7).
#
#   bench/cloud/run.sh <row> <run-id> --yes [--max-minutes 90] [--sweep-wait 600]
#   bench/cloud/run.sh <row> <run-id> --plan-only
#
# <row> is a key of bench/cloud/rows.json (aws-free, aws-ondemand, gcp-free,
# azure-free). <run-id> is new: lowercase letters, digits and -, up to 16.
#
# What it does, in order:
#   1. the cost guard (ledger.py check): caps, the 25% stop line, 06:00-20:00 IST,
#      the hold file, no earlier run left standing;
#   2. builds what the row deploys (the Lambda or Flex zip, or the Cloud Run image);
#   3. writes the run to the ledger, then arms the EXIT trap;
#   4. tofu apply (Google Cloud: the image repository alone first, then the push,
#      then everything);
#   5. waits for the loadgen's done.json (until --max-minutes), downloads the raw
#      results and the cloud's metrics;
#   6. the trap, on success, failure, timeout or interrupt: tofu destroy (three
#      tries), sweep.sh now and again after --sweep-wait, the ledger's destroy time
#      and sweep result. A dirty sweep exits 3 and must be escalated at once.
#
# Nothing is created without --yes. --plan-only stops after tofu plan.
#
# Credentials are read from files and never printed (plan §9):
#   aws    ~/aws.env (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY[, AWS_SESSION_TOKEN])
#   gcp    ~/gcp-sa.json and ~/gcp.env (GCP_PROJECT, GCP_BILLING_ACCOUNT)
#   azure  ~/azure.env (ARM_CLIENT_ID, ARM_CLIENT_SECRET, ARM_TENANT_ID, ARM_SUBSCRIPTION_ID)
# BENCH_BUDGET_EMAILS (comma-separated) receives the budget alerts.
# The gcloud and az logins are kept in the run's work directory, not in ~.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
tofu="${TOFU:-$(command -v tofu || echo /mnt/Projects/tools/bin/tofu)}"

usage() { sed -n '2,32p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 2; }
[[ $# -ge 2 ]] || usage
row="$1" run_id="$2"; shift 2
yes=0 plan_only=0 max_minutes=90 sweep_wait=600 date_dir="$(date -u +%F)"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --yes) yes=1 ;;
    --plan-only) plan_only=1 ;;
    --max-minutes) max_minutes="$2"; shift ;;
    --sweep-wait) sweep_wait="$2"; shift ;;
    --date) date_dir="$2"; shift ;;
    *) usage ;;
  esac
  shift
done
[[ $run_id =~ ^[a-z0-9][a-z0-9-]{0,15}$ ]] || { echo "run id: lowercase letters, digits and -, up to 16" >&2; exit 2; }
((yes || plan_only)) || { echo "refusing to create cloud resources without --yes (or use --plan-only)" >&2; exit 2; }

rowjson="$(jq -ec --arg r "$row" '.rows[$r]' "$here/rows.json")" || { echo "unknown row $row" >&2; exit 2; }
cloud="$(jq -r .cloud <<<"$rowjson")"
root="$here/$(jq -r .root <<<"$rowjson")"
estimate="$(jq -r .estimate_usd <<<"$rowjson")"
work="${BENCH_WORK:-$HOME/.cache/loams-bench}/$run_id"
results="$repo/bench/results/cloud-free-tier/$date_dir-$row"
commit="$(git -C "$repo" rev-parse HEAD)"
log() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*" | tee -a "$work/run.log" >&2; }

[[ ! -e "$results" ]] || { echo "$results exists: one directory per row and date (use --date)" >&2; exit 2; }
mkdir -p "$work"
chmod 700 "$work"

# --- 1. the guard ------------------------------------------------------------------
python3 -I "$here/ledger.py" check --cloud "$cloud" --row "$row" --estimate "$estimate" || exit $?

# --- credentials (never echoed) --------------------------------------------------------
load_credentials() {
  local f
  case "$cloud" in
    aws)
      f="$HOME/aws.env"; [[ -s $f ]] || { echo "missing $f" >&2; return 1; }
      set -a; . "$f"; set +a
      export AWS_DEFAULT_REGION="$(jq -r .region <<<"$rowjson")" AWS_PAGER=""
      aws sts get-caller-identity >/dev/null || { echo "aws credentials refused" >&2; return 1; } ;;
    gcp)
      f="$HOME/gcp.env"; [[ -s $f && -s $HOME/gcp-sa.json ]] || { echo "missing ~/gcp.env or ~/gcp-sa.json (or empty)" >&2; return 1; }
      set -a; . "$f"; set +a
      : "${GCP_PROJECT:?GCP_PROJECT in ~/gcp.env}" "${GCP_BILLING_ACCOUNT:?GCP_BILLING_ACCOUNT in ~/gcp.env}"
      export GOOGLE_APPLICATION_CREDENTIALS="$HOME/gcp-sa.json" CLOUDSDK_CONFIG="$work/gcloud" CLOUDSDK_CORE_PROJECT="$GCP_PROJECT"
      gcloud auth activate-service-account --key-file "$GOOGLE_APPLICATION_CREDENTIALS" --quiet >/dev/null 2>&1 ||
        { echo "gcloud refused the service account key" >&2; return 1; } ;;
    azure)
      f="$HOME/azure.env"; [[ -s $f ]] || { echo "missing $f" >&2; return 1; }
      set -a; . "$f"; set +a
      : "${ARM_CLIENT_ID:?}" "${ARM_CLIENT_SECRET:?}" "${ARM_TENANT_ID:?}" "${ARM_SUBSCRIPTION_ID:?}"
      export AZURE_CONFIG_DIR="$work/az"
      az login --service-principal --username "$ARM_CLIENT_ID" --password "$ARM_CLIENT_SECRET" --tenant "$ARM_TENANT_ID" --output none 2>/dev/null &&
        az account set --subscription "$ARM_SUBSCRIPTION_ID" ||
        { echo "az refused the service principal" >&2; return 1; } ;;
  esac
}
load_credentials || exit 1
: "${BENCH_BUDGET_EMAILS:?BENCH_BUDGET_EMAILS (comma-separated) receives the budget alerts}"

# --- 2. build what the row deploys --------------------------------------------------------
app="$here/app"
build_app() {
  ( cd "$app" && npm ci --no-audit --no-fund --silent && node build.mjs "$1" ) >>"$work/build.log" 2>&1
}
vars="$work/run.tfvars.json"
bench_env="${BENCH_ENV_JSON:-}"   # extra BENCH_* sizes for the loadgen, as a JSON object
[[ -n $bench_env ]] || bench_env='{}'
jq -n --argjson row "$rowjson" --arg run "$run_id" --arg rowname "$row" --arg commit "$commit" \
  --argjson max "$max_minutes" --arg emails "$BENCH_BUDGET_EMAILS" --argjson env "$bench_env" \
  '$row.vars + {run_id:$run,row:$rowname,commit:$commit,max_minutes:$max,budget_emails:($emails|split(",")),bench_env:$env}' > "$vars"
case "$cloud" in
  aws | azure)
    build_app "$cloud" || { echo "build failed (see $work/build.log)" >&2; exit 1; }
    cp "$app/dist/$cloud.zip" "$work/$cloud.zip"
    jq --arg p "$work/$cloud.zip" '. + {package_path:$p}' "$vars" > "$vars.tmp" && mv "$vars.tmp" "$vars" ;;
  gcp)
    jq --arg p "$GCP_PROJECT" --arg b "$GCP_BILLING_ACCOUNT" '. + {project_id:$p,billing_account:$b} | del(.budget_emails)' "$vars" > "$vars.tmp" && mv "$vars.tmp" "$vars" ;;
esac

# --- tofu, with state in the work directory ------------------------------------------------
export TF_DATA_DIR="$work/.terraform" TF_IN_AUTOMATION=1 TF_INPUT=0
tf() { "$tofu" -chdir="$root" "$@"; }
tf init -input=false -backend-config="path=$work/terraform.tfstate" >>"$work/tofu.log" 2>&1 ||
  { echo "tofu init failed (see $work/tofu.log)" >&2; exit 1; }

if ((plan_only)); then
  tf plan -var-file="$vars" -out="$work/plan" 2>&1 | tee -a "$work/tofu.log"
  exit "${PIPESTATUS[0]}"
fi

# --- 3. the ledger, then the trap ---------------------------------------------------------------
python3 -I "$here/ledger.py" start --cloud "$cloud" --row "$row" --run-id "$run_id" --estimate "$estimate" || exit $?
started="$(date -u +%FT%TZ)"
mkdir -p "$results"
torn_down=0
teardown() {
  ((torn_down)) && return
  torn_down=1
  trap - EXIT
  trap '' INT TERM # a second interrupt must not stop the destroy
  log "teardown: tofu destroy"
  local i ok=0 sweep=clean note=""
  for i in 1 2 3; do
    if tf destroy -auto-approve -var-file="$vars" >>"$work/tofu.log" 2>&1; then ok=1; break; fi
    log "destroy attempt $i failed"; sleep $((i * 30))
  done
  ((ok)) || note="destroy failed 3 times"
  "$here/sweep.sh" "$cloud" "$run_id" "$work/outputs.json" > "$results/sweep-1.txt" 2>&1 || sweep=dirty
  if [[ $sweep == clean ]]; then
    log "sweep 1 clean; sweep 2 in ${sweep_wait}s (listing APIs are eventually consistent)"
    sleep "$sweep_wait"
    "$here/sweep.sh" "$cloud" "$run_id" "$work/outputs.json" > "$results/sweep-2.txt" 2>&1 || sweep=dirty
  fi
  python3 -I "$here/ledger.py" destroyed --run-id "$run_id" --sweep "$sweep" --note "$note"
  if [[ $sweep == dirty ]]; then
    log "ESCALATE: resources tagged loams-bench=$run_id remain on $cloud (see $results/sweep-*.txt); tell the coordinator now"
    exit 3
  fi
  log "teardown complete"
}
trap teardown EXIT
trap 'log "interrupted"; exit 130' INT TERM

# --- 4. apply ------------------------------------------------------------------------------------
if [[ $cloud == gcp ]]; then
  log "apply (repository only)"
  tf apply -auto-approve -var-file="$vars" -var loadgen=false \
    -target=module.app.google_artifact_registry_repository.this >>"$work/tofu.log" 2>&1 || { log "apply failed"; exit 1; }
  repo_path="$(tf output -raw repository)"
  image="$repo_path/bench-app:$run_id"
  log "build and push $image"
  podman build -q -f "$app/Containerfile.cloudrun" -t "$image" "$app" >>"$work/build.log" 2>&1 || { log "image build failed"; exit 1; }
  gcloud auth print-access-token | podman login -u oauth2accesstoken --password-stdin "https://${repo_path%%/*}" >/dev/null 2>&1 &&
    podman push --digestfile "$work/digest" "$image" >>"$work/build.log" 2>&1 || { log "push failed"; exit 1; }
  jq --arg i "${repo_path}/bench-app@$(cat "$work/digest")" '. + {image:$i}' "$vars" > "$vars.tmp" && mv "$vars.tmp" "$vars"
fi
log "apply"
tf apply -auto-approve -var-file="$vars" >>"$work/tofu.log" 2>&1 || { log "apply failed (see $work/tofu.log)"; exit 1; }
tf output -json > "$work/outputs.json"
jq 'with_entries(.value |= .value)' "$work/outputs.json" > "$results/tofu-outputs.json"
# the inputs, without the alert addresses
jq 'del(.budget_emails, .package_path)' "$vars" > "$results/inputs.json"
printf '%s\n' "$commit" > "$results/commit"
results_url="$(jq -r .results_url.value "$work/outputs.json")"

# --- 5. wait for the loadgen ------------------------------------------------------------------------
remote_exists() {
  case "$cloud" in
    aws) aws s3 ls "${results_url}done.json" >/dev/null 2>&1 ;;
    gcp) gcloud storage ls "${results_url}done.json" >/dev/null 2>&1 ;;
    azure)
      local acct; acct="$(jq -r .storage_account.value "$work/outputs.json")"
      [[ "$(az storage blob exists --auth-mode login --account-name "$acct" --container-name bench-results \
        --name "$run_id/$row/done.json" --query exists -o tsv 2>/dev/null)" == true ]] ;;
  esac
}
download() {
  case "$cloud" in
    aws) aws s3 cp --recursive --only-show-errors "$results_url" "$results/" ;;
    gcp) gcloud storage cp --recursive --quiet "${results_url}*" "$results/" ;;
    azure)
      local acct; acct="$(jq -r .storage_account.value "$work/outputs.json")"
      az storage blob download-batch --auth-mode login --account-name "$acct" --source bench-results \
        --pattern "$run_id/$row/*" --destination "$work/dl" --output none &&
        cp -r "$work/dl/$run_id/$row/." "$results/" ;;
  esac
}
deadline=$(( $(date +%s) + max_minutes * 60 ))
log "waiting for ${results_url}done.json (until $(date -u -d "@$deadline" +%FT%TZ))"
done_seen=0
while (( $(date +%s) < deadline )); do
  if remote_exists; then done_seen=1; break; fi
  sleep 60
done
((done_seen)) || log "no done.json by the deadline: downloading what there is"
download || log "download failed"

# --- the cloud's own figures, before destroy removes the log groups ---------------------------------
log "metrics export"
"$here/metrics/$cloud.sh" "$work/outputs.json" "$started" "$(date -u +%FT%TZ)" "$results/metrics" >>"$work/metrics.log" 2>&1 ||
  log "metrics export incomplete (see $work/metrics.log)"

# per-request SQLite files are large; keep them compressed
find "$results" -name '*.db' -exec gzip -9 {} +

teardown
cp "$work/run.log" "$results/wrapper.log"
python3 -I "$here/summarize.py" --results "$repo/bench/results/cloud-free-tier" || log "summarize failed"
((done_seen)) || exit 4
exit 0
