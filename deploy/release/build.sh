#!/usr/bin/env bash
# Builds the Loams release images from deploy/release/Containerfile.<name>,
# for linux/amd64 and linux/arm64, tagged :<version> and its channel's tags
# (:preview for a prerelease; :<major>.<minor> and :latest for a stable
# version, see scripts/release/channel.sh).
#
#   deploy/release/build.sh [options] [image...]
#
#   image            loams, loams-fabric, loams-agentd, loams-pg-control
#                    [default: all four]; managed Spark's (preview, opt in):
#                    loams-spark-proxy, loams-sail, loams-spark
#   --version V      the version tag [default: the tag on HEAD, else 0.0.0-dev.<sha>]
#   --registry R     [default: ghcr.io/ostrium-labs, or $LOAMS_REGISTRY]
#   --platforms P    comma-separated [default: linux/amd64,linux/arm64]
#   --push           push every tag (CI on a release tag); without it the
#                    images stay local
#   --jobs N         CARGO_BUILD_JOBS inside the build stage
#   --arch-tag       tag only :<version>-<arch> (one platform; CI joins the
#                    per-arch images into :<version> and its channel tags with
#                    `docker buildx imagetools create`)
#
# Uses podman (one manifest list per image, `podman manifest push --all`) when
# it is installed, else `docker buildx` (which needs --push, or a single
# platform, to keep the result: a multi-platform build cannot be --load-ed).
# Building a foreign platform runs under QEMU binfmt emulation, which is slow;
# CI builds each platform on a native runner by passing --platforms.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
registry="${LOAMS_REGISTRY:-ghcr.io/ostrium-labs}"
platforms="linux/amd64,linux/arm64"
version=""
push=0
jobs=""
arch_tag=0
images=()

die() { echo "build.sh: $*" >&2; exit 1; }

while (($#)); do
  case "$1" in
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --registry) registry="${2:?--registry needs a value}"; shift 2 ;;
    --platforms) platforms="${2:?--platforms needs a value}"; shift 2 ;;
    --push) push=1; shift ;;
    --jobs) jobs="${2:?--jobs needs a value}"; shift 2 ;;
    --arch-tag) arch_tag=1; shift ;;
    -h|--help) sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    loams|loams-fabric|loams-agentd|loams-pg-control|loams-spark-proxy|loams-sail|loams-spark) images+=("$1"); shift ;;
    *) die "unknown argument: $1" ;;
  esac
done
((${#images[@]})) || images=(loams loams-fabric loams-agentd loams-pg-control)
[[ -z "$jobs" || "$jobs" =~ ^[0-9]+$ ]] || die "--jobs must be a number"

revision="$(git -C "$root" rev-parse HEAD 2>/dev/null || echo unknown)"
if [[ -z "$version" ]]; then
  version="$(git -C "$root" describe --tags --exact-match HEAD 2>/dev/null || true)"
  [[ -n "$version" ]] || version="0.0.0-dev.${revision:0:12}"
fi
version="${version#v}"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "version $version is not semver"

# LOAMS_BUILD_ENGINE=podman|buildx picks one when both are installed (the
# upgrade gate builds into Docker's store, which its stack runs from).
if [[ -n "${LOAMS_BUILD_ENGINE:-}" ]]; then
  engine="$LOAMS_BUILD_ENGINE"
  [[ "$engine" == podman || "$engine" == buildx ]] || die "LOAMS_BUILD_ENGINE is podman or buildx"
elif command -v podman >/dev/null 2>&1; then
  engine=podman
elif command -v docker >/dev/null 2>&1 && docker buildx version >/dev/null 2>&1; then
  engine=buildx
else
  die "needs podman, or docker with buildx"
fi

# Dev builds (0.0.0-dev.<sha>) are prereleases to channel.sh, so they keep :preview.
if tag_list=$("$root/scripts/release/channel.sh" "$version" 2>/dev/null); then
  read -r -a tags <<<"$(sed -n 's/^image_tags=//p' <<<"$tag_list")"
else
  tags=("$version" preview)
fi
if ((arch_tag)); then
  [[ "$platforms" != *,* ]] || die "--arch-tag takes one platform"
  tags=("$version-${platforms##*/}")
fi

build_args=(--build-arg "VERSION=$version" --build-arg "REVISION=$revision")
[[ -n "$jobs" ]] && build_args+=(--build-arg "CARGO_BUILD_JOBS=$jobs")

for image in "${images[@]}"; do
  file="$root/deploy/release/Containerfile.$image"
  [[ -f "$file" ]] || die "no $file"
  ref="$registry/$image"
  echo "build.sh: $ref:{${tags[*]}} for $platforms ($engine)"
  case "$engine" in
    podman)
      # A manifest list per image; --format docker keeps the HEALTHCHECK.
      podman manifest rm "$ref:${tags[0]}" >/dev/null 2>&1 || true
      podman build --format docker --platform "$platforms" --manifest "$ref:${tags[0]}" \
        "${build_args[@]}" -f "$file" "$root"
      for t in "${tags[@]:1}"; do podman tag "$ref:${tags[0]}" "$ref:$t"; done
      if ((push)); then
        for t in "${tags[@]}"; do
          podman manifest push --all "$ref:${tags[0]}" "docker://$ref:$t"
        done
      fi
      ;;
    buildx)
      out=(--push)
      if ((!push)); then
        [[ "$platforms" == *,* ]] && die "docker buildx keeps a multi-platform build only with --push; pass --platforms linux/amd64 to build locally"
        out=(--load)
      fi
      tag_args=()
      for t in "${tags[@]}"; do tag_args+=(-t "$ref:$t"); done
      docker buildx build --platform "$platforms" "${build_args[@]}" \
        "${tag_args[@]}" "${out[@]}" -f "$file" "$root"
      ;;
  esac
done
