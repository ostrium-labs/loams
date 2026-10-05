#!/usr/bin/env bash
# CI entry point for the connector registry (CN1 Task 2; CN1's file structure names
# scripts/connectors/gen_registry.sh as the script the connectors CI job runs). It is a thin
# wrapper: every argument, including --check, --docs and --write-csv, goes to the Python
# generator unchanged, so the shell and the Python can never disagree about what CI checks.
#
#   scripts/connectors/gen_registry.sh --check          # CI: fail on any drift (exit non-zero)
#   scripts/connectors/gen_registry.sh                 # write the generated stubs
#   scripts/connectors/gen_registry.sh --docs PATH     # render Appendix A's tables from the CSV
#   scripts/connectors/gen_registry.sh --validate      # validate the manifests against the schema
#
# The repository root is resolved from this script's own location so the job works from any
# working directory, the same way scripts/spec/provenance.sh does it.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
exec python3 "$root/scripts/connectors/gen_registry.py" "$@"