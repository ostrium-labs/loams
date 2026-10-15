#!/usr/bin/env bash
# The upgrade fixtures' own tests (UP1 Task 19), with the pinned
# requirements, through uv:
#
#   run.sh [pytest args...]
#
# LOAMS_UPGRADE_ENGINE (with LOAMS_API_KEY) also runs
# write_then_check_on_same_release_passes against a running stack.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec uv run --no-project --python 3.12 --with-requirements "${here}/requirements.txt" \
    pytest -q -p no:cacheprovider "${here}" "$@"
