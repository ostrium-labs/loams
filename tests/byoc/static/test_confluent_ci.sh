#!/usr/bin/env bash
# scripts/byoc/confluent-ci.sh's reaper against recorded API JSON (no Confluent
# call), and the nightly workflow's shape (plan CC1 Task 10).
# shellcheck disable=SC2015 # A && B || C reports a failed check
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
s="$repo/scripts/byoc/confluent-ci.sh"
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }
run() { env -u CONFLUENT_CLOUD_API_KEY -u CONFLUENT_CLOUD_API_SECRET CONFLUENT_CI_FIXTURES="$repo/tests/byoc/fixtures/confluent" \
  CONFLUENT_CI_NOW=1800000000 CONFLUENT_CI_ENVIRONMENT=env-test01 CONFLUENT_CI_WORK="$(mktemp -d)" "$@"; }

out="$(run "$s" reap --dry-run)"
grep -q 'WOULD DELETE cluster lkc-expired01 loams-ci-111' <<<"$out" && grep -q 'WOULD DELETE API key KEYEXPIRED01' <<<"$out" \
  && ok "reaper_deletes_expired_clusters (and their keys)" || bad "expired not deleted: $out"
! grep -E 'WOULD DELETE.*(lkc-fresh0002|KEYFRESH0002)' <<<"$out" >/dev/null && ok "reaper keeps a run inside its 3-hour TTL" || bad "a fresh run would be deleted"
! grep -E 'WOULD DELETE.*(lkc-120zwvj|KEYORACLE003|lkc-demo0003|KEYOWNER0004|lkc-hand0004)' <<<"$out" >/dev/null && grep -q 'keep lkc-120zwvj' <<<"$out" \
  && ok "reaper_keeps_oracle_and_demo (and keys not named loams-ci-*)" || bad "oracle, demo or a foreign key would be deleted: $out"
grep -q 'REPORT: cluster lkc-demo0003' <<<"$out" && grep -q 'REPORT: cluster lkc-hand0004' <<<"$out" && ! grep -q 'REPORT: cluster lkc-new00005' <<<"$out" \
  && ok "reaper_reports_unnamed (older than a day only, never deleted)" || bad "report wrong: $out"
set +e; o2="$(run env CONFLUENT_CI_ENVIRONMENT=env-3kqddw "$s" reap --dry-run 2>&1)"; rc=$?; set -e
[[ $rc != 0 ]] && grep -q protected <<<"$o2" && ok "the oracle's environment is refused" || bad "env-3kqddw accepted: $o2"
set +e; run env CONFLUENT_CI_ENVIRONMENT='env-x;rm' "$s" reap --dry-run >/dev/null 2>&1; rc=$?; set -e
[[ $rc != 0 ]] && ok "a malformed environment id is refused" || bad "env-x;rm accepted"
grep -q 'keep key KEYOTHERENV5' <<<"$out" && ! grep -q 'WOULD DELETE API key KEYOTHERENV5' <<<"$out" \
  && ok "reaper_keeps_keys_outside_the_environment (API keys are listed org-wide)" || bad "a key outside the environment would be deleted: $out"
! grep -E 'WOULD DELETE.*(lkc-bench0006|KEYBENCH0006)' <<<"$out" >/dev/null && grep -q 'REPORT: cluster lkc-bench0006' <<<"$out" \
  && ok "reaper_keeps_other_work_in_loams_ci (only loams-ci-<run id> names; the CFB benchmark is reported)" || bad "a non-run cluster or key would be deleted: $out"
# Without fixtures, only loams-ci (env-3kqygo) is accepted; refused before any API call.
set +e; o3="$(env -u CONFLUENT_CI_FIXTURES CONFLUENT_CLOUD_API_KEY=x CONFLUENT_CLOUD_API_SECRET=x CONFLUENT_API_URL=http://127.0.0.1:9 \
  CONFLUENT_CI_ENVIRONMENT=env-test01 CONFLUENT_CI_WORK="$(mktemp -d)" "$s" reap --dry-run 2>&1)"; rc=$?; set -e
[[ $rc != 0 ]] && grep -q 'only ever touches env-3kqygo' <<<"$o3" && ok "reaper_only_touches_loams_ci (env-3kqygo)" || bad "another environment accepted: $o3"
grep -q 'ci_env="env-3kqygo"' "$s" && ok "the loams-ci environment is pinned in the script" || bad "ci_env not pinned"

# The nightly: no-op until enabled, teardown under always(), one at a time, no key in the file.
wf="$repo/.github/workflows/byoc-confluent-nightly.yml"
grep -q "vars.CONFLUENT_CI_ENABLED" "$wf" && grep -qE "if: always\(\) && steps.guard.outputs.ready == 'true'" "$wf" && grep -q 'confluent-ci.sh down' "$wf" \
  && ok "nightly_tears_down_in_always (and is gated on CONFLUENT_CI_ENABLED)" || bad "nightly gate or teardown"
grep -q 'group: byoc-confluent' "$wf" && grep -q 'timeout-minutes: 50' "$wf" && grep -q 'environment: byoc-confluent' "$wf" \
  && ok "nightly: concurrency byoc-confluent, 50-minute timeout, environment byoc-confluent" || bad "nightly settings"
rp="$repo/.github/workflows/byoc-confluent-reaper.yml"
grep -q "cron: \"" "$rp" && grep -q 'confluent-ci.sh reap' "$rp" && ok "reaper workflow runs reap on a schedule" || bad "reaper workflow"
! grep -rEi '(CONFLUENT_CLOUD_API_(KEY|SECRET)|KAFKA_API_SECRET): *[A-Za-z0-9+/]{12,}' "$wf" "$rp" && ok "no key in the workflows" || bad "a key in a workflow"
exit "$fail"
