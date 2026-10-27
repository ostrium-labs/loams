#!/usr/bin/env bash
# Ask the compat-mode frontend for its health from the compose network
# (no host port is published). Run from the repository root.
set -euo pipefail
. conformance/compute/lib.sh
oc_temporal_wait temporal-frontend:7233 300
oc_run "$TEMPORAL_ADMIN_TOOLS_IMAGE" temporal operator cluster health --address temporal-frontend:7233
