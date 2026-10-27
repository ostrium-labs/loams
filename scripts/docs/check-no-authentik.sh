#!/usr/bin/env bash
# The gate of IA1 Task 1 (design §76 §13, D1600): Authentik is removed, so the
# name may appear only in the historical records the allow-list names.
#
#   scripts/docs/check-no-authentik.sh               # the repository
#   scripts/docs/check-no-authentik.sh --self-test   # check the checker
#   scripts/docs/check-no-authentik.sh DIR ALLOWLIST # any tree (self-test)
#
# Every file `git grep -il authentik` lists must match a pattern in the
# allow-list (scripts/docs/no-authentik-allowlist.txt), or else every match in
# it must be one of the two historical documents' file names, which other
# documents link (design §38's and plan MT1's; renaming them would break every
# link to them, on GitHub and on loams.dev), or this gate's own name
# (`no-authentik`, so CI can run it by path). A pattern is a path relative to the
# root, where `*` also matches `/`. A pattern that matches no file fails the
# check too, so the list cannot keep entries for files that no longer need them.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ "${1:-}" == --self-test ]]; then
  data="$root/scripts/docs/testdata/no-authentik"
  "$0" "$data/clean" "$data/allowlist.txt"
  if output=$("$0" "$data/dirty" "$data/allowlist.txt" 2>&1); then
    echo 'no-authentik: a planted mention was accepted' >&2
    exit 1
  fi
  [[ "$output" == *'notes/setup.md:3:'* && "$output" == *'stale allow-list entry: history/gone.md'* ]] || {
    echo "$output" >&2
    exit 1
  }
  [[ "$output" != *'links.md'* ]] || {
    echo "no-authentik: a link to a historical document was reported: $output" >&2
    exit 1
  }
  echo 'no-authentik: self-test passed (a stray mention and a stale entry rejected, links kept)'
  exit 0
fi

scan_root="${1:-$root}"
allowlist="${2:-$root/scripts/docs/no-authentik-allowlist.txt}"

# The historical documents' file names, which stay because links use them,
# and this gate's own name, which CI and docs use to run it.
linked_names='38-knative-authentik-gitops|2026-10-02-mt1-authentik-identity|no-authentik'

list_files() {
  if git -C "$scan_root" rev-parse --is-inside-work-tree >/dev/null 2>&1 &&
    [[ "$(git -C "$scan_root" rev-parse --show-toplevel)" == "$scan_root" ]]; then
    git -C "$scan_root" grep -il authentik || true
  else
    (cd "$scan_root" && git grep --no-index -il authentik -- . || true)
  fi
}

patterns=()
while IFS= read -r line; do
  line="${line%%#*}"
  line="${line#"${line%%[![:space:]]*}"}"
  line="${line%"${line##*[![:space:]]}"}"
  [[ -n "$line" ]] && patterns+=("$line")
done <"$allowlist"

mapfile -t files < <(list_files | sed 's|^\./||' | LC_ALL=C sort)

failed=0
declare -A used=()
for f in "${files[@]}"; do
  allowed=0
  for p in "${patterns[@]}"; do
    # shellcheck disable=SC2053 # the pattern is a glob on purpose
    if [[ "$f" == $p ]]; then
      allowed=1
      used["$p"]=1
      break
    fi
  done
  ((allowed)) && continue
  # Binary files have no lines to report; they must be allow-listed.
  if ! grep -Iq . "$scan_root/$f"; then
    echo "$f: binary file mentions Authentik and is not allow-listed" >&2
    failed=1
    continue
  fi
  stray=$(grep -in authentik "$scan_root/$f" | sed -E "s/($linked_names)//g" | grep -i authentik || true)
  if [[ -n "$stray" ]]; then
    while IFS= read -r l; do echo "$f:$l" >&2; done <<<"$stray"
    failed=1
  fi
done

for p in "${patterns[@]}"; do
  if [[ -z "${used[$p]:-}" ]]; then
    echo "stale allow-list entry: $p (no file it matches mentions Authentik)" >&2
    failed=1
  fi
done

if ((failed)); then
  echo 'no-authentik: Authentik is removed (D1600); reword the lines above, or, for a historical record, add it to scripts/docs/no-authentik-allowlist.txt' >&2
  exit 1
fi
echo "no-authentik: ok (${#files[@]} files mention it, all historical or allow-listed)"
