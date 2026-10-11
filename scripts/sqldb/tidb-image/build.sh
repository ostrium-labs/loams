#!/usr/bin/env bash
# Build Loams' patched tidb-server image from the ostrium-labs/tidb patch queue
# (design §47 §5.4; SQ1 R5.3).
#
#   build.sh <tidb checkout> [image]
#
# <tidb checkout> is a clean checkout of ostrium-labs/tidb at a patch-queue tag
# (`v8.5.8-loams.N` on branch `loams/v8.5.8-patches`). [image] defaults to
# `ghcr.io/ostrium-labs/tidb:<tag>`. The script only builds; push with
# `podman push --digestfile <file> <image>` and pin the pushed digest in
# release/sqldb-images.toml.
#
# The binary is built the way TiDB's Makefile builds `make server` (CGO on,
# `-tags codes`, the same `-X` version flags), inside the Go image TiDB's own
# Dockerfile pins, so it does not depend on the host's glibc. It then replaces
# /tidb-server in the upstream image of the same release, so the base (Rocky
# Linux 9), entrypoint and ports stay PingCAP's.
#
# Environment:
#   TIDB_GO_CACHE  Go module and build caches (default: .tidb-go-cache next to
#                  the checkout; several GB, so keep it off a tmpfs).
#   GO_JOBS        go build -p (default 4; the host is shared).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$(cd "${1:?usage: build.sh <tidb checkout> [image]}" && pwd)"

# TiDB v8.5.8's Dockerfile builder (golang:1.25.12) and the upstream
# pingcap/tidb:v8.5.8 index that §47 §4 pinned before the patch queue.
GO_IMAGE=docker.io/library/golang:1.25.12@sha256:9006890ecba0a168034d99516084099ae3114d9f2b7d6572c77f2dde57ebc980
BASE_IMAGE=docker.io/pingcap/tidb@sha256:df168c764bf2dfdb166dc37a5c3b0e210d29d5f3ab2d33317fd0fdf7b32037f5

git_src() { git -C "$src" "$@"; }
# Annotated tags only (no --tags): every patch-queue release is one.
tag="$(git_src describe --exact-match HEAD)" || {
  echo "build.sh: $src is not at an annotated tag" >&2; exit 1; }
[[ "$tag" =~ ^v8\.5\.8-loams\.[0-9]+$ ]] || { echo "build.sh: tag $tag is not a v8.5.8-loams.N tag" >&2; exit 1; }
[[ -z "$(git_src status --porcelain --untracked-files=no)" ]] || {
  echo "build.sh: $src has local changes" >&2; exit 1; }
image="${2:-ghcr.io/ostrium-labs/tidb:$tag}"
revision="$(git_src rev-parse HEAD)"
branch="$(git_src rev-parse --abbrev-ref HEAD)"
# The commit time, not the wall clock, so the binary depends only on the tag.
build_ts="$(TZ=UTC git_src log -1 --format=%cd --date=format-local:'%Y-%m-%d %H:%M:%S')"

cache="${TIDB_GO_CACHE:-$(dirname "$src")/.tidb-go-cache}"
mkdir -p "$cache/mod" "$cache/build" "$src/bin"

ldflags="-X 'github.com/pingcap/tidb/pkg/parser/mysql.TiDBReleaseVersion=$tag'"
ldflags+=" -X 'github.com/pingcap/tidb/pkg/util/versioninfo.TiDBBuildTS=$build_ts'"
ldflags+=" -X 'github.com/pingcap/tidb/pkg/util/versioninfo.TiDBGitHash=$revision'"
ldflags+=" -X 'github.com/pingcap/tidb/pkg/util/versioninfo.TiDBGitBranch=$branch'"
ldflags+=" -X 'github.com/pingcap/tidb/pkg/util/versioninfo.TiDBEdition=Community'"

echo "build.sh: building tidb-server $tag ($revision)" >&2
# -buildvcs=false: a worktree's .git points outside the mount; the version
# comes from the -X flags above.
podman run --rm \
  -v "$src:/tidb" -v "$cache/mod:/go/pkg/mod" -v "$cache/build:/root/.cache/go-build" \
  -w /tidb -e CGO_ENABLED=1 -e GOTOOLCHAIN=local -e GOFLAGS=-buildvcs=false \
  "$GO_IMAGE" \
  go build -p "${GO_JOBS:-4}" -tags codes -ldflags "$ldflags" -o bin/tidb-server ./cmd/tidb-server

podman build --pull=missing -f "$here/Containerfile" \
  --build-arg "BASE_IMAGE=$BASE_IMAGE" \
  --build-arg "VERSION=$tag" --build-arg "REVISION=$revision" \
  -t "$image" "$src/bin"
podman run --rm "$image" -V
echo "build.sh: built $image" >&2
