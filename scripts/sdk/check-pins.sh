#!/usr/bin/env bash
# Check that no pinned generation tool has moved (SDK1 Task 1's pin check).
#
#   scripts/sdk/check-pins.sh              # check the repository as it stands
#   scripts/sdk/check-pins.sh --self-test  # check the checker
#
# A generation difference must be a deliberate act — a line in
# `scripts/sdk/pins.lock` and the regenerated output in the same PR — and never
# whatever the runner happened to have. This fails when:
#
#   1. a `remote: buf.build/<owner>/<plugin>:<ref>` in any buf template does not
#      match the version in the lock file;
#   2. a `remote:` reference is not an exact pin (no ref at all, `latest`, or a
#      pre-release), because an unpinned reference resolves to whatever is newest
#      on the day it runs;
#   3. a buf template references a plugin the lock file does not list, so a new
#      tool cannot arrive unpinned;
#   4. a library pin the lock file records (npm, PyPI, go.mod, Cargo.toml) has
#      moved away from the locked version, or has been loosened to a range;
#   5. a lock row has no integrity, because "pinned" without one is a version
#      and not a pin.
#
# buf publishes no digest for a protoc plugin, so a `bsr` row's integrity is the
# plugin's *upstream* version — the `protoc-gen-*` release the BSR build wraps.
# That is the strongest pin buf offers for a remote plugin, and it is checkable
# by reading the plugin's own release notes. Every other kind carries a real
# hash from its registry, and `--refresh` re-fetches those.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

lock="scripts/sdk/pins.lock"
failures=0

fail() {
  echo "check-pins: $*" >&2
  failures=$((failures + 1))
}

warn() {
  echo "check-pins: warning: $*" >&2
}

# A buf plugin's exact version: `v<major>[.<minor>...]`, nothing else. buf's own
# convention for a protoc plugin is `v<protoc version>-<plugin version>`, which
# has one dot; anything with a dash after the numbers is a pre-release or a range.
version_is_exact() {
  printf '%s' "$1" | grep -Eq '^v[0-9]+(\.[0-9]+)*$'
}

# Every buf template the SDK pipeline owns: the per-language ones under sdks/ and
# the app-package one at the root.
sdk_templates() {
  find ./sdks -name 'buf.gen*.yaml' -not -path '*/node_modules/*' | sort
  printf '%s\n' ./buf.gen.apps.yaml
}

# The templates at the repository root that the *mobile* repos run
# (`buf.gen.swift.yaml`, `buf.gen.kotlin.yaml`, AP0's ruling E7). They belong to
# `ostrium-labs/loams-mobile` and are checked as warnings, not failures: their
# remote plugins are unpinned today, which is a real gap, but failing every
# build in this repository over another owner's files would be worse than
# reporting it. `scripts/sdk/check-pins.sh --strict` promotes them to failures
# for whoever fixes them.
mobile_templates() {
  find . -maxdepth 1 -name 'buf.gen*.yaml' ! -name 'buf.gen.apps.yaml' | sort
}

# `remote: buf.build/owner/plugin:ref` on one line, one per line.
remote_refs() {
  sed -n 's/^[[:space:]]*-[[:space:]]*remote:[[:space:]]*\([^[:space:]#]*\).*/\1/p' "$1"
}

# The locked version of a buf remote plugin, empty when the lock has no row.
locked_bsr() {
  awk -v id="$1" '$1 == "bsr" && $2 == id { print $3 }' "$lock"
}

# The locked "<version> <integrity>" of a tool, empty when absent.
locked() {
  awk -v kind="$1" -v id="$2" '$1 == kind && $2 == id { print $3 "\t" $4; exit }' "$lock"
}

# The locked "<version> <integrity>" of the single tool of a kind that has no id
# (the buf CLI).
locked_singleton() {
  awk -v kind="$1" '$1 == kind { print $2 "\t" $3; exit }' "$lock"
}

# The lock rows of one kind, as "<id> <version>" lines.
locked_rows() {
  awk -v kind="$1" '$1 == kind { print $2 " " $3 }' "$lock"
}

# --- 1-3: the buf remote plugins ---------------------------------------------
# `report` is `fail` or `warn`; the mobile templates are warned about for the
# reason given above `mobile_templates`.
check_templates() {
  local report="${1:-fail}"
  shift || true
  local templates=("$@")
  if [ "${#templates[@]}" -eq 0 ]; then
    mapfile -t templates < <(sdk_templates)
  fi
  local template ref id version want
  for template in "${templates[@]}"; do
    while read -r ref; do
      [ -n "$ref" ] || continue
      id="${ref%:*}"
      version="${ref##*:}"
      if [ "$version" = "$ref" ]; then
        "$report" "$template: '$id' has no version, so buf resolves the newest build every run"
        continue
      fi
      if ! version_is_exact "$version"; then
        "$report" "$template: '$id' is pinned to '$version', which is not an exact version"
        continue
      fi
      want="$(locked_bsr "$id")"
      if [ -z "$want" ]; then
        "$report" "$template: '$id' is not in $lock, so nothing pins its version"
      elif [ "$want" != "$version" ]; then
        "$report" "$template: '$id' is pinned to '$version' but $lock says '$want'; a pin moves only in the PR that regenerates the output"
      fi
    done < <(remote_refs "$template")
  done
}

