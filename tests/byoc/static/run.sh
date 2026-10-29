#!/usr/bin/env bash
# Static tests for deploy/byoc (BY0 Tasks 3-5): no cloud account, no network.
#
# Needs: bash, jq, python3 with PyYAML, curl >= 7.75 (--aws-sigv4).
# Optional (CI sets them): SHELLCHECK=<path> and CLOUD_INIT_SRC=<a cloud-init
# source tree, with its Python dependencies installed> for the schema check.
set -uo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
byoc="$repo/deploy/byoc"
tmp="$(mktemp -d "${TMPDIR:-/var/tmp}/byoc-static.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
t() { local name="$1"; shift; if "$@" >"$tmp/log" 2>&1; then ok "$name"; else bad "$name"; sed 's/^/     /' "$tmp/log" | head -20; fi; }

# render each cloud with valid inputs
declare -A render_args=(
  [aws]="--bucket my-bkt --region us-east-1 --install t"
  [gcp]="--bucket my-bkt --project my-project --install t"
  [azure]="--container loams --storage-account myacct --vault my-vault --install t"
)
for c in aws gcp azure; do
  # shellcheck disable=SC2086
  "$byoc/render.sh" "$c" ${render_args[$c]} >"$tmp/$c.yaml" 2>"$tmp/$c.err" || { bad "render $c: $(cat "$tmp/$c.err")"; }
done

t "cloud_init_files_are_generated_and_current" "$byoc/gen-cloud-init.sh" --check

if [[ -n "${CLOUD_INIT_SRC:-}" ]]; then
  for c in aws gcp azure; do
    t "cloud_init_validates_$c (template)" bash -c "cd '$CLOUD_INIT_SRC' && python3 -m cloudinit.cmd.main schema --config-file '$byoc/$c/cloud-init.yaml' 2>&1 | grep -q '^Valid schema'"
    t "cloud_init_validates_$c (rendered)" bash -c "cd '$CLOUD_INIT_SRC' && python3 -m cloudinit.cmd.main schema --config-file '$tmp/$c.yaml' 2>&1 | grep -q '^Valid schema'"
  done
else
  echo "skip cloud_init_validates (CLOUD_INIT_SRC is not set)"
fi

# the rendered files carry the same scripts the repo does, and the release key
wf() { python3 -I "$here/write_file.py" "$@"; }
for c in aws gcp azure; do
  t "scripts_in_${c}_match_lib" bash -c "
    diff <('$here/write_file.py' '$byoc/$c/cloud-init.yaml' /usr/local/bin/loams-byoc-secrets) '$byoc/lib/loams-byoc-secrets' &&
    diff <('$here/write_file.py' '$byoc/$c/cloud-init.yaml' /usr/local/bin/loams-byoc-bootstrap) '$byoc/lib/loams-byoc-bootstrap' &&
    diff <('$here/write_file.py' '$byoc/$c/cloud-init.yaml' /etc/systemd/system/loams-byoc-confluent.service) '$byoc/lib/loams-byoc-confluent.service' &&
    diff <('$here/write_file.py' '$byoc/$c/cloud-init.yaml' /etc/systemd/system/loams-byoc-confluent.timer) '$byoc/lib/loams-byoc-confluent.timer' &&
    diff <('$here/write_file.py' '$byoc/$c/cloud-init.yaml' /etc/loams-byoc/loams-release-key.asc) '$repo/docs/release/loams-release-key.asc'"
done

# never public: no inbound rule in any file or command under deploy/byoc or the guides
t "cloud_init_has_no_inbound_rules" bash -c "
  ! grep -rnE 'authorize-security-group-ingress|ingress.*0\\.0\\.0\\.0/0|--allow +(tcp|udp|all)[^ ]* .*0\\.0\\.0\\.0/0|source-ranges[= ]+0\\.0\\.0\\.0/0|--nsg-rule +(SSH|RDP|HTTP|HTTPS)|priority.*Allow.*Inbound|ports?:.*(0\\.0\\.0\\.0)' '$byoc' '$repo/docs/guides/byoc' 2>/dev/null"
