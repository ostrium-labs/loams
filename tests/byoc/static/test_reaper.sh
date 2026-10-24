#!/usr/bin/env bash
# The AWS matrix's reaper and teardown against recorded describe-* JSON (no AWS call).
# shellcheck disable=SC2015,SC2154 # A && B || C reports a failed check
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
m="$repo/scripts/byoc/aws-matrix.sh"
fx="$repo/tests/byoc/fixtures/aws"
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
run() { AWS_MATRIX_FIXTURES="$fx" AWS_MATRIX_NOW=1800000000 "$@"; }

out="$(run "$m" reap --dry-run)"
grep -q 'terminate-instances --instance-ids i-expired0000000001' <<<"$out" && ok "reaper_deletes_expired (instance)" || bad "expired instance not deleted: $out"
grep -q 'delete-secret --secret-id loams/ci-111/tailnet-authkey' <<<"$out" && ok "reaper_deletes_expired (secret)" || bad "expired secret not deleted"
! grep -qE 'i-fresh00000000002|ci-222' <<<"$(grep WOULD <<<"$out")" && ok "reaper_keeps_unexpired" || bad "an unexpired resource would be deleted"
! grep -qE 'i-persist000000003|loams/ci/persistent' <<<"$(grep WOULD <<<"$out")" && ok "reaper_keeps_persistent" || bad "a persistent resource would be deleted"
grep -q 'UNTAGGED: instance i-untagged00000004' <<<"$out" && ! grep -q 'UNTAGGED: instance i-untaggednew' <<<"$out" && ! grep -q 'UNTAGGED.*i-expired' <<<"$out" && ok "reaper_reports_untagged (old and untagged only)" || bad "untagged report wrong: $out"
! grep -E 'WOULD.*i-untagged' <<<"$out" >/dev/null && ok "reaper never deletes untagged" || bad "untagged would be deleted"

out="$(GITHUB_RUN_ID=777 run "$m" down)"
grep -q 'terminate-instances --instance-ids i-run7770000000001' <<<"$out" && grep -q 'delete-security-group --group-id sg-0123456789abcdef0' <<<"$out" \
  && grep -q 'ci-777/tailnet-authkey' <<<"$out" && ! grep -q 'ci-888' <<<"$out" && ok "down removes only this run's resources" || bad "down: $out"

# the workflows: teardown runs under always(), the role is the owner's, no key
wf="$repo/.github/workflows/byoc-aws-nightly.yml"
grep -qE "if: always\(\) && steps.guard.outputs.ready == 'true'" "$wf" && grep -q 'aws-matrix.sh down' "$wf" && ok "nightly_tears_down_in_always" || bad "down is not under always()"
grep -q 'timeout-minutes: 50' "$wf" && grep -q 'group: byoc-aws' "$wf" && grep -q 'id-token: write' "$wf" && ok "nightly has its timeout, concurrency group and OIDC permission" || bad "nightly settings"
! grep -rEi 'aws_secret_access_key|aws_access_key_id|AKIA[0-9A-Z]{16}' "$repo/.github/workflows/byoc-aws-nightly.yml" "$repo/.github/workflows/byoc-reaper.yml" "$repo/deploy/byoc/aws/ci-role-policy.json" && ok "no static AWS key in the workflows" || bad "a static key"
grep -qE 'configure-aws-credentials@[0-9a-f]{40}' "$wf" && ok "configure-aws-credentials is pinned by commit SHA" || bad "configure-aws-credentials is not pinned by SHA"
jq -e '[.Statement[] | select(.Effect=="Allow") | .Action | (if type=="array" then .[] else . end)] | all(. != "*" and (endswith(":*")|not))' "$repo/deploy/byoc/aws/ci-role-policy.json" >/dev/null && ok "ci role policy grants no wildcard action" || bad "wildcard action"
jq -e '[.Statement[] | .Action[]] | sort == (["ec2:RunInstances","ecs:CreateService","ecs:RunTask","lambda:CreateFunction"] | sort)' "$repo/deploy/byoc/aws/ci-deny-policy.json" >/dev/null && ok "ci deny policy is the budget action's four denies" || bad "deny policy"
exit "$fail"
