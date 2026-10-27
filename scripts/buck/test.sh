#!/usr/bin/env bash
# Tests with Buck2, with libtest's counts summed the way `cargo test` and
# `cargo nextest` report them:
#
#   scripts/buck/test.sh [target patterns...] [-- buck2 test args]
#
# Buck2 runs each test binary (`<package>-unittest`, `<package>-test-<name>`)
# as one test; this script prints Buck2's summary, then the passed, failed and
# ignored test functions over every binary, and exits with Buck2's status.
set -uo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo"
buck2=${BUCK2:-$repo/.tools/bin/buck2}

patterns=()
while [[ $# -gt 0 && $1 != -- ]]; do patterns+=("$1"); shift; done
[[ ${1:-} == -- ]] && shift
[[ ${#patterns[@]} -eq 0 ]] && patterns=(//...)

log=$(mktemp "$repo/buck-out/test-log.XXXXXX")
trap 'rm -f "$log"' EXIT
"$buck2" test "${patterns[@]}" "$@" > "$log" 2>&1
status=$?
grep -E '✗|Fail:|FAILED|panicked' "$log" | grep -v ' 0 failed' | head -200 || true
tail -1 "$log"
grep -o 'test result: [a-zA-Z]*\. [0-9]* passed; [0-9]* failed; [0-9]* ignored' "$log" \
  | awk '{n++; p+=$4; f+=$6; i+=$8} END {printf "libtest: %d binaries, %d passed, %d failed, %d ignored\n", n, p, f, i}'
exit $status