t "the_vm_never_listens_on_a_public_port (compose binds are loopback)" bash -c "
  ! grep -nE '^\s+- \"?(0\\.0\\.0\\.0:)?[0-9]+:[0-9]+' '$repo/deploy/release/compose.yaml'"

# no static cloud key lands on the host: the .env and engine.env bootstrap would write
for c in aws gcp azure; do
  wf "$tmp/$c.yaml" /etc/loams-byoc/config.env >"$tmp/$c.config"
  LOAMS_BYOC_CONFIG="$tmp/$c.config" "$byoc/lib/loams-byoc-bootstrap" --render-env-only >"$tmp/$c.rendered" 2>&1
done
t "aws_env_has_no_access_key" bash -c "! grep -E 'AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|AWS_SESSION_TOKEN' '$tmp/aws.rendered' && grep -q '^LOAMS_BUCKET=s3://my-bkt/loams\$' '$tmp/aws.rendered' && grep -q '^AWS_REGION=us-east-1\$' '$tmp/aws.rendered'"
t "gcp_env_has_no_service_account_key" bash -c "! grep -E 'GOOGLE_SERVICE_ACCOUNT|GOOGLE_APPLICATION_CREDENTIALS|private_key' '$tmp/gcp.rendered' && grep -q '^LOAMS_BUCKET=gs://my-bkt/loams\$' '$tmp/gcp.rendered'"
t "azure_env_has_no_account_key" bash -c "! grep -E 'AZURE_STORAGE_ACCOUNT_KEY|AZURE_STORAGE_SAS|AZURE_STORAGE_CLIENT_SECRET|AZURE_STORAGE_KEY' '$tmp/azure.rendered' && grep -q '^LOAMS_BUCKET=az://loams/loams\$' '$tmp/azure.rendered' && grep -q '^AZURE_STORAGE_ACCOUNT_NAME=myacct\$' '$tmp/azure.rendered' && grep -q '^AZURE_CREDENTIAL_TYPE=managed_identity\$' '$tmp/azure.rendered'"
t "auth_mode_is_tailscale_without_connectors" bash -c "grep -q '^LOAMS_AUTH_MODE=tailscale\$' '$tmp/aws.rendered'"
t "auth_mode_adds_apikey_for_connectors (CC1: the connect socket needs an API key)" bash -c "
  { cat '$tmp/aws.config'; echo 'LOAMS_PROFILES=tikv,connectors'; } >'$tmp/conn.config' &&
  LOAMS_BYOC_CONFIG='$tmp/conn.config' '$byoc/lib/loams-byoc-bootstrap' --render-env-only | grep -q '^LOAMS_AUTH_MODE=tailscale,apikey\$' &&
  { cat '$tmp/aws.config'; echo 'CONFLUENT_INTEGRATIONS=confluent'; } >'$tmp/int.config' &&
  LOAMS_BYOC_CONFIG='$tmp/int.config' '$byoc/lib/loams-byoc-bootstrap' --render-env-only | grep -q '^LOAMS_AUTH_MODE=tailscale,apikey\$'"
t "render_env_never_holds_a_secret_value" bash -c "! grep -h -E '^(TS_AUTHKEY|LOAMS_SETUP_TOKEN)=' '$tmp'/*.rendered | grep -qvE '=<from secret store>\$'"
t "bootstrap_refuses_a_bucket_of_the_wrong_cloud" bash -c "
  sed 's#s3://my-bkt#gs://my-bkt#' '$tmp/aws.config' >'$tmp/bad.config';
  ! LOAMS_BYOC_CONFIG='$tmp/bad.config' '$byoc/lib/loams-byoc-bootstrap' --render-env-only"

