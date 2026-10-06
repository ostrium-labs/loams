#!/usr/bin/env bash
# Installs the Rust protoc plugins buf.gen.yaml names, into
# ~/.local/share/loams-tools (not the global cargo bin). Versions are pinned:
# bump them together with the `buffa` and `connectrpc` dependencies.
set -euo pipefail
root="${LOAMS_TOOLS_DIR:-$HOME/.local/share/loams-tools}"
cargo install --locked --root "$root" \
  protoc-gen-buffa@0.9.2 protoc-gen-buffa-packaging@0.9.2 connectrpc-codegen@0.9.0
echo "installed into $root/bin (add it to PATH)"
