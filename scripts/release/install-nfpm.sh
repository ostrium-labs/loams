#!/usr/bin/env bash
# Install nFPM, pinned by version and by SHA-256, into ./bin or $1.
#
# nFPM builds the .deb, the .rpm and the .pkg.tar.zst (D627). It is a static Go
# binary, so there is nothing to compile and no toolchain to pin: the release is
# downloaded, the checksum is checked against the value below, and the checksum
# file itself is checked against the tag it came from.
#
# The pinned values were taken from the release's own checksums.txt on
# 2026-10-04 (nfpm v2.47.0, published 2026-06-20). Verified 2026-10-04 by
# installing from this script on x86_64 Linux and running `nfpm --version`.
#
# Usage: scripts/release/install-nfpm.sh [INSTALL_DIR]
set -euo pipefail

NFPM_VERSION="2.47.0"
REPO="goreleaser/nfpm"

# sha256 of nfpm_<version>_<platform>.tar.gz, from the release's checksums.txt.
declare -A SHA256=(
  ["Linux_x86_64"]="0660ca602b2d2d2ae4781a06c692b3eeb9d437ffea05b831d76e41f4a3188783"
  ["Linux_arm64"]="1c0f5f2999b9a974bfb04fdb0cc3306096de530ac5dbb25d739cc5f5219c919c"
)

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) PLATFORM="Linux_x86_64" ;;
  Linux-aarch64|Linux-arm64) PLATFORM="Linux_arm64" ;;
  *) echo "install-nfpm: no pinned nFPM build for $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

DEST="${1:-$PWD/bin}"
BASE="https://github.com/$REPO/releases/download/v$NFPM_VERSION"
TARBALL="nfpm_${NFPM_VERSION}_${PLATFORM}.tar.gz"

mkdir -p "$DEST"
workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

echo "install-nfpm: fetching $TARBALL"
curl --proto '=https' --tlsv1.2 -fsSL -o "$workdir/$TARBALL" "$BASE/$TARBALL"
echo "install-nfpm: fetching checksums.txt"
curl --proto '=https' --tlsv1.2 -fsSL -o "$workdir/checksums.txt" "$BASE/checksums.txt"

expected="${SHA256[$PLATFORM]}"
actual=$(sha256sum "$workdir/$TARBALL" | cut -d' ' -f1)
if [ "$actual" != "$expected" ]; then
  echo "install-nfpm: $TARBALL has sha256 $actual, expected $expected" >&2
  exit 1
fi
# The checksum has to be the one this project published, not one a compromised
# download could have rewritten.
grep -q "^$expected  $TARBALL\$" "$workdir/checksums.txt" || {
  echo "install-nfpm: $TARBALL is not listed with $expected in the release checksums.txt" >&2
  exit 1
}

tar -xzf "$workdir/$TARBALL" -C "$workdir"
install -m 0755 "$workdir/nfpm" "$DEST/nfpm"
"$DEST/nfpm" --version | tail -2
echo "install-nfpm: $DEST/nfpm is nFPM $NFPM_VERSION ($PLATFORM)"