# render.sh refuses values that could break out of the config or the script
t "render_rejects_injection" bash -c "
  for bad in 'x;rm -rf /' 'a b' '\$(id)' 'a\`id\`' 'A_B' ''; do
    ! '$byoc/render.sh' aws --bucket \"\$bad\" --region us-east-1 >/dev/null 2>&1 || { echo \"accepted bucket '\$bad'\"; exit 1; }
  done
  ! '$byoc/render.sh' aws --bucket b --region us-east-1 --profiles 'tikv;x' >/dev/null 2>&1 &&
  ! '$byoc/render.sh' aws --bucket b --region us-east-1 --version 'latest' >/dev/null 2>&1 &&
  ! '$byoc/render.sh' aws --bucket b >/dev/null 2>&1 &&
  ! '$byoc/render.sh' gcp --bucket b --project 'x' >/dev/null 2>&1 &&
  ! '$byoc/render.sh' azure --container c --storage-account 'Bad_Name' --vault v >/dev/null 2>&1"
t "render_pins_a_version_and_never_latest" bash -c "! grep -q 'latest' <(grep -v '^#' '$tmp/aws.yaml' | grep -E 'LOAMS_VERSION') && grep -q 'LOAMS_VERSION=0.1.2' '$tmp/aws.yaml'"
# Confluent (CC1 Task 6, D1827): user data names the integrations, never a key
t "rendered_user_data_has_no_confluent_value" bash -c "
  out=\$('$byoc/render.sh' aws --bucket my-bkt --region us-east-1 --confluent-integrations confluent,events) &&
  grep -q '^      CONFLUENT_INTEGRATIONS=confluent,events\$' <<<\"\$out\" &&
  ! grep -E '^\s*(export\s+)?(CONFLUENT|KAFKA)_[A-Z_]*=' <<<\"\$out\" | grep -vqE '^\s*CONFLUENT_INTEGRATIONS=[a-z0-9,-]*\$' &&
  ! grep -qiE 'api[-_]?secret=|sasl\.password' <<<\"\$out\""
t "render_rejects_bad_confluent_integration" bash -c "! '$byoc/render.sh' aws --bucket my-bkt --region us-east-1 --confluent-integrations 'a;b' >/dev/null 2>&1"
t "render_gzip_roundtrips" bash -c "diff <('$byoc/render.sh' aws --bucket my-bkt --region us-east-1 --install t --gzip | gunzip) '$tmp/aws.yaml'"
t "aws_gzip_fits_the_16KB_user_data_limit" bash -c "[ \$('$byoc/render.sh' aws --bucket my-bkt --region us-east-1 --gzip | wc -c) -lt 16384 ]"
t "azure_custom_data_fits_64KB" bash -c "[ \$(wc -c <'$tmp/azure.yaml') -lt 65536 ]"
t "gcp_metadata_fits_256KB" bash -c "[ \$(wc -c <'$tmp/gcp.yaml') -lt 262144 ]"

# the instance policy is scoped: every resource names the bucket, key or secret prefix
t "instance_policy_is_scoped" bash -c "
  p='$byoc/aws/instance-policy.json'
  jq -e '[.Statement[] | .Resource | (if type==\"array\" then .[] else . end)] | all(. != \"*\" and (test(\"BUCKET|KEY_ID|SECRET_PREFIX\")))' \$p &&
  jq -e '[.Statement[] | .Action | (if type==\"array\" then .[] else . end)] | all(test(\"^(s3:(GetObject|PutObject|DeleteObject|AbortMultipartUpload|ListMultipartUploadParts|ListBucket)|kms:(GenerateDataKey|Decrypt)|secretsmanager:GetSecretValue)\$\"))' \$p &&
  jq -e '[.Statement[] | .Effect] | all(. == \"Allow\")' \$p &&
  jq -e '.Statement | length == 4' \$p"
t "aws_trust_policy_is_ec2_only" jq -e '.Statement | length == 1 and .[0].Principal.Service == "ec2.amazonaws.com"' "$byoc/aws/instance-trust-policy.json"

