#!/usr/bin/env bash
# The nightly Confluent Cloud row (plan CC1 Task 10, design 80 §9, D1831, D1832):
# an ephemeral Basic cluster in Ostrium's `loams-ci` environment, next to the AWS
# nightly VM, through the confluent module, then a smoke and a teardown that is
# checked by id; and an hourly reaper for what a crashed run left behind.
#
#   scripts/byoc/confluent-ci.sh up     [--dry-run]  apply examples/confluent (two phases), mint the keys
#   scripts/byoc/confluent-ci.sh smoke               tests/byoc/confluent_smoke.sh against this run's cluster
#   scripts/byoc/confluent-ci.sh down   [--dry-run]  destroy, delete the run's keys, check nothing is left
#   scripts/byoc/confluent-ci.sh reap   [--dry-run]  delete loams-ci-* clusters and keys older than 3 hours;
#                                                    report (never delete) anything else older than a day
#
# Environment:
#   CONFLUENT_CLOUD_API_KEY/_SECRET  the CI provisioner's Cloud key (EnvironmentAdmin on loams-ci only)
#   CONFLUENT_CI_ENVIRONMENT         the loams-ci environment (env-...)
#   CONFLUENT_CI_DATA_SA             the persistent loams-ci-data service account (sa-...; CC0 R0.6)
#   GITHUB_RUN_ID                    names the run: loams-ci-<run>
#   CONFLUENT_CI_VM                  1 (default): write the data key to AWS Secrets Manager under
#                                    loams/ci-<run>/ for the nightly VM; 0: Confluent only
#   AWS_REGION                       default us-east-1 (Q1334); also the cluster's region
#   CONFLUENT_CI_WORK                the run's local state (default $RUNNER_TEMP or .confluent-ci)
#   TOFU                             OpenTofu (default: tofu on PATH)
#   CONFLUENT_CI_FIXTURES            recorded API JSON for the reaper's tests (no call is made)
#   CONFLUENT_CI_NOW                 the time, in seconds, for those tests
#
# Keys: the Cloud key is read from the environment and passed to curl on stdin;
# Kafka keys minted here live in 0600 files in the work directory and in the
# process environment of `tofu` only; nothing prints a secret. OpenTofu state is
# local, encrypted with a passphrase made for the run, and deleted by `down`.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
api="${CONFLUENT_API_URL:-https://api.confluent.cloud}"
region="${AWS_REGION:-us-east-1}"
run="${GITHUB_RUN_ID:-local}"
name="loams-ci-$run"
env_id="${CONFLUENT_CI_ENVIRONMENT:-}"
work="${CONFLUENT_CI_WORK:-${RUNNER_TEMP:-$repo/.confluent-ci}/confluent-ci-$run}"
now="${CONFLUENT_CI_NOW:-$(date +%s)}"
ttl_seconds=$((3 * 3600))
report_seconds=86400
tofu="${TOFU:-tofu}"
# Never touched, whatever their names say: the Kafka gateway oracle (design 80 §9.1).
protected_envs=" env-3kqddw "
protected_clusters=" lkc-120zwvj "

cmd="${1:-}"; shift || true
dry=0
for a in "$@"; do case "$a" in --dry-run) dry=1 ;; *) echo "confluent-ci.sh: unknown option $a" >&2; exit 2 ;; esac; done

say() { echo "==> $*"; }
die() { echo "confluent-ci.sh: $*" >&2; exit 1; }

need_env() {
  [[ "$env_id" =~ ^env-[a-z0-9]+$ ]] || die "CONFLUENT_CI_ENVIRONMENT is not an env-... id"
  [[ "$protected_envs" != *" $env_id "* ]] || die "$env_id is protected (the oracle's environment)"
}

