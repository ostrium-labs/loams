#!/usr/bin/env bash
# HE0 driver for Plausible CE: synthetic traffic only (plan HE1 Task 5).
# Creates its user, site, goals and API key through the release's `rpc`
# inside the unmodified container, then runs driver.py.
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if [ -n "${COMPOSE:-}" ]; then read -r -a C <<<"$COMPOSE"
elif command -v docker >/dev/null 2>&1; then C=(docker compose)
else C=(podman compose); fi
"${C[@]}" -f "$HERE/compose.yaml" exec -T plausible /app/bin/plausible rpc "$(cat "$HERE/setup.exs")"
exec python3 -I "$HERE/driver.py"