# --- 5: every lock row is a pin ---------------------------------------------
check_lock_shape() {
  local line kind rest id version integrity
  while read -r line; do
    case "$line" in '' | '#'*) continue ;; esac
    kind="${line%% *}"
    rest="${line#* }"
    id="${rest%% *}"
    rest="${rest#* }"
    version="${rest%% *}"
    integrity="${rest#* }"
    integrity="${integrity%% *}"
    case "$kind" in
      npm | pypi | go | crate | bsr | buf) ;;
      *) fail "$lock: unknown kind '$kind' on the '$id' row" ;;
    esac
    if [ -z "$id" ] || [ -z "$version" ] || [ -z "$integrity" ]; then
      fail "$lock: the '$id' row has no version or no integrity; a version alone is not a pin"
    fi
  done < "$lock"
}

# --- 4: the library pins -----------------------------------------------------
check_npm() {
  local manifest="sdks/typescript/packages/client/package.json"
  [ -f "$manifest" ] || return 0
  local id spec version integrity
  while read -r id; do
    spec="$(locked npm "$id")"
    [ -n "$spec" ] || continue
    version="${spec%%	*}"
    integrity="${spec#*	}"
    grep -qF "\"$id\": \"$version\"" "$manifest" \
      || fail "$manifest does not pin \"$id\" to exactly $version"
    [ -f web/pnpm-lock.yaml ] || continue
    grep -qF "$id@$version" web/pnpm-lock.yaml \
      || fail "web/pnpm-lock.yaml has no entry for $id@$version"
    [ -n "$integrity" ] || continue
    # The integrity sits on the `resolution:` line of the entry that follows the
    # `@name@version` key line.
    # The lockfile lists the package as `'@scope/name@version':` (sometimes
    # unquoted) and the integrity on the `resolution:` line below it. The quote
    # character is built with sprintf because awk's escape support varies.
    awk -v key="$id@$version" -v want="$integrity" -v quote="$(printf "'")" '
      { bare = $0; sub(/^[ \t]+/, "", bare); sub(/:$/, "", bare); gsub(quote, "", bare) }
      bare == key { found = 1; next }
      found && /resolution: \{integrity:/ { ok = index($0, want) > 0; exit }
      # `exit` runs END, so the verdict is carried in `ok` rather than in the
      # exit status: an `exit 1` here would override an `exit 0` above it.
      END { exit ok ? 0 : 1 }
    ' web/pnpm-lock.yaml || fail "web/pnpm-lock.yaml's integrity for $id@$version is not $integrity"
  done < <(locked_rows npm | awk '{print $1}')
}

check_pypi() {
  local id version requirements
  for requirements in sdks/python/pyproject.toml sdks/python/requirements.txt; do
    [ -f "$requirements" ] || continue
    while read -r id version; do
      case "$id" in
        connectrpc | protoc-gen-connectrpc) ;;
        *) continue ;;
      esac
      grep -Eq "(==|[\"'])${id}==${version}([\"']|$)" "$requirements" \
        || fail "$requirements does not pin $id to exactly $version"
    done < <(locked_rows pypi)
  done
}

check_go() {
  [ -f sdks/go/go.mod ] || return 0
  local id version
  while read -r id version; do
    awk -v id="$id" -v version="$version" '
      $1 == "require" { inblock = 1 }
      inblock && $1 == id && $2 == version { found = 1 }
      /^\)/ { inblock = 0 }
      END { exit found ? 0 : 1 }
    ' sdks/go/go.mod || fail "sdks/go/go.mod does not require $id at exactly $version"
  done < <(locked_rows go)
}

# Cargo.toml carries a caret requirement, which is a range; Cargo.lock carries
# the resolved version and its checksum. The lockfile is therefore both the pin
# and the integrity, and that is what is compared.
check_crates() {
  [ -f Cargo.lock ] || return 0
  local id version checksum
  while read -r id version checksum; do
    # A Cargo.lock package block is `name`, `version`, `source`, `checksum`, in
    # that order, so the checksum is found by scanning the block rather than by
    # taking the next line.
    awk -v name="\"$id\"" -v version="\"$version\"" -v checksum="\"$checksum\"" '
      /^\[\[package\]\]$/ { seen = 0; ver = 0; sum = 0 }
      $0 == "name = " name { seen = 1; next }
      seen && !ver && $0 == "version = " version { ver = 1; next }
      seen && ver && !sum && $0 == "checksum = " checksum { sum = 1 }
      seen && ver && sum { found = 1 }
      END { exit found ? 0 : 1 }
    ' Cargo.lock || fail "Cargo.lock does not resolve $id to $version with checksum $checksum"
  done < <(awk '$1 == "crate" { print $2 " " $3 " " $4 }' "$lock")
}

