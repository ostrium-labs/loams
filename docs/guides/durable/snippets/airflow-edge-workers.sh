#!/usr/bin/env bash
# List the Edge workers through the API server's Edge API, from the compose
# network, with the conformance harness's client. Run from the repository root.
set -euo pipefail
. conformance/compute/lib.sh
. conformance/compute/airflow/common.sh
af workers
