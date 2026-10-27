#!/usr/bin/env bash
# Tests deploy/byoc/lib/loams-byoc-secrets for each cloud against mock_cloud.py.
# shellcheck disable=SC2015 # A && B || C is used on purpose: C reports a failed check
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
script="$repo/deploy/byoc/lib/loams-byoc-secrets"
tmp="$(mktemp -d "${TMPDIR:-/var/tmp}/byoc-secrets.XXXXXX")"
trap '[[ -z "${srv:-}" ]] || kill "$srv" 2>/dev/null || true; rm -rf "$tmp"' EXIT
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }

cat >"$tmp/secrets.json" <<JSON
{"aws": {"loams/t/tailnet-authkey": "tskey-auth-PLANTED-1", "loams/t/setup-token": "setup-PLANTED-2", "loams/t/api-keys": "apikeys-PLANTED-3",
         "loams/t/confluent-confluent-api-key": "CKEY-PLANTED-4", "loams/t/confluent-confluent-api-secret": "csecret-PLANTED-5\n",
         "loams/t/confluent-confluent-sr-key": "SRKEY-PLANTED-6", "loams/t/confluent-confluent-sr-secret": "srsecret-PLANTED-7"},
 "gcp": {"mock-project/loams-t-tailnet-authkey": "tskey-auth-PLANTED-1", "mock-project/loams-t-setup-token": "setup-PLANTED-2",
         "mock-project/loams-t-confluent-events-api-key": "CKEY-PLANTED-8"},
 "azure": {"loams-t-tailnet-authkey": "tskey-auth-PLANTED-1", "loams-t-setup-token": "setup-PLANTED-2", "loams-t-api-keys": "apikeys-PLANTED-3"}}
JSON
python3 -I "$here/mock_cloud.py" "$tmp/port" "$tmp/secrets.json" &
srv=$!
for _ in $(seq 50); do [[ -s "$tmp/port" ]] && break; sleep 0.1; done
port="$(cat "$tmp/port")"
base="http://127.0.0.1:$port"

stack_for() {  # fresh stack dir with an .env
  local d="$tmp/stack-$1"
  mkdir -p "$d/secrets"
  printf 'LOAMS_VERSION=0.1.1\nTS_AUTHKEY=\nLOAMS_SETUP_TOKEN=\nOTHER=keep\n' >"$d/.env"
  echo "$d"
}
run() {  # cloud prefix extra-env...
  local cloud="$1" prefix="$2"; shift 2
  local stack; stack="$(stack_for "$cloud")"
  cat >"$tmp/config-$cloud.env" <<CFG
LOAMS_BYOC_CLOUD=$cloud
SECRET_PREFIX=$prefix
REGION=us-east-1
AZURE_VAULT=mock-vault
CONFLUENT_INTEGRATIONS=${INTEGRATIONS:-}
CFG
  env LOAMS_BYOC_CONFIG="$tmp/config-$cloud.env" LOAMS_BYOC_STACK="$stack" \
    LOAMS_BYOC_IMDS_URL="$base/$cloud" LOAMS_BYOC_METADATA_URL="$base/$cloud" \
    LOAMS_BYOC_SECRETS_URL="$base/$cloud-${SM:?}" "$@" "$script" ${OPT:+"$OPT"} "$cloud" >"$tmp/out-$cloud" 2>&1
}

check_clean_output() {  # no planted value in what the script printed
  if grep -q PLANTED "$tmp/out-$1"; then bad "$1: a secret value was printed"; else ok "$1: no secret value printed"; fi
}
check_env() {
  local d="$tmp/stack-$1"
  grep -q '^TS_AUTHKEY=tskey-auth-PLANTED-1$' "$d/.env" && grep -q '^LOAMS_SETUP_TOKEN=setup-PLANTED-2$' "$d/.env" \
    && grep -q '^OTHER=keep$' "$d/.env" && ok "$1: .env has the tailnet key and setup token, other keys kept" \
    || bad "$1: .env wrong: $(sed 's/=.*/=…/' "$d/.env" | tr '\n' ' ')"
  [[ "$(stat -c %a "$d/.env")" == 600 ]] && ok "$1: .env is mode 600" || bad "$1: .env mode $(stat -c %a "$d/.env")"
}

