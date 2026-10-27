#!/usr/bin/env bash
# MA1 Task 27: a failing Mastra gate on dev (.github/workflows/mastra-gate.yml)
# opens the issue "Mastra gate fails on dev" with the label mastra-compat, or
# comments on the one already open. GH_TOKEN is the workflow's GITHUB_TOKEN.
set -euo pipefail

: "${GH_TOKEN:?GH_TOKEN must be set}"
: "${GITHUB_REPOSITORY:?}" "${GITHUB_SERVER_URL:?}" "${GITHUB_RUN_ID:?}"

title="Mastra gate fails on dev"
run_url="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
body="The Mastra gate failed on dev at ${GITHUB_SHA:-an unknown commit}: ${run_url}

Each cell's verdict (unexpected failures and unexpected passes) is in the run log and in the \`mastra-gate-results\` artifact. An unexpected pass means a fix landed on dev: mark the entry \`fixedAfter\`/\`fixedBy\` in ostrium-labs/loams-mastra's expected-failures lists."

# --search matches words, not the exact title, so filter on the title itself.
number="$(gh issue list --repo "$GITHUB_REPOSITORY" --state open \
  --search "in:title Mastra gate fails" --json number,title \
  --jq "map(select(.title == \"${title}\")) | .[0].number // empty")"

if [[ -n "$number" ]]; then
  gh issue comment "$number" --repo "$GITHUB_REPOSITORY" --body "$body"
else
  gh issue create --repo "$GITHUB_REPOSITORY" --title "$title" --label mastra-compat \
    --body "${body}

This issue is opened and updated by \`scripts/ci/mastra-gate-issue.sh\`. Close it when the gate passes again; the next failure opens a new one."
fi
