#!/usr/bin/env bash
# HE0 driver for HyperDX: synthetic telemetry only (plan HE1 Task 5).
set -euo pipefail
exec python3 -I "$(dirname "${BASH_SOURCE[0]}")/driver.py"
