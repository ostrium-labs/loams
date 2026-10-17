#!/usr/bin/env bash
# Renders the release notes' "Upgrading" section for VERSION from
# release/upgrade-path.toml (design §54 §11.3, D942; UP1 Task 11).
#   scripts/release/upgrade-notes.sh 0.1.0
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: $0 VERSION" >&2; exit 2; }
exec python3 "$(dirname "$0")/upgrade_path.py" notes "$1"
