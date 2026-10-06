#!/usr/bin/env bash
# Regenerates crates/loams-desktop-link/src/gen from the vendored protos with buf.
# Needs `buf` and the plugins from scripts/loams/install-proto-plugins.sh on PATH.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
export PATH="$HOME/.local/share/loams-tools/bin:$PATH"
cd "$root/crates/loams-desktop-link"
buf generate