# the secrets fetcher against mocks of IMDS, Secrets Manager, the metadata server, Key Vault
t "secrets_fetch_all_clouds" "$here/test_secrets.sh"
# the bundle half of the bootstrap against a local, test-signed release
t "bootstrap_verifies_signature_and_checksum" "$here/test_bootstrap.sh"
t "exposure_scan_finds_open_ports" "$here/test_exposure.sh"
t "aws_matrix_reaper_and_nightly" "$here/test_reaper.sh"
# the Confluent region and price tables (CC1 Task 5)
t "confluent_tables" "$here/test_confluent_tables.sh"
# the Confluent nightly's reaper and workflow shape (CC1 Task 10)
t "confluent_ci_reaper_and_nightly" "$here/test_confluent_ci.sh"

# install.sh --cloud (BY0 Task 6): the env it would write, against golden files
inst="$repo/deploy/release/byoc/install.sh"
g="$here/golden"
t "install_cloud_aws_renders_bucket_and_region" bash -c "diff <('$inst' --render-env-only --cloud aws --bucket s3://my-bkt/loams --region us-east-1) '$g/install_aws.golden'"
t "install_cloud_gcp_renders_bucket" bash -c "diff <('$inst' --render-env-only --cloud gcp --bucket gs://my-bkt/loams --project my-project) '$g/install_gcp.golden'"
t "install_cloud_azure_renders_account" bash -c "diff <('$inst' --render-env-only --cloud azure --bucket az://loams/loams --storage-account myacct --vault my-vault --client-id 11111111-2222-3333-4444-555555555555 --profiles tikv --dir prod) '$g/install_azure.golden'"
t "install_cloud_env_has_no_cloud_key" bash -c "! cat '$g'/install_*.golden | grep -E 'AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|GOOGLE_SERVICE_ACCOUNT_KEY|AZURE_STORAGE_ACCOUNT_KEY|AZURE_STORAGE_SAS'"
t "install_cloud_refuses_a_mismatched_or_unsafe_bucket" bash -c "
  ! '$inst' --render-env-only --cloud aws --bucket gs://b --region us-east-1 &&
  ! '$inst' --render-env-only --cloud aws --bucket 's3://b;id' --region us-east-1 &&
  ! '$inst' --render-env-only --cloud aws --bucket s3://b &&
  ! '$inst' --render-env-only --cloud azure --bucket az://c/loams --vault v &&
  ! '$inst' --render-env-only --bucket s3://b --region us-east-1"

# install.sh --kafka (CC1 Task 8): the .env keys and instance files it would write
t "install_kafka_flag_matrix" "$here/test_install_kafka.sh"

# the guides name only files that exist, and open no port
t "guides_reference_existing_files" "$here/guides_reference_existing_files.sh"

# the linter
sc="${SHELLCHECK:-$(command -v shellcheck || true)}"
if [[ -n "$sc" ]]; then
  t "shellcheck" "$sc" -x "$byoc/render.sh" "$byoc/gen-cloud-init.sh" "$byoc/lib/loams-byoc-bootstrap" "$byoc/lib/loams-byoc-secrets" "$inst" "$here/run.sh" "$here/test_secrets.sh" "$here/test_bootstrap.sh" "$here/test_exposure.sh" "$here/test_reaper.sh" "$repo/scripts/byoc/aws-matrix.sh" "$repo/scripts/byoc/confluent-tables.sh" "$here/test_confluent_tables.sh" "$repo/scripts/byoc/confluent-ci.sh" "$repo/tests/byoc/confluent_smoke.sh" "$here/test_confluent_ci.sh" "$repo/tests/byoc/exposure.sh" "$here/guides_reference_existing_files.sh" "$here/test_install_kafka.sh"
else
  echo "skip shellcheck (not installed)"
fi

exit "$fail"
