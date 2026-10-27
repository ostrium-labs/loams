#!/usr/bin/env bash
# HE0 driver for OpenPanel (plan HE1 Tasks 4 and 9): synthetic traffic only.
# Signs up, creates a project and a write client, ingests a seeded week of
# events through POST /api/track (so the Redis buffers and the worker's batch
# inserts run) and reads every dashboard page's tRPC procedures, the funnel,
# conversion and retention charts included. See driver.py.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT_ROOT=${HE0_OUT:-$HERE/../.out}
export HE0_OUT_APP=${HE0_OUT_APP:-$OUT_ROOT/${HE0_APP:-openpanel}}
mkdir -p "$HE0_OUT_APP"
exec python3 -I "$HERE/driver.py"
