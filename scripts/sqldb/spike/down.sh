#!/usr/bin/env bash
# Stop the spike: every TiDB container it started, then PD and TiKV.
# -v also removes the named volumes and target-spike's rendered configs.
set -euo pipefail
# shellcheck source=lib.sh source-path=SCRIPTDIR
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

volumes=false
[[ "${1:-}" == -v ]] && volumes=true
mapfile -t tidbs < <(podman ps -aq --filter "label=io.loams.spike=$SPIKE_PROJECT")
if (( ${#tidbs[@]} )); then podman rm -f -t 5 "${tidbs[@]}" >/dev/null; fi
if $volumes; then
  spike_compose down -v
  rm -f "$SPIKE_OUT"/tidb-*.toml
else
  spike_compose down
fi
