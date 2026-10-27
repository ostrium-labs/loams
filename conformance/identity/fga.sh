#!/usr/bin/env bash
# The OpenFGA CLI (openfga/cli v0.8.1, Apache-2.0, pinned by digest), run in a container on the
# IA0 compose network with deploy/openfga mounted read-only at /model.
#   ./fga.sh model test --tests /model/tests/core.fga.yaml
#   ./fga.sh model write --store-id <id> --file /model/fga.mod
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
model=$(cd "$here/../../deploy/openfga" && pwd)
engine=$(command -v docker || command -v podman)
image=docker.io/openfga/cli:v0.8.1@sha256:1b8035ea4aeb8ad739cbea972488cdda37886644ed004737880b63403bae0fe8
net=()
if "$engine" network inspect loams-ia0_default >/dev/null 2>&1; then
  net=(--network loams-ia0_default -e FGA_API_URL=http://openfga:8080)
fi
exec "$engine" run --rm -i "${net[@]}" -v "$model:/model:ro,z" "$image" "$@"