SM=sm run aws loams/t && check_env aws && check_clean_output aws || { bad "aws run failed: $(cat "$tmp/out-aws")"; }
[[ "$(cat "$tmp/stack-aws/secrets/api-keys")" == apikeys-PLANTED-3 ]] && ok "aws: optional api-keys written" || bad "aws: api-keys"
SM=sm run gcp loams/t && check_env gcp && check_clean_output gcp || bad "gcp run failed: $(cat "$tmp/out-gcp")"
[[ ! -e "$tmp/stack-gcp/secrets/api-keys" ]] && ok "gcp: absent api-keys skipped" || bad "gcp: api-keys should not exist"
SM=kv run azure loams/t && check_env azure && check_clean_output azure || bad "azure run failed: $(cat "$tmp/out-azure")"
SM=kv run azure loams/t AZURE_CLIENT_ID=11111111-2222-3333-4444-555555555555 && ok "azure: client id accepted" || bad "azure client id"

# Confluent integrations (CC1 Task 6, D1827): files in the connect role's secret store
mode_of() { stat -c %a "$1" 2>/dev/null || echo missing; }
d="$tmp/stack-aws/secrets/connect/0/confluent-confluent"
if INTEGRATIONS=confluent SM=sm run aws loams/t; then
  [[ "$(cat "$d/username")" == CKEY-PLANTED-4 && "$(cat "$d/password")" == csecret-PLANTED-5 && "$(cat "$d/sr-username")" == SRKEY-PLANTED-6 && "$(cat "$d/sr-password")" == srsecret-PLANTED-7 ]] \
    && ok "fetch_writes_confluent_files (aws: username, password, Schema Registry)" || bad "aws: confluent files wrong"
  [[ "$(tail -c1 "$d/password" | od -An -c | tr -d ' ')" != '\n' ]] && ok "aws: confluent values have no trailing newline" || bad "aws: trailing newline kept"
  [[ "$(mode_of "$d/username")$(mode_of "$d/password")" == 600600 && "$(mode_of "$d")$(mode_of "$tmp/stack-aws/secrets/connect")" == 700700 ]] \
    && ok "fetch_writes_confluent_files_0600 (files 600, directories 700)" || bad "aws: modes $(mode_of "$d/password") $(mode_of "$d")"
  [[ ! -e "$d/metrics-key" ]] && ok "aws: absent optional metrics key skipped" || bad "aws: metrics-key should not exist"
  ! grep -qE 'PLANTED-[4-7]|CONFLUENT' "$tmp/stack-aws/.env" && ok "env_has_no_confluent_value" || bad "aws: a Confluent value reached .env"
  check_clean_output aws
else bad "aws with a Confluent integration failed: $(cat "$tmp/out-aws")"; fi
if INTEGRATIONS="events" SM=sm run gcp loams/t; then
  g="$tmp/stack-gcp/secrets/connect/0/confluent-events"
  [[ "$(cat "$g/username")" == CKEY-PLANTED-8 && ! -e "$g/password" ]] && grep -q 'loams-t-confluent-events-api-secret is not in the secret store yet' "$tmp/out-gcp" \
    && ok "missing_optional_confluent_secret_is_not_fatal (gcp: named, stack continues)" || bad "gcp: missing api-secret: $(cat "$tmp/out-gcp")"
  check_clean_output gcp
else bad "gcp: a missing Confluent secret must not fail the fetch: $(cat "$tmp/out-gcp")"; fi
# a key removed from the store is removed from the file store on the next fetch
mkdir -p "$tmp/stack-gcp/secrets/connect/0/confluent-events" && printf old >"$tmp/stack-gcp/secrets/connect/0/confluent-events/password"
INTEGRATIONS="events" SM=sm run gcp loams/t || true
[[ ! -e "$tmp/stack-gcp/secrets/connect/0/confluent-events/password" ]] && ok "gcp: a secret gone from the store is removed from the files" || bad "gcp: stale password kept"
if INTEGRATIONS="ok,Bad_Name" SM=sm run aws loams/t; then bad "aws: a bad integration name should fail"; else
  grep -q "'Bad_Name' is not an integration name" "$tmp/out-aws" && ok "aws: a bad integration name is refused" || bad "aws: $(cat "$tmp/out-aws")"; fi
