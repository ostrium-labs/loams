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
{"aws": {"loams/t/tailnet-authkey": "tskey-auth-PLANTED-1", "loams/t/setup-token": "setup-PLANTED-2", "loams/t/api-keys": "apikeys-PLANTED-3"},
 "gcp": {"mock-project/loams-t-tailnet-authkey": "tskey-auth-PLANTED-1", "mock-project/loams-t-setup-token": "setup-PLANTED-2"},
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
CFG
  env LOAMS_BYOC_CONFIG="$tmp/config-$cloud.env" LOAMS_BYOC_STACK="$stack" \
    LOAMS_BYOC_IMDS_URL="$base/$cloud" LOAMS_BYOC_METADATA_URL="$base/$cloud" \
    LOAMS_BYOC_SECRETS_URL="$base/$cloud-${SM:?}" "$@" "$script" "$cloud" >"$tmp/out-$cloud" 2>&1
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
