#!/usr/bin/env bash
# Run `pnpm nx <args>` and survive an Nx Cloud outage (docs/guides/nx-cloud.md, budget guard).
#
# When the cloud is on and the run fails because of Nx Cloud itself (a bad or revoked token, the
# free plan's credits used up, the service unreachable), Nx exits non-zero before or around the
# tasks. This wrapper then reruns the same command once with NX_NO_CLOUD=true, a plain local
# run, and leaves a warning in the job log. A real task failure is not retried: its output has
# no Nx Cloud error, and the command fails as before.
#
#   scripts/ci/nx-cloud-run.sh run-many -t build typecheck --exclude='*,!@loams/*'
#
# NX_CLOUD_RUN_NX overrides the command (the self-test, scripts/ci/nx-cloud-run.test.sh).
set -uo pipefail

nx=(${NX_CLOUD_RUN_NX:-pnpm nx})
log="$(mktemp "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/nx-cloud-run.XXXXXX")"
trap 'rm -f "$log"' EXIT

"${nx[@]}" "$@" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}
[ "$status" -eq 0 ] && exit 0
[ "${NX_NO_CLOUD:-}" = true ] && exit "$status"

# Nx Cloud's own failures. Task output never prints these.
cloud_error='Nx Cloud: Workspace is unable to be authorized|Invalid Credentials \(CI Access Token\)|Nx Cloud.*(disabled|exceeded|over (the|your) (limit|plan)|out of credits|usage limit|unavailable|timed out|could not (connect|reach))|(ECONNREFUSED|ENOTFOUND|ETIMEDOUT|ECONNRESET).*(nx\.app|cloud\.nx)'
if grep -Eiq "$cloud_error" "$log"; then
  echo "::warning title=Nx Cloud unavailable::Nx Cloud failed this run (see the log above); rerunning without the cloud."
  NX_NO_CLOUD=true "${nx[@]}" "$@"
  exit $?
fi
exit "$status"
