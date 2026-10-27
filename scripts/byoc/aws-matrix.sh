#!/usr/bin/env bash
# The nightly AWS row of the BYOC matrix (BY0 Task 9, D1380): bootstrap the
# persistent pieces once, then up, smoke, down, and a reaper that deletes what a
# crashed run left behind. Runs with an AWS role the owner creates (see the plan's
# owner checklist); nothing here holds or asks for a key.
#
#   scripts/byoc/aws-matrix.sh bootstrap [--dry-run]   once, by the owner, with their own CLI session
#   scripts/byoc/aws-matrix.sh up        [--dry-run]   launch a t3.large from deploy/byoc/aws/cloud-init.yaml
#   scripts/byoc/aws-matrix.sh smoke                   exposure scan of the instance's public IP
#   scripts/byoc/aws-matrix.sh down      [--dry-run]   terminate this run's resources
#   scripts/byoc/aws-matrix.sh reap      [--dry-run]   delete everything whose loams:ttl is past
#
# Environment: AWS_REGION (default us-east-1), GITHUB_RUN_ID (the run's tag),
# TS_AUTHKEY (an ephemeral, preauthorised Tailscale key, only for `up`),
# LOAMS_CI_VERSION (the release, default 0.1.1), AWS_MATRIX_CONFLUENT_INTEGRATIONS
# (integrations whose Confluent keys the VM fetches, CC1 Task 10), AWS_MATRIX_FIXTURES (a directory
# of recorded `describe-*` JSON for tests: no AWS call is made).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
region="${AWS_REGION:-us-east-1}"
run="${GITHUB_RUN_ID:-local}"
version="${LOAMS_CI_VERSION:-0.1.1}"
now="${AWS_MATRIX_NOW:-$(date +%s)}"
ttl_seconds=$((3 * 3600))
cmd="${1:-}"; shift || true
dry=0
for a in "$@"; do case "$a" in --dry-run) dry=1 ;; *) echo "aws-matrix.sh: unknown option $a" >&2; exit 2 ;; esac; done

say() { echo "==> $*"; }
# aws wrapper: recorded JSON when AWS_MATRIX_FIXTURES is set, else the CLI.
aws_json() {  # fixture-name -- aws args...
  local name="$1"; shift
  if [[ -n "${AWS_MATRIX_FIXTURES:-}" ]]; then cat "$AWS_MATRIX_FIXTURES/$name.json"; else aws --region "$region" "$@" --output json; fi
}
act() {  # run a mutating aws command, or print it with --dry-run or fixtures
  if ((dry)) || [[ -n "${AWS_MATRIX_FIXTURES:-}" ]]; then echo "WOULD: aws $*"; else aws --region "$region" "$@"; fi
}