if INTEGRATIONS="confluent" SM=sm run aws loams/t CONNECT_NAMESPACE=7; then
  [[ -s "$tmp/stack-aws/secrets/connect/7/confluent-confluent/password" ]] && ok "aws: CONNECT_NAMESPACE chooses the namespace directory" || bad "aws: namespace 7"
else bad "aws: CONNECT_NAMESPACE=7 failed: $(cat "$tmp/out-aws")"; fi

# --confluent-only (the 5-minute timer and `systemctl reload loams-byoc`): the
# Confluent files only; .env is left alone, and the required secrets are not read.
if INTEGRATIONS=confluent OPT=--confluent-only SM=sm run aws loams/t; then
  d="$tmp/stack-aws/secrets/connect/0/confluent-confluent"
  [[ "$(cat "$d/password")" == csecret-PLANTED-5 ]] && ok "confluent_only_refreshes_the_files" || bad "aws --confluent-only: files"
  grep -q '^TS_AUTHKEY=$' "$tmp/stack-aws/.env" && grep -q '^LOAMS_SETUP_TOKEN=$' "$tmp/stack-aws/.env" \
    && ok "confluent_only_leaves_env_alone" || bad "aws --confluent-only changed .env"
  check_clean_output aws
else bad "aws --confluent-only failed: $(cat "$tmp/out-aws")"; fi
# even with a prefix that has no tailnet key: --confluent-only never reads it
if INTEGRATIONS=confluent OPT=--confluent-only SM=sm run aws loams/other; then
  grep -q 'loams/other/confluent-confluent-api-key is not in the secret store yet' "$tmp/out-aws" \
    && ok "confluent_only_skips_required_secrets" || bad "aws --confluent-only: $(cat "$tmp/out-aws")"
else bad "aws --confluent-only must not need the tailnet key: $(cat "$tmp/out-aws")"; fi
# no integration: a no-op that reads nothing (no metadata service needed)
if INTEGRATIONS="" OPT=--confluent-only SM=sm run aws loams/t LOAMS_BYOC_IMDS_URL=http://127.0.0.1:9; then
  ok "confluent_only_without_integrations_is_a_no_op"
else bad "aws --confluent-only without integrations: $(cat "$tmp/out-aws")"; fi

# a missing required secret names it and fails
if SM=sm run aws loams/other; then bad "aws: a missing secret should fail"; else
  grep -q 'missing secret loams/other/tailnet-authkey' "$tmp/out-aws" && ok "aws: missing secret named" || bad "aws: message: $(cat "$tmp/out-aws")"; fi
if SM=sm run gcp loams/other; then bad "gcp: a missing secret should fail"; else
  grep -q 'missing secret loams-other-tailnet-authkey' "$tmp/out-gcp" && ok "gcp: missing secret named" || bad "gcp: message: $(cat "$tmp/out-gcp")"; fi
if SM=kv run azure loams/other; then bad "azure: a missing secret should fail"; else
  grep -q 'missing secret loams-other-tailnet-authkey' "$tmp/out-azure" && ok "azure: missing secret named" || bad "azure: message: $(cat "$tmp/out-azure")"; fi

# no metadata service: fails with the identity hint, not a hang
if SM=sm run aws loams/t LOAMS_BYOC_IMDS_URL=http://127.0.0.1:9; then bad "aws: no IMDS should fail"; else
  grep -q 'IMDSv2 did not answer' "$tmp/out-aws" && ok "aws: unreachable IMDS explained" || bad "aws: $(cat "$tmp/out-aws")"; fi

exit "$fail"