# api METHOD PATH [json-body] -> body on stdout; HTTP status in $work/status.
# The Cloud key goes to curl on stdin, never in argv.
api() {
  local m="$1" p="$2" url
  [[ "$p" == http* ]] && url="$p" || url="$api$p"
  mkdir -p "$work"
  local args=(-sS -X "$m" -H 'Content-Type: application/json' --max-time 60 --retry 3 --retry-connrefused -o "$work/body" -w '%{http_code}')
  [[ $# -lt 3 ]] || args+=(--data "$3")
  : "${CONFLUENT_CLOUD_API_KEY:?CONFLUENT_CLOUD_API_KEY is not set}" "${CONFLUENT_CLOUD_API_SECRET:?CONFLUENT_CLOUD_API_SECRET is not set}"
  printf 'user = "%s:%s"\n' "$CONFLUENT_CLOUD_API_KEY" "$CONFLUENT_CLOUD_API_SECRET" | curl -K - "${args[@]}" "$url" >"$work/status" || echo 000 >"$work/status"
  cat "$work/body"
}
status() { cat "$work/status"; }

# A recorded fixture (tests) or a paged GET of every item.
list_all() {  # fixture-name path
  # An empty list comes back as "data": null.
  if [[ -n "${CONFLUENT_CI_FIXTURES:-}" ]]; then jq -c '.data[]?' "$CONFLUENT_CI_FIXTURES/$1.json"; return; fi
  local next="$2" page
  while [[ -n "$next" && "$next" != null ]]; do
    page="$(api GET "$next")"
    [[ "$(status)" == 200 ]] || die "GET $next answered HTTP $(status)"
    jq -c '.data[]?' <<<"$page"
    next="$(jq -r '.metadata.next // empty' <<<"$page")"
  done
}

act() {  # description, then the DELETE path
  local what="$1" path="$2"
  if ((dry)) || [[ -n "${CONFLUENT_CI_FIXTURES:-}" ]]; then echo "WOULD DELETE $what ($path)"; return; fi
  api DELETE "$path" >/dev/null
  case "$(status)" in 2*|404) echo "deleted $what" ;; *) echo "FAILED to delete $what: HTTP $(status)"; return 1 ;; esac
}

iso_to_epoch() { date -u -d "$1" +%s 2>/dev/null || echo "$now"; }

# --- reap ------------------------------------------------------------------------------
reap() {
  need_env
  local n=0 failed=0 item id dname created age
  while IFS= read -r item; do
    [[ -n "$item" ]] || continue
    id="$(jq -r .id <<<"$item")"; dname="$(jq -r '.spec.display_name // ""' <<<"$item")"
    created="$(jq -r '.metadata.created_at // ""' <<<"$item")"; age=$((now - $(iso_to_epoch "$created")))
    if [[ "$protected_clusters" == *" $id "* ]]; then echo "keep $id ($dname): protected"; continue; fi
    if [[ "$dname" == loams-ci-* ]]; then
      if ((age > ttl_seconds)); then
        say "expired cluster $id $dname (age $((age / 60)) min)"
        act "cluster $id $dname" "/cmk/v2/clusters/$id?environment=$env_id" || failed=1; n=$((n + 1))
      else echo "keep $id $dname (age $((age / 60)) min, TTL $((ttl_seconds / 60)) min)"; fi
    elif ((age > report_seconds)); then
      echo "REPORT: cluster $id \"$dname\" in $env_id is older than a day and not named loams-ci-* (not deleted)"
    fi
  done < <(list_all clusters "/cmk/v2/clusters?environment=$env_id&page_size=100")
  while IFS= read -r item; do
    [[ -n "$item" ]] || continue
    id="$(jq -r .id <<<"$item")"; dname="$(jq -r '.spec.display_name // ""' <<<"$item")"
    created="$(jq -r '.metadata.created_at // ""' <<<"$item")"; age=$((now - $(iso_to_epoch "$created")))
    local res; res="$(jq -r '.spec.resource.id // ""' <<<"$item")"
    if [[ "$protected_clusters" == *" $res "* ]]; then echo "keep key $id: on a protected cluster"; continue; fi
    if [[ "$dname" == loams-ci-* ]] && ((age > ttl_seconds)); then
      say "expired API key $id $dname"
      act "API key $id $dname" "/iam/v2/api-keys/$id" || failed=1; n=$((n + 1))
    fi
  done < <(list_all api-keys "/iam/v2/api-keys?page_size=100")
  say "reap: $n expired resource(s)"
  return "$failed"
}

