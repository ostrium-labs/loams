#!/usr/bin/env bash
# PgDog's licence boundary (design §46 §8.3, D236; PG2 Task 19). PgDog is
# AGPL-3.0 and Loams runs it only as an unmodified, separate process. Over the
# tracked files of the repository this fails when:
#   - a Cargo.lock or a JavaScript lockfile names a `pgdog` package, or a
#     vendored directory holds one;
#   - a file references the `ghcr.io/pgdogdev/pgdog` image outside the bundles
#     that may (deploy/pgdog, deploy/helm/loams-postgres,
#     deploy/loams-postgres-single) and the test harnesses that are never
#     distributed (the router's scripts/router/inventory, and the compute
#     conformance harness conformance/compute, whose transaction-mode
#     overlay runs PgDog unmodified; DX1 ruling R0.8); prose (*.md) may name it;
#   - a reference lacks an @sha256 digest;
#   - a bundle that references the image lacks LICENSE-NOTICE.md.
# Then it runs scripts/spec/provenance.sh (no PgDog text in the router specs).
#
#   scripts/ci/pgdog-licence.sh [root]
#   scripts/ci/pgdog-licence.sh --self-test
set -euo pipefail

IMAGE='ghcr.io/pgdogdev/pgdog'
BUNDLES=(deploy/pgdog deploy/helm/loams-postgres deploy/loams-postgres-single)
HARNESS=(scripts/router/inventory conformance/compute)

