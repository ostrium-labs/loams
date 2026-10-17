#!/usr/bin/env bash
# upgrade_sh_dry_run_prints_remote_commands (UP1 Task 22): byoc/upgrade.sh's
# --dry-run, and its remote script run locally against a stub stack and a
# stub `loams` (no SSH, no download).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail() { echo "upgrade-test: $*" >&2; exit 1; }

out="$("$here/upgrade.sh" --dry-run -p 2222 -i /k ubuntu@203.0.113.10 --to 0.1.0 --yes)"
grep -qx 'ssh -p 2222 -i /k ubuntu@203.0.113.10 <remote script>' <<<"$out" || fail "ssh line: $out"
grep -qx 'remote: dir=loams version=0.1.0 mode=upgrade yes=1' <<<"$out" || fail "args: $out"
"$here/upgrade.sh" --dry-run h --to latest >/dev/null 2>&1 && fail "a non-version --to was accepted"
"$here/upgrade.sh" --dry-run h >/dev/null 2>&1 && fail "a missing --to was accepted"
"$here/upgrade.sh" --dry-run h --to 0.1.0 | grep -q -- ' -t ' || fail "no TTY without --yes"

# The remote script, with the target's CLI already fetched.
script="$(sed -n '/^remote script:$/,$p' <<<"$out" | tail -n +2)"
mkdir -p "$tmp/loams/release" "$tmp/loams/.loams-upgrade/bin"
touch "$tmp/loams/release/compose.yaml"
printf '#!/bin/sh\necho "loams $*"\n' > "$tmp/loams/.loams-upgrade/bin/loams-0.1.0"
chmod +x "$tmp/loams/.loams-upgrade/bin/loams-0.1.0"
if [ "$(uname -m)" = x86_64 ]; then
  ran="$(bash -c "$script" loams-upgrade "$tmp/loams" 0.1.0 upgrade 1)"
  [ "$ran" = "loams upgrade --to 0.1.0 --dir $tmp/loams/release --output text --yes" ] || fail "ran: $ran"
  ran="$(bash -c "$script" loams-upgrade "$tmp/loams" 0.1.0 check 0)"
  [ "$ran" = "loams upgrade check --to 0.1.0 --dir $tmp/loams/release --output text" ] || fail "ran: $ran"
  ran="$(bash -c "$script" loams-upgrade "$tmp/loams" 0.1.0 rollback 1)"
  [ "$ran" = "loams upgrade rollback --dir $tmp/loams/release --output text --yes" ] || fail "ran: $ran"
fi
bash -c "$script" loams-upgrade "$tmp/nothing" 0.1.0 upgrade 1 >/dev/null 2>&1 && fail "no stack was accepted"
echo "upgrade-test: passed"