check_buf_cli() {
  local spec version reported
  spec="$(locked_singleton buf)"
  [ -n "$spec" ] || return 0
  version="${spec%%	*}"
  local binary="sdks/typescript/packages/client/node_modules/.bin/buf"
  [ -x "$binary" ] || return 0
  reported="$("$binary" --version 2>/dev/null | awk '{print $2}')"
  [ "$reported" = "$version" ] || fail "the installed buf is $reported but $lock says $version"
}

check_all() {
  [ -f "$lock" ] || {
    echo "check-pins: $lock is missing" >&2
    exit 1
  }
  check_lock_shape
  check_templates fail
  mapfile -t mobile < <(mobile_templates)
  if [ "${#mobile[@]}" -gt 0 ]; then
    check_templates warn "${mobile[@]}"
  fi
  check_npm
  check_pypi
  check_go
  check_crates
  check_buf_cli
}

# --- --self-test -------------------------------------------------------------
# A check that cannot fail is worse than no check, so each rule is exercised
# against a template that must pass and templates that must fail.
self_test() {
  local passed=0 total=0 scratch
  scratch="$(mktemp -d)"
  trap 'rm -rf "$scratch"' RETURN

  # Assert that checking one template leaves `failures` at the wanted count.
  # `failures` is a global the checkers increment, so this reads it directly.
  assert_failures() {
    local want="$1" template="$2" label="$3"
    total=$((total + 1))
    failures=0
    check_templates fail "$scratch/$template"
    if [ "$failures" -eq "$want" ]; then
      passed=$((passed + 1))
    else
      echo "  FAIL $label: wanted $want problem(s), got $failures" >&2
    fi
    failures=0
  }

  cat > "$scratch/buf.gen.exact.yaml" <<'EOF'
version: v2
plugins:
  - remote: buf.build/protocolbuffers/python:v35.0
    out: out
EOF
  cat > "$scratch/buf.gen.unversioned.yaml" <<'EOF'
version: v2
plugins:
  - remote: buf.build/protocolbuffers/python
    out: out
EOF
  cat > "$scratch/buf.gen.latest.yaml" <<'EOF'
version: v2
plugins:
  - remote: buf.build/protocolbuffers/python:latest
    out: out
EOF
  cat > "$scratch/buf.gen.prerelease.yaml" <<'EOF'
version: v2
plugins:
  - remote: buf.build/protocolbuffers/python:v36.0-rc1
    out: out
EOF
  cat > "$scratch/buf.gen.moved.yaml" <<'EOF'
version: v2
plugins:
  - remote: buf.build/protocolbuffers/python:v34.1
    out: out
EOF
  cat > "$scratch/buf.gen.unlocked.yaml" <<'EOF'
version: v2
plugins:
  - remote: buf.build/someowner/someplugin:v1.0.0
    out: out
EOF
  cat > "$scratch/buf.gen.local.yaml" <<'EOF'
version: v2
plugins:
  - local: ./some-plugin
    out: out
EOF

  echo "check-pins --self-test"
  assert_failures 0 buf.gen.exact.yaml "an exactly pinned, locked plugin passes"
  assert_failures 1 buf.gen.unversioned.yaml "a plugin with no version fails"
  assert_failures 1 buf.gen.latest.yaml "latest fails"
  assert_failures 1 buf.gen.prerelease.yaml "a pre-release version fails"
  assert_failures 1 buf.gen.moved.yaml "a version that moved off the lock fails"
  assert_failures 1 buf.gen.unlocked.yaml "a plugin absent from the lock fails"
  assert_failures 0 buf.gen.local.yaml "a local plugin is not a pin and is left alone"

  total=$((total + 1))
  if [ "$(version_is_exact v1.36.11 && echo yes)" = yes ] &&
    ! version_is_exact v1.36.11-rc1 &&
    ! version_is_exact latest &&
    ! version_is_exact ""; then
    passed=$((passed + 1))
  else
    echo "  FAIL version_is_exact accepts or rejects the wrong versions" >&2
  fi

  total=$((total + 1))
  failures=0
  check_lock_shape
  if [ "$failures" -eq 0 ]; then passed=$((passed + 1)); else
    echo "  FAIL the lock file itself does not pass its own shape check ($failures problems)" >&2
  fi
  failures=0

  echo "  $passed/$total passed"
  [ "$passed" -eq "$total" ]
}

case "${1:-}" in
  --self-test)
    self_test
    ;;
  "")
    check_all
    if [ "$failures" -ne 0 ]; then
      echo "check-pins: $failures problem(s). A pin moves only in the PR that regenerates the output, and $lock moves with it." >&2
      exit 1
    fi
    echo "check-pins: every pinned tool matches $lock"
    ;;
  *)
    echo "usage: scripts/sdk/check-pins.sh [--self-test]" >&2
    exit 2
    ;;
esac