# --- reap ---------------------------------------------------------------------
reap() {
  local n=0
  # Instances tagged with a ttl in the past, and not persistent.
  while IFS=$'\t' read -r id ttl; do
    [[ -n "$id" ]] || continue
    if ((ttl < now)); then say "expired instance $id (ttl $ttl)"; act ec2 terminate-instances --instance-ids "$id"; n=$((n + 1)); else echo "keep $id (ttl in $((ttl - now))s)"; fi
  done < <(aws_json describe-instances ec2 describe-instances --filters Name=tag-key,Values=loams:ttl Name=instance-state-name,Values=pending,running,stopping,stopped \
    | jq -r '.Reservations[].Instances[] | select(([.Tags[]? | select(.Key=="loams:persistent" and .Value=="true")] | length) == 0)
             | [.InstanceId, ((.Tags[] | select(.Key=="loams:ttl") | .Value) // "0")] | @tsv')
  # Secrets with an expired ttl.
  while IFS=$'\t' read -r name ttl; do
    [[ -n "$name" ]] || continue
    if ((ttl < now)); then say "expired secret $name (ttl $ttl)"; act secretsmanager delete-secret --secret-id "$name" --force-delete-without-recovery; n=$((n + 1)); else echo "keep $name (ttl in $((ttl - now))s)"; fi
  done < <(aws_json list-secrets secretsmanager list-secrets --filters Key=tag-key,Values=loams:ttl \
    | jq -r '.SecretList[] | select(([.Tags[]? | select(.Key=="loams:persistent" and .Value=="true")] | length) == 0)
             | [.Name, ((.Tags[] | select(.Key=="loams:ttl") | .Value) // "0")] | @tsv')
  # Report, never delete: instances older than a day with no loams: tag at all.
  local cutoff; cutoff="$(date -u -d "@$((now - 86400))" +%Y-%m-%dT%H:%M:%S)"
  while IFS=$'\t' read -r id launched; do
    [[ -n "$id" ]] || continue
    echo "UNTAGGED: instance $id launched $launched has no loams: tag (not deleted)"
  done < <(aws_json describe-instances-all ec2 describe-instances --filters Name=instance-state-name,Values=pending,running,stopping,stopped \
    | jq -r --arg cutoff "$cutoff" '.Reservations[].Instances[]
             | select(([.Tags[]? | select(.Key | startswith("loams:"))] | length) == 0 and (.LaunchTime[0:19] < $cutoff))
             | [.InstanceId, .LaunchTime] | @tsv')
  say "reap: $n expired resource(s)"
}

# --- up / smoke / down ----------------------------------------------------------
ci_tags=("Key=loams:ci-run,Value=$run" "Key=loams:ttl,Value=$((now + ttl_seconds))")

bootstrap() {
  local account bucket
  account="$(aws sts get-caller-identity --query Account --output text)"
  bucket="loams-ci-$account"
  say "bootstrap in account $account, $region (persistent: KMS alias/loams-ci, bucket $bucket, profile loams-ci-instance)"
  if ((dry)); then
    echo "WOULD: create KMS key alias/loams-ci, bucket $bucket (versioned, public access blocked), role and instance profile loams-ci-instance; all tagged loams:persistent=true"
    return
  fi
  local key_arn
  key_arn="$(aws --region "$region" kms create-key --description "Loams CI" --tags TagKey=loams:persistent,TagValue=true --query KeyMetadata.Arn --output text)"
  aws --region "$region" kms create-alias --alias-name alias/loams-ci --target-key-id "$key_arn"
  local loc=()
  [[ "$region" == us-east-1 ]] || loc=(--create-bucket-configuration "LocationConstraint=$region")
  aws --region "$region" s3api create-bucket --bucket "$bucket" "${loc[@]}"
  aws --region "$region" s3api put-public-access-block --bucket "$bucket" --public-access-block-configuration BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true
  aws --region "$region" s3api put-bucket-versioning --bucket "$bucket" --versioning-configuration Status=Enabled
  aws --region "$region" s3api put-bucket-tagging --bucket "$bucket" --tagging 'TagSet=[{Key=loams:persistent,Value=true}]'
  aws iam create-role --role-name loams-ci-instance --assume-role-policy-document "file://$repo/deploy/byoc/aws/instance-trust-policy.json" --tags Key=loams:persistent,Value=true
  aws iam create-instance-profile --instance-profile-name loams-ci-instance --tags Key=loams:persistent,Value=true
  aws iam add-role-to-instance-profile --instance-profile-name loams-ci-instance --role-name loams-ci-instance
  echo "created: alias/loams-ci, s3://$bucket, instance profile loams-ci-instance"
  echo "Next: attach the instance policy for this bucket and key (deploy/byoc/aws/instance-policy.json, with SECRET_PREFIX=loams/ci)."
}

up() {
  local account bucket ami sg userdata secret id
  account="$(aws sts get-caller-identity --query Account --output text)"
  bucket="loams-ci-$account"
  [[ -n "${TS_AUTHKEY:-}" ]] || { echo "aws-matrix.sh up: TS_AUTHKEY (an ephemeral key) is required" >&2; exit 2; }
  secret="loams/ci-$run"
  userdata="$(mktemp "$repo/.aws-matrix-ud.XXXXXX")"
  trap 'rm -f "$userdata"' RETURN
  "$repo/deploy/byoc/render.sh" aws --bucket "$bucket" --region "$region" --version "$version" --install "ci-$run" --secret-prefix "$secret" \
    --confluent-integrations "${AWS_MATRIX_CONFLUENT_INTEGRATIONS:-}" --gzip >"$userdata"
  if ((dry)); then echo "WOULD: create secrets $secret/tailnet-authkey and setup-token, a security group with no inbound rule, and run a t3.large with the profile loams-ci-instance"; return; fi
  aws --region "$region" secretsmanager create-secret --name "$secret/tailnet-authkey" --secret-string "$TS_AUTHKEY" --tags "${ci_tags[@]}" >/dev/null
  aws --region "$region" secretsmanager create-secret --name "$secret/setup-token" --secret-string "$(openssl rand -hex 32)" --tags "${ci_tags[@]}" >/dev/null
  ami="$(aws --region "$region" ssm get-parameter --name /aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id --query Parameter.Value --output text)"
  sg="$(aws --region "$region" ec2 create-security-group --group-name "loams-ci-$run" --description "Loams CI: no inbound" --query GroupId --output text --tag-specifications "ResourceType=security-group,Tags=[{Key=loams:ci-run,Value=$run},{Key=loams:ttl,Value=$((now + ttl_seconds))}]")"
  id="$(aws --region "$region" ec2 run-instances --image-id "$ami" --instance-type t3.large \
    --iam-instance-profile Name=loams-ci-instance --security-group-ids "$sg" --associate-public-ip-address \
    --metadata-options HttpTokens=required,HttpPutResponseHopLimit=2,HttpEndpoint=enabled \
    --block-device-mappings 'DeviceName=/dev/sda1,Ebs={VolumeSize=60,VolumeType=gp3,Encrypted=true}' \
    --user-data "fileb://$userdata" \
    --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=loams-ci-$run},{Key=loams:ci-run,Value=$run},{Key=loams:ttl,Value=$((now + ttl_seconds))}]" \
    --query 'Instances[0].InstanceId' --output text)"
  echo "instance=$id" >>"${GITHUB_OUTPUT:-/dev/stdout}"
  say "launched $id (ttl $((ttl_seconds / 3600))h)"
}

public_ip() {
  aws --region "$region" ec2 describe-instances --filters "Name=tag:loams:ci-run,Values=$run" Name=instance-state-name,Values=running \
    --query 'Reservations[0].Instances[0].PublicIpAddress' --output text
}

smoke() {
  local ip; ip="$(public_ip)"
  [[ -n "$ip" && "$ip" != None ]] || { echo "no running instance for run $run" >&2; exit 1; }
  "$repo/tests/byoc/exposure.sh" "$ip"
}

down() {
  local ids secrets sgs id sg s
  ids="$(aws_json down-instances ec2 describe-instances --filters "Name=tag:loams:ci-run,Values=$run" Name=instance-state-name,Values=pending,running,stopping,stopped \
    | jq -r '.Reservations[].Instances[].InstanceId')"
  for id in $ids; do act ec2 terminate-instances --instance-ids "$id"; done
  # shellcheck disable=SC2086 # ids is a whitespace-separated list of instance ids
  if [[ -n "$ids" && -z "${AWS_MATRIX_FIXTURES:-}" && $dry -eq 0 ]]; then aws --region "$region" ec2 wait instance-terminated --instance-ids $ids; fi
  sgs="$(aws_json down-sgs ec2 describe-security-groups --filters "Name=tag:loams:ci-run,Values=$run" | jq -r '.SecurityGroups[].GroupId')"
  for sg in $sgs; do act ec2 delete-security-group --group-id "$sg"; done
  secrets="$(aws_json down-secrets secretsmanager list-secrets --filters Key=tag-key,Values=loams:ci-run | jq -r --arg run "$run" '.SecretList[] | select([.Tags[]? | select(.Key=="loams:ci-run" and .Value==$run)] | length > 0) | .Name')"
  for s in $secrets; do act secretsmanager delete-secret --secret-id "$s" --force-delete-without-recovery; done
  say "down: run $run cleaned"
}

case "$cmd" in
  bootstrap) bootstrap ;;
  up) up ;;
  smoke) smoke ;;
  down) down ;;
  reap) reap ;;
  *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
