#!/usr/bin/env bash
# The licence gate over conformance/compute/pins.env (D1271, D359; OC1 OC0
# Task 1). The logic is scripts/compute/licence_gate.py; see its header.
#
#   scripts/compute/licence_gate.sh [--online] [--pins FILE] [--licences FILE]
set -euo pipefail
exec python3 "$(dirname "$0")/licence_gate.py" "$@"
