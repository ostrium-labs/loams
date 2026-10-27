#!/usr/bin/env bash
# Run by loams-ci-monitor.timer (root) every 15 minutes: records cache sizes,
# caps and runner names for status.sh (which needs no sudo then), and logs
# status.sh's report to the journal: `journalctl -u loams-ci-monitor`.
# A failed check makes the unit fail, which `systemctl --failed` shows.
set -uo pipefail
DATA_ROOT=${1:-$(sed -n 's/^DATA_ROOT=//p' /opt/loams-ci/status.env 2>/dev/null)}
DATA_ROOT=${DATA_ROOT:-/mnt/Projects/ci}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
install -d -m 755 /run/loams-ci
tmp=$(mktemp /run/loams-ci/sizes.XXXXXX)
for d in nativelink sccache runners cache containers; do
  [[ -d $DATA_ROOT/$d ]] && printf 'size %s %s\n' "$DATA_ROOT/$d" "$(du -sb "$DATA_ROOT/$d" | cut -f1)"
done >"$tmp"
[[ -r /etc/loams-ci/nativelink.json5 ]] &&
  awk 'match($0, /max_bytes: *[0-9]+/) {v = substr($0, RSTART, RLENGTH); gsub(/[^0-9]/, "", v); s += v} END {if (s) printf "cap nativelink %.0f\n", s/1073741824}' /etc/loams-ci/nativelink.json5 >>"$tmp"
for d in "$DATA_ROOT"/runners/*/; do [[ -d $d ]] && printf 'runner %s\n' "$(basename "$d")"; done >>"$tmp"
chmod 644 "$tmp"
mv "$tmp" /run/loams-ci/sizes
DATA_ROOT=$DATA_ROOT "$HERE/status.sh"
