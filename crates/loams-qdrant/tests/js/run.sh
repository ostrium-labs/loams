#!/usr/bin/env bash
# Runs the @qdrant/js-client-rest smoke against a running `loams dev`
# (plan MA1 Task 25).
#
#   run.sh <1.18.0|1.19.0>
#
# The REST URL is http://127.0.0.1:6333 unless LOAMS_QDRANT_URL says
# otherwise. Installs the pinned clients from the lockfile first.
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <1.18.0|1.19.0>" >&2
    exit 2
fi
case "$1" in
    1.18.0) alias=1.18 ;;
    1.19.0) alias=1.19 ;;
    *) echo "unknown client version $1 (1.18.0 or 1.19.0)" >&2; exit 2 ;;
esac
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${here}"
# Its own pnpm-workspace.yaml (overrides) makes this directory a workspace root,
# separate from the repository workspace.
pnpm install --frozen-lockfile --ignore-scripts --silent
exec node smoke.mjs --version "${alias}" --url "${LOAMS_QDRANT_URL:-http://127.0.0.1:6333}"
