#!/usr/bin/env bash
# Holds ported code to its attribution (docs/licences/ported-code.md; KV1
# plan KV0 Task 9, D1649): a file under the ported trees that says
# `ported-from:` has the full header, and every file the register lists has
# a `ported-from:` block naming each of its origin files.
# Usage: check-ported-headers.sh [root]   (default: the repository)
#        check-ported-headers.sh --self-test
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"

if [[ "${1:-}" == --self-test ]]; then
  data="$here/testdata"
  "$0" "$data/good" >/dev/null || {
    echo 'ported headers: the good tree was refused' >&2
    exit 1
  }
  expect() { # <tree> <text the refusal must contain>
    local out
    if out=$("$0" "$data/$1" 2>&1); then
      echo "ported headers: $1 was accepted" >&2
      exit 1
    fi
    [[ "$out" == *"$2"* ]] || {
      echo "ported headers: $1 was refused for the wrong reason:" >&2
      echo "$out" >&2
      exit 1
    }
  }
  expect bad-incomplete 'incomplete ported-from block'
  expect bad-unmodified 'no "Modified by Ostrium Labs for Loams" line'
  expect bad-unmarked 'listed in the register but has no ported-from'
  expect bad-missing-origin 'does not name origin file src/cmd/set.rs'
  expect bad-missing-file 'listed in the register but missing'
  echo 'ported headers: self-test passed (5 planted failures refused)'
  exit 0
fi

root="${1:-$repo}"
register="$root/docs/licences/ported-code.md"
trees=(crates/loams-resp crates/loams-cql crates/loams-tikv/src/cdc)
failed=0
fail() {
  echo "ported headers: $*" >&2
  failed=1
}

# The text after a comment leader: `//`, `#`, `--`, `/*`, ` *`.
strip='s/^[[:space:]]*(\/\/+|#+|--|\/\*+|\*+)?[[:space:]]*//'

check_header() { # <file> <relative name>
  local file="$1" name="$2" problems
  problems=$(sed -E "$strip" "$file" | awk -v name="$name" '
    { line[NR] = $0 }
    END {
      n = 0
      for (i = 1; i <= NR; i++) {
        if (line[i] !~ /^ported-from:[[:space:]]*[^[:space:]]/) continue
        n++
        if (line[i + 1] !~ /^revision:[[:space:]]*[^[:space:]]/ ||
            line[i + 2] !~ /^path:[[:space:]]*[^[:space:]]/ ||
            line[i + 3] !~ /^licence:[[:space:]]*[^[:space:]]/) {
          printf "%s:%d: incomplete ported-from block (needs revision:, path:, licence: in order)\n", name, i
        }
      }
      if (n == 0 && NR > 0) {
        # A bare marker with no value.
        for (i = 1; i <= NR; i++) if (line[i] ~ /^ported-from:/) {
          printf "%s:%d: incomplete ported-from block (no origin)\n", name, i
        }
      }
    }')
  [[ -z "$problems" ]] || fail "$problems"
  grep -Eqi 'modified by Ostrium Labs for Loams' "$file" ||
    fail "$name: no \"Modified by Ostrium Labs for Loams\" line"
}

# 1. Every file with the marker has the full header.
for tree in "${trees[@]}"; do
  [[ -d "$root/$tree" ]] || continue
  while IFS= read -r -d '' file; do
    check_header "$file" "${file#"$root"/}"
  done < <(grep -rlZ 'ported-from:' "$root/$tree" || true)
done

# 2. Every file of the register exists, has the marker and names each of
# its origin files on a `path:` line.
if [[ -f "$register" ]]; then
  while IFS='|' read -r _ file origin files _; do
    file="$(echo "$file" | tr -d '`' | xargs)"
    files="$(echo "$files" | tr -d '`')"
    [[ "$file" == crates/* ]] || continue
    if [[ ! -f "$root/$file" ]]; then
      fail "$file: listed in the register but missing"
      continue
    fi
    if ! grep -q 'ported-from:' "$root/$file"; then
      fail "$file: listed in the register but has no ported-from header"
      continue
    fi
    paths="$(sed -E "$strip" "$root/$file" | sed -nE 's/^path:[[:space:]]*//p' | tr ',' '\n' | xargs -n1)"
    IFS=',' read -ra wanted <<<"$files"
    for want in "${wanted[@]}"; do
      want="$(echo "$want" | xargs)"
      [[ -z "$want" ]] && continue
      grep -qxF -- "$want" <<<"$paths" ||
        fail "$file: the header does not name origin file $want (register: $(echo "$origin" | xargs))"
    done
  done < <(grep -E '^\|[[:space:]]*`?crates/' "$register" || true)
else
  fail "no register at ${register#"$root"/}"
fi

if [[ "$failed" != 0 ]]; then
  echo 'ported headers: see docs/licences/ported-code.md' >&2
  exit 1
fi
echo 'ported headers: ok'
