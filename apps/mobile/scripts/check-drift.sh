#!/usr/bin/env bash
# Compare against an isolated regeneration; works before the import is committed.
set -euo pipefail
exec python3 "$(dirname "$0")/check-drift.py"
