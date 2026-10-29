#!/usr/bin/env bash
# Runs before every job on a local CI runner (ACTIONS_RUNNER_HOOK_JOB_STARTED).
# Refuses the job when the host's disk is low, so CI never fills the disk the
# owner works on.
set -euo pipefail
min=${LOAMS_CI_MIN_FREE_GB:-20}
dir=${RUNNER_WORK:-${GITHUB_WORKSPACE:-/}}
free=$(df -BG --output=avail "$dir" | tail -n1 | tr -dc '0-9')
if ((free < min)); then
  echo "::error title=Local CI host disk low::$(hostname) has ${free} GiB free on $(df --output=target "$dir" | tail -n1) (minimum ${min}). Free disk on the host, or set the repository variable CI_RUNNER=github."
  exit 1
fi
echo "local CI host $(hostname): ${free} GiB free"