# --- up ------------------------------------------------------------------------------------
tf() { (cd "$work/root" && TF_IN_AUTOMATION=1 "$tofu" "$@"); }

write_root() {
  mkdir -p "$work/root"; chmod 700 "$work"
  [[ -s "$work/passphrase" ]] || { (umask 077; head -c 32 /dev/urandom | base64 >"$work/passphrase"); }
  sed "s#source *= *\"../../modules/confluent\"#source = \"$repo/deploy/tofu/modules/confluent\"#" \
    "$repo/deploy/tofu/examples/confluent/main.tf" >"$work/root/main.tf"
  grep -q "$repo/deploy/tofu/modules/confluent" "$work/root/main.tf" || die "could not point the root at the module"
  jq -n --arg name "$name" --arg env "$env_id" --arg sa "$CONFLUENT_CI_DATA_SA" --arg region "$region" '{
    name: $name, cloud: "aws", region: $region, environment_id: $env, cluster_type: "basic", max_ecku: 1,
    data_service_account_id: $sa, create_metrics_account: false,
    topics: {
      ($name + "-events"): {partitions: 2, retention_bytes: 104857600, retention_ms: 10800000},
      ($name + "-cdc"):    {partitions: 1, retention_bytes: 104857600, retention_ms: 10800000}
    },
    connector_bytes_per_sec: {in: 1048576, out: 1048576},
    confluent_budget_monthly_usd: 10
  }' >"$work/root/terraform.tfvars.json"
}

# with_tf_env CMD...: tofu with the passphrase and, if minted, the admin key.
with_tf_env() {
  TF_VAR_state_passphrase="$(cat "$work/passphrase")"; export TF_VAR_state_passphrase
  # shellcheck disable=SC1091 # made by up, at run time
  if [[ -s "$work/kafka-admin.env" ]]; then set -a; . "$work/kafka-admin.env"; set +a; fi
  "$@"
  unset TF_VAR_state_passphrase KAFKA_ID KAFKA_REST_ENDPOINT KAFKA_API_KEY KAFKA_API_SECRET
}

provisioner() {  # the owner of the Cloud key (the CI service account)
  local b; b="$(api GET "/iam/v2/api-keys/$CONFLUENT_CLOUD_API_KEY")"
  [[ "$(status)" == 200 ]] || die "could not read the Cloud key's owner (HTTP $(status))"
  jq -r '.spec.owner.id' <<<"$b"
}

# mint OWNER RESOURCE LABEL FILE: a Kafka key; id and secret to FILE (0600). Prints the id.
mint() {
  local owner="$1" res="$2" label="$3" file="$4" b
  b="$(api POST /iam/v2/api-keys "$(jq -cn --arg o "$owner" --arg r "$res" --arg e "$env_id" --arg n "$name-$label" \
    '{spec:{display_name:$n, description:"Loams CI (confluent-ci.sh), TTL 3h", owner:{id:$o}, resource:{id:$r, environment:$e}}}')")"
  [[ "$(status)" == 202 || "$(status)" == 200 ]] || die "minting the $label key answered HTTP $(status)"
  (umask 077; jq -r '"id=" + .id + "\nsecret=" + .spec.secret' <<<"$b" >"$file")
  rm -f "$work/body"
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then echo "::add-mask::$(sed -n 's/^secret=//p' "$file")"; fi
  sed -n 's/^id=//p' "$file"
}