# The check itself, over the git repository at $1.
check() {
  local root=$1 fail=0 f line
  local -a files
  mapfile -t files < <(git -C "$root" ls-files)

  # No PgDog package in a lockfile, nor vendored.
  for f in "${files[@]}"; do
    case $f in
      */Cargo.lock | Cargo.lock)
        if grep -qE '^name = "pgdog([-_][a-z0-9_-]*)?"$' "$root/$f"; then
          echo "pgdog-licence: $f has a pgdog crate (no PgDog code in Loams)"; fail=1
        fi ;;
      */pnpm-lock.yaml | pnpm-lock.yaml | */package-lock.json | package-lock.json | */yarn.lock | yarn.lock)
        if grep -qE '(^|[/"'"'"' ])pgdog([-_@/"'"'"':]|$)' "$root/$f"; then
          echo "pgdog-licence: $f has a pgdog package (no PgDog code in Loams)"; fail=1
        fi ;;
    esac
    case /$f in
      */vendor/pgdog* | */vendored/pgdog* | */third_party/pgdog* | */third-party/pgdog*)
        echo "pgdog-licence: $f is vendored PgDog (no PgDog source in Loams)"; fail=1 ;;
    esac
  done

  # Image references: where, pinned, and with the notice.
  local -A bundle_refs=()
  local b allowed
  for f in "${files[@]}"; do
    [ -f "$root/$f" ] || continue
    grep -qF "$IMAGE" "$root/$f" || continue
    # Prose may name the image, and this script names it to look for it.
    case $f in *.md | scripts/ci/pgdog-licence.sh) continue ;; esac
    allowed=
    for b in "${BUNDLES[@]}"; do
      case $f in "$b"/*) allowed=$b; bundle_refs[$b]=1 ;; esac
    done
    for b in "${HARNESS[@]}"; do
      case $f in "$b"/*) allowed=$b ;; esac
    done
    if [ -z "$allowed" ]; then
      echo "pgdog-licence: $f references $IMAGE outside ${BUNDLES[*]} (and the test harnesses ${HARNESS[*]})"; fail=1
    fi
    # A digest on the line, or `@$var` / `@${var}` in a file that pins a
    # full digest (the harness's `digest="${PGDOG_DIGEST:-sha256:…}"`).
    local pinned_in_file=
    grep -qE 'sha256:[0-9a-f]{64}' "$root/$f" && pinned_in_file=1
    while IFS= read -r line; do
      if grep -qE 'sha256:[0-9a-f]{64}' <<<"$line"; then
        continue
      fi
      if [ -n "$pinned_in_file" ] && grep -qF "$IMAGE@\$" <<<"$line"; then
        continue
      fi
      echo "pgdog-licence: $f: $IMAGE without an @sha256 digest: ${line#"${line%%[![:space:]]*}"}"
      fail=1
    done < <(grep -F "$IMAGE" "$root/$f")
  done
  for b in "${!bundle_refs[@]}"; do
    if [ ! -f "$root/$b/LICENSE-NOTICE.md" ]; then
      echo "pgdog-licence: $b references $IMAGE but has no LICENSE-NOTICE.md"; fail=1
    fi
  done

  # No PgDog text in the router specs and conformance (D318).
  if [ -x "$root/scripts/spec/provenance.sh" ]; then
    "$root/scripts/spec/provenance.sh" >/dev/null || { echo "pgdog-licence: scripts/spec/provenance.sh failed"; fail=1; }
  fi
  return $fail
}

# Fixtures: a repository per case.
self_test() {
  local tmp ok=0
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' RETURN
  local digest=sha256:25d1908886f595a2e3b00ec59783326712e3c0f75f91d29f032ff712b30f2266
  repo() {
    rm -rf "$tmp/r"; mkdir -p "$tmp/r"; git -C "$tmp/r" init -q
    git -C "$tmp/r" config user.email t@example.invalid; git -C "$tmp/r" config user.name t
  }
  put() { mkdir -p "$(dirname "$tmp/r/$1")"; printf '%s\n' "$2" >"$tmp/r/$1"; git -C "$tmp/r" add -- "$1"; }
  expect() { # expect <0|1> <case>
    local got=0
    check "$tmp/r" >"$tmp/out" 2>&1 || got=1
    if [ "$got" != "$1" ]; then
      echo "self-test $2: expected exit $1, got $got"; cat "$tmp/out"; ok=1
    else
      echo "self-test $2: ok"
    fi
  }

  # no_pgdog_in_lockfiles
  repo
  put Cargo.lock '[[package]]
name = "pgdog-config"
version = "0.1.0"'
  expect 1 no_pgdog_in_lockfiles/cargo
  repo
  put apps/web/pnpm-lock.yaml "  /pgdog@1.0.0:
    resolution: {integrity: x}"
  expect 1 no_pgdog_in_lockfiles/pnpm
  repo
  put third_party/pgdog/src/lib.rs 'fn main() {}'
  expect 1 no_pgdog_in_lockfiles/vendored
  repo
  put Cargo.lock '[[package]]
name = "pgdog-like-name-elsewhere-not"
version = "0.1.0"
[[package]]
name = "tokio"'
  put crates/x/src/lib.rs '// talks to pgdog over the admin database'
  expect 1 no_pgdog_in_lockfiles/prefix_is_still_pgdog
  repo
  put Cargo.lock '[[package]]
name = "tokio"'
  put crates/x/src/lib.rs '// talks to pgdog over the admin database'
  expect 0 no_pgdog_in_lockfiles/clean

  # pgdog_image_pinned_by_digest
  repo
  put deploy/pgdog/LICENSE-NOTICE.md 'notice'
  put deploy/pgdog/compose.yaml "    image: $IMAGE:v0.1.60"
  expect 1 pgdog_image_pinned_by_digest/tag
  repo
  put deploy/pgdog/LICENSE-NOTICE.md 'notice'
  put deploy/pgdog/compose.yaml "    image: $IMAGE@$digest"
  put deploy/release/compose.yaml "    image: $IMAGE@$digest"
  expect 1 pgdog_image_pinned_by_digest/outside_the_bundles
  repo
  put deploy/pgdog/LICENSE-NOTICE.md 'notice'
  put deploy/pgdog/compose.yaml "    image: $IMAGE@$digest"
  put scripts/router/inventory/compose.pg.yml "    image: $IMAGE@\${PGDOG_DIGEST:-$digest}"
  put scripts/router/inventory/shims/pgdog "digest=\"\${PGDOG_DIGEST:-$digest}\"
exec podman run \"$IMAGE@\${digest}\" pgdog"
  put docs/design/x.md "PgDog runs from $IMAGE pinned by digest."
  expect 0 pgdog_image_pinned_by_digest/pinned

  repo
  put deploy/pgdog/LICENSE-NOTICE.md 'notice'
  put deploy/pgdog/run.sh "digest=latest
exec podman run \"$IMAGE@\$digest\""
  expect 1 pgdog_image_pinned_by_digest/variable_without_a_digest

  # licence_bundle_present
  repo
  put deploy/helm/loams-postgres/values.yaml "pgdog: {image: $IMAGE@$digest}"
  expect 1 licence_bundle_present/missing
  repo
  put deploy/helm/loams-postgres/values.yaml "pgdog: {image: $IMAGE@$digest}"
  put deploy/helm/loams-postgres/LICENSE-NOTICE.md 'notice'
  expect 0 licence_bundle_present/present

  return $ok
}

if [ "${1:-}" = --self-test ]; then
  self_test
else
  root=${1:-$(git rev-parse --show-toplevel)}
  if check "$root"; then
    echo "pgdog-licence: ok"
  else
    exit 1
  fi
fi
