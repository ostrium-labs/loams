#!/usr/bin/env bash
# Runs after every job on a local CI runner (ACTIONS_RUNNER_HOOK_JOB_COMPLETED):
# the "ephemeral-ish" cleanup. Removes the job's checkout, temporary files and
# per-job caches, and the compose stacks the job started from its workspace.
# Keeps the downloaded actions, the tool cache, CARGO_HOME and RUSTUP_HOME.
# Never removes images.
set -uo pipefail
ws=${RUNNER_WORKSPACE:-}
if [[ -z $ws && -n ${GITHUB_WORKSPACE:-} ]]; then ws=$(dirname "$GITHUB_WORKSPACE"); fi

if command -v docker >/dev/null 2>&1 && [[ -n $ws ]]; then
  docker ps -a --format '{{.ID}} {{.Label "com.docker.compose.project.working_dir"}} {{.Label "com.docker.compose.project"}}' 2>/dev/null |
    while read -r id dir project; do
      [[ -n $dir && $dir == "$ws"/* ]] || continue
      docker rm -f -v "$id" >/dev/null 2>&1
      echo "$project"
    done | sort -u | while read -r project; do
      [[ -n $project ]] || continue
      docker network ls -q --filter "label=com.docker.compose.project=$project" | xargs -r docker network rm >/dev/null 2>&1
      docker volume ls -q --filter "label=com.docker.compose.project=$project" | xargs -r docker volume rm >/dev/null 2>&1
    done
fi

for d in "$ws" "${RUNNER_TEMP:-}"; do
  [[ -n $d && -d $d && $d == "${RUNNER_WORK:-/nonexistent}"/* ]] || continue
  find "$d" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
done
rm -rf /tmp/* /var/tmp/* "$HOME/.cache" "$HOME/.npm" "$HOME/.pnpm-store" "$HOME/.local/share/pnpm" 2>/dev/null
echo "local CI cleanup done; $(df -h --output=avail "${RUNNER_WORK:-/}" | tail -n1 | tr -d ' ') free"
exit 0