up() {
  need_env
  [[ "${CONFLUENT_CI_DATA_SA:-}" =~ ^sa-[a-z0-9]+$ ]] || die "CONFLUENT_CI_DATA_SA is not an sa-... id (the persistent loams-ci-data account)"
  write_root
  if ((dry)); then
    echo "WOULD: tofu apply $name (Basic, max_ecku 1, $region, $env_id) with kafka_admin=none, mint an admin key, apply topics and ACLs, mint the data key for $CONFLUENT_CI_DATA_SA"
    return
  fi
  date -u +%s >"$work/started"
  with_tf_env tf init -input=false -no-color >/dev/null
  say "phase 1: environment, cluster"
  with_tf_env tf apply -input=false -no-color -auto-approve -var kafka_admin=none | grep -E '^(Apply|module\..*: Creation complete)' || true
  local cluster rest prov
  cluster="$(with_tf_env tf output -raw cluster_id)"; rest="$(with_tf_env tf output -raw rest_endpoint)"
  echo "$cluster" >"$work/cluster"
  say "cluster $cluster"
  prov="$(provisioner)"
  mint "$prov" "$cluster" admin "$work/admin-key" >"$work/admin-key.id"
  (umask 077; printf 'KAFKA_ID=%s\nKAFKA_REST_ENDPOINT=%s\nKAFKA_API_KEY=%s\nKAFKA_API_SECRET=%s\n' "$cluster" "$rest" \
    "$(sed -n 's/^id=//p' "$work/admin-key")" "$(sed -n 's/^secret=//p' "$work/admin-key")" >"$work/kafka-admin.env")
  say "phase 2: topics and ACLs (the admin key stays out of state)"
  local i
  for i in 1 2 3 4 5 6; do  # a new key can take a minute to work
    if with_tf_env tf apply -input=false -no-color -auto-approve | grep -E '^(Apply|Error)'; then break; fi
    ((i < 6)) || die "phase 2 did not apply"
    sleep 20
  done
  mint "$CONFLUENT_CI_DATA_SA" "$cluster" data "$work/data-key" >"$work/data-key.id"
  say "data key $(cat "$work/data-key.id") for $CONFLUENT_CI_DATA_SA"
  if [[ "${CONFLUENT_CI_VM:-1}" == 1 ]]; then
    local prefix="loams/ci-$run" short value
    for short in api-key api-secret; do
      [[ "$short" == api-key ]] && value=id || value=secret
      sed -n "s/^$value=//p" "$work/data-key" | tr -d '\n' | aws --region "$region" secretsmanager create-secret \
        --name "$prefix/confluent-confluent-$short" --secret-string file:///dev/stdin \
        --tags "Key=loams:ci-run,Value=$run" "Key=loams:ttl,Value=$((now + ttl_seconds))" --output json >/dev/null
    done
    say "wrote $prefix/confluent-confluent-api-key and -api-secret (tagged loams:ci-run=$run)"
  fi
  {
    echo "cluster_id=$cluster"
    echo "bootstrap=$(with_tf_env tf output -raw bootstrap_endpoint)"
  } >>"${GITHUB_OUTPUT:-/dev/null}"
}

# --- smoke ---------------------------------------------------------------------------------
smoke() {
  [[ -s "$work/cluster" && -s "$work/data-key" ]] || die "no cluster for run $run (run up first)"
  CONFLUENT_SMOKE_REST="$(with_tf_env tf output -raw rest_endpoint)" \
  CONFLUENT_SMOKE_CLUSTER="$(cat "$work/cluster")" \
  CONFLUENT_SMOKE_KEY_FILE="$work/data-key" \
  CONFLUENT_SMOKE_TOPIC="$name-events" \
  CONFLUENT_SMOKE_CHECKPOINTS="__loams_checkpoints" \
    "$repo/tests/byoc/confluent_smoke.sh"
}

