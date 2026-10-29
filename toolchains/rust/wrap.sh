#!/bin/sh
# Runs a Rust tool for Buck2: toolchains/rust/wrap.sh <tool> [args], called by
# toolchains/rust/{rustc,clippy-driver,rustdoc}.
#
# * RUSTUP_TOOLCHAIN pins rustup's proxy to the channel of rust-toolchain.toml
#   (copied to toolchains/rust/channel by scripts/buck/gen.py):
#   build scripts run under buck-out, which may be a symlink to another disk
#   where rustup would not find rust-toolchain.toml.
# * CARGO_MANIFEST_DIR is the crate's repository-relative path
#   (LOAMS_MANIFEST_DIR, set by buck/rust.bzl), as tests run from the
#   repository root; the prelude would make it an absolute buck-out path.
# * CARGO_BIN_EXE_<name> and CARGO_TARGET_TMPDIR are made relative to the repository root for the same
#   reason: a binary built on another machine must not carry its path.
tool=$1
shift
read -r channel < "$(dirname "$0")/channel"
export RUSTUP_TOOLCHAIN="$channel"
if [ -n "${LOAMS_MANIFEST_DIR:-}" ]; then
  export CARGO_MANIFEST_DIR="$LOAMS_MANIFEST_DIR"
fi
root=$(pwd -P)
for var in CARGO_TARGET_TMPDIR $(env | sed -n 's/^\(CARGO_BIN_EXE_[A-Za-z0-9_-]*\)=.*/\1/p'); do
  eval "val=\${$var}"
  case "$val" in
    "$root"/*) export "$var=${val#"$root"/}" ;;
  esac
done
exec "$tool" "$@"
