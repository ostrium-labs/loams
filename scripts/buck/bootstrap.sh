#!/usr/bin/env bash
# Install the pinned Buck2 tooling (scripts/buck/versions.env) for this
# checkout:
#
#   scripts/buck/bootstrap.sh            install buck2 and reindeer
#   scripts/buck/bootstrap.sh --check    exit 1 if either is missing or stale
#
# Each tool is installed once per machine into a versioned directory under
# ${LOAMS_BUCK_CACHE:-${XDG_CACHE_HOME:-~/.cache}/loams-build}, and linked into
# the checkout as .tools/bin/{buck2,reindeer} (git-ignored), so worktrees share
# one download and one reindeer build. buck2 is checked against the release's
# sha256 before it is unpacked; reindeer is built from its pinned commit with
# the lockfile it ships.
#
# buck-out: Buck2 writes its output to buck-out/ at the repository root, and
# it must be a real directory there: Buck2 links sources into buck-out with
# relative symlinks (../../crates/...), which break when buck-out is itself a
# symlink to another disk. To keep build output off a small disk, put the
# checkout (or worktree) on the larger one.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=scripts/buck/versions.env
source "$repo/scripts/buck/versions.env"

check=false
case "${1:-}" in
  --check) check=true ;;
  "") ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) triple=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) triple=aarch64-unknown-linux-gnu ;;
  Darwin-arm64) triple=aarch64-apple-darwin ;;
  Darwin-x86_64) triple=x86_64-apple-darwin ;;
  *) echo "bootstrap: no pinned buck2 for $(uname -s)-$(uname -m)" >&2; exit 2 ;;
esac
sha_var="BUCK2_SHA256_${triple//-/_}"
buck2_sha=${!sha_var}

cache=${LOAMS_BUCK_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/loams-build}
buck2_dir="$cache/buck2-$BUCK2_VERSION-$triple"
reindeer_dir="$cache/reindeer-${REINDEER_REV:0:12}"
bin="$repo/.tools/bin"

have() { [[ -x "$1" ]]; }

if $check; then
  ok=true
  have "$bin/buck2" && [[ "$(readlink -f "$bin/buck2")" == "$buck2_dir/buck2" ]] || { echo "bootstrap: buck2 $BUCK2_VERSION not installed" >&2; ok=false; }
  have "$bin/reindeer" && [[ "$(readlink -f "$bin/reindeer")" == "$reindeer_dir/bin/reindeer" ]] || { echo "bootstrap: reindeer ${REINDEER_REV:0:12} not installed" >&2; ok=false; }
  $ok && echo "bootstrap: buck2 $BUCK2_VERSION and reindeer ${REINDEER_REV:0:12} are current"
  $ok
  exit
fi

for tool in curl zstd cargo; do
  command -v "$tool" > /dev/null || { echo "bootstrap: $tool is required" >&2; exit 2; }
done
mkdir -p "$cache" "$bin"

sha256() {
  if command -v sha256sum > /dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

if ! have "$buck2_dir/buck2"; then
  echo "bootstrap: downloading buck2 $BUCK2_VERSION ($triple)"
  tmp="$cache/.buck2-$BUCK2_VERSION.$$"
  rm -rf "$tmp"; mkdir -p "$tmp"
  trap 'rm -rf "$tmp"' EXIT
  curl --fail --location --silent --show-error --retry 3 \
    -o "$tmp/buck2.zst" \
    "https://github.com/facebook/buck2/releases/download/$BUCK2_VERSION/buck2-$triple.zst"
  got=$(sha256 "$tmp/buck2.zst")
  if [[ "$got" != "$buck2_sha" ]]; then
    echo "bootstrap: buck2-$triple.zst sha256 is $got, expected $buck2_sha" >&2
    exit 1
  fi
  zstd -q -d "$tmp/buck2.zst" -o "$tmp/buck2"
  chmod +x "$tmp/buck2"
  rm -rf "$buck2_dir"; mkdir -p "$buck2_dir"
  mv "$tmp/buck2" "$buck2_dir/buck2"
  rm -rf "$tmp"; trap - EXIT
fi
ver=$("$buck2_dir/buck2" --version)
if [[ "$ver" != *"$BUCK2_COMMIT"* ]]; then
  echo "bootstrap: $buck2_dir/buck2 reports '$ver', expected commit $BUCK2_COMMIT" >&2
  exit 1
fi

if ! have "$reindeer_dir/bin/reindeer"; then
  echo "bootstrap: building reindeer ${REINDEER_REV:0:12} (a few minutes, once per machine)"
  # Build in the workspace's own target directory (whatever .cargo/config or
  # CARGO_TARGET_DIR says), not cargo install's temporary one.
  target=$(cd "$repo" && cargo metadata --format-version 1 --no-deps --offline 2> /dev/null \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' 2> /dev/null || true)
  args=()
  [[ -n "$target" ]] && args+=(--target-dir "$target/loams-build-tools")
  cargo install --locked --git "$REINDEER_GIT" --rev "$REINDEER_REV" \
    --root "$reindeer_dir" "${args[@]}" reindeer
fi

ln -sfn "$buck2_dir/buck2" "$bin/buck2"
ln -sfn "$reindeer_dir/bin/reindeer" "$bin/reindeer"

if [[ -L "$repo/buck-out" ]]; then
  echo "bootstrap: $repo/buck-out is a symlink; Buck2 needs a real directory (remove it)" >&2
  exit 1
fi

echo "bootstrap: $("$bin/buck2" --version), reindeer ${REINDEER_REV:0:12}"
echo "bootstrap: add $bin to PATH, or call .tools/bin/buck2"