# --- down ----------------------------------------------------------------------------------
down() {
  need_env
  local cluster="" failed=0
  [[ -s "$work/cluster" ]] && cluster="$(cat "$work/cluster")"
  if ((dry)); then echo "WOULD: tofu destroy $name, delete its keys, check $env_id lists no $name cluster"; return; fi
  if [[ -d "$work/root" && -s "$work/root/terraform.tfstate" ]]; then
    # The admin key is needed to delete topics and ACLs; mint one if up died before it did.
    if [[ -n "$cluster" && ! -s "$work/kafka-admin.env" ]]; then
      local rest; rest="$(with_tf_env tf output -raw rest_endpoint 2>/dev/null || true)"
      if [[ -n "$rest" ]]; then
        mint "$(provisioner)" "$cluster" admin "$work/admin-key" >"$work/admin-key.id"
        (umask 077; printf 'KAFKA_ID=%s\nKAFKA_REST_ENDPOINT=%s\nKAFKA_API_KEY=%s\nKAFKA_API_SECRET=%s\n' "$cluster" "$rest" \
          "$(sed -n 's/^id=//p' "$work/admin-key")" "$(sed -n 's/^secret=//p' "$work/admin-key")" >"$work/kafka-admin.env")
        sleep 30
      fi
    fi
    with_tf_env tf destroy -input=false -no-color -auto-approve | grep -E '^(Destroy|Error)' || failed=1
  fi
  # Whatever tofu could not remove: the cluster itself (its keys and ACLs go with it).
  local ids
  ids="$(list_all clusters "/cmk/v2/clusters?environment=$env_id&page_size=100" | jq -r --arg n "$name" 'select(.spec.display_name == $n) | .id')"
  for cluster in $ids; do
    [[ "$protected_clusters" != *" $cluster "* ]] || continue
    act "cluster $cluster $name" "/cmk/v2/clusters/$cluster?environment=$env_id" || failed=1
  done
  # The run's keys usually go with the cluster (a gone key answers 403 or 404).
  local f kid
  for f in data-key.id admin-key.id; do
    [[ -s "$work/$f" ]] || continue
    kid="$(cat "$work/$f")"
    api DELETE "/iam/v2/api-keys/$kid" >/dev/null
    case "$(status)" in 2*|403|404) ;; *) echo "FAILED to delete API key $kid: HTTP $(status)"; failed=1 ;; esac
  done
  if [[ "${CONFLUENT_CI_VM:-1}" == 1 ]] && command -v aws >/dev/null; then
    for f in api-key api-secret; do
      aws --region "$region" secretsmanager delete-secret --secret-id "loams/ci-$run/confluent-confluent-$f" --force-delete-without-recovery >/dev/null 2>&1 || true
    done
  fi
  # Checked by id and by name: nothing of this run is left.
  local left="" gone=0
  if [[ -n "$cluster" ]]; then
    for _ in $(seq 30); do
      api GET "/cmk/v2/clusters/$cluster?environment=$env_id" >/dev/null
      [[ "$(status)" == 404 ]] && { gone=1; break; }
      sleep 10
    done
    if ((gone)); then say "cluster $cluster: 404"; else echo "FAIL: cluster $cluster still answers HTTP $(status)"; failed=1; fi
  fi
  left="$(list_all clusters "/cmk/v2/clusters?environment=$env_id&page_size=100" | jq -r --arg n "$name" 'select(.spec.display_name == $n) | .id')"
  [[ -z "$left" ]] || { echo "FAIL: $env_id still lists $name: $left"; failed=1; }
  left="$(list_all api-keys "/iam/v2/api-keys?page_size=100" | jq -r --arg n "$name-" 'select(.spec.display_name | startswith($n)) | .id')"
  if [[ -z "$left" ]]; then say "no API key named $name-* is left"; else echo "FAIL: API keys left: $left"; failed=1; fi
  if [[ -s "$work/started" ]]; then say "run took $(( ($(date -u +%s) - $(cat "$work/started")) / 60 )) min"; fi
  rm -rf "$work"
  if ((failed == 0)); then say "down: run $run cleaned"; else die "down: something of run $run is left (the reaper retries hourly)"; fi
}

case "$cmd" in
  up) up ;;
  smoke) smoke ;;
  down) down ;;
  reap) reap ;;
  *) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
