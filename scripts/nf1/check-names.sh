#!/usr/bin/env bash
# Fails if a pre-rename name is left (NF1 Task 1b, D823): the crate
# loams-neon (now loams-postgres), the dev stack deploy/neon (now
# deploy/loams-postgres-dev) or the fork's old URL ostrium-labs/neon (now
# ostrium-labs/loams-postgres).
#
#   scripts/nf1/check-names.sh [root]
#
# Scans the tracked files under root (all files when root is not a git
# checkout). docs/ is skipped: the design history and rulings keep the old
# names, with a mapping note at the top. Allowed on purpose, because they name
# users' data: the compose project `name: loams-neon` (Docker names volumes
# after it), the desktop's per-user copy `dir: "neon"` and `stacks/neon`.
set -euo pipefail

root=${1:-.}
pattern='crates/loams-neon|-p loams-neon|loams_neon::|deploy/neon\b|github\.com/ostrium-labs/neon\b'
allow='name: loams-neon|dir: "neon"|stacks/neon'

if git -C "$root" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  list() { git -C "$root" ls-files -z; }
else
  list() { (cd "$root" && find . -type f -not -path './.git/*' -printf '%P\0'); }
fi

hits=$(list | grep -zv -e '^docs/' -e '^scripts/nf1/' \
  | (cd "$root" && xargs -0r grep -nIP "$pattern" --) \
  | grep -vE "$allow" || true)

if [ -n "$hits" ]; then
  echo "check-names: pre-rename names left (D823):" >&2
  echo "$hits" >&2
  exit 1
fi
echo "check-names: ok"
