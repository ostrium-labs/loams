#!/usr/bin/env bash
# Runs the whole conformance story for every language, in one command
# (SDK1 Task 4).
#
#   sdks/conformance/run-all.sh                 # verify, harness tests, every suite
#   sdks/conformance/run-all.sh --lang python   # one language
#   sdks/conformance/run-all.sh --strict        # drift is a failure, not a warning
#   sdks/conformance/run-all.sh --harness-only  # corpus + harness, no language suite
#
# Three layers, cheapest first, because they fail for different reasons and the
# order matters when something is broken:
#
#   1. **The corpus.** `verify-corpus.mjs` proves every required fixture exists,
#      that each recording still matches its own expectation, that the fault
#      catalogue matches the injector, and that every reason the registry promises
#      is either recorded or explicitly declared unproducible. A language suite
#      run against a corpus that does not verify is measuring nothing.
#   2. **The harness.** The Task 4 tests: every fixture replays byte for byte, and
#      every injected fault produces the shape `faults.json` documents.
#   3. **The languages.** Each language's six named tests, plus the 100%
#      required-fixture bar from `required.mjs`, plus a drift report.
#
# Exit codes: 0 everything passed, 1 something failed, 2 the runner could not run.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

lang=""
strict=0
harness_only=0
for arg in "$@"; do
  case "$arg" in
    --strict) strict=1 ;;
    --harness-only) harness_only=1 ;;
    -*) echo "unknown flag: $arg" >&2; exit 2 ;;
    *) lang="$arg" ;;
  esac
done

failures=()

section() {
  echo ""
  echo "=== $1 ==="
}

# ---- 1. the corpus ----------------------------------------------------------

section "the conformance corpus"
if ! node sdks/conformance/verify-corpus.mjs; then
  echo "" >&2
  echo "the corpus does not verify; nothing else was run" >&2
  exit 1
fi

# ---- 2. the harness ---------------------------------------------------------

section "the conformance harness"
if ! node --test sdks/conformance/conformance.test.mjs; then
  failures+=("the harness tests")
fi

if [ "$harness_only" -eq 1 ]; then
  section "done (--harness-only)"
  if [ "${#failures[@]}" -gt 0 ]; then
    echo "failed: ${failures[*]}" >&2
    exit 1
  fi
  exit 0
fi

# ---- 3. the languages -------------------------------------------------------

# Which languages have a suite on disk right now, as opposed to in the plan.
langs="$(node -e "
import('./sdks/conformance/required.mjs').then(async (m) => {
  const { existsSync } = await import('node:fs');
  const out = Object.keys(m.LANGUAGES).filter((l) => existsSync(m.LANGUAGES[l].dir));
  console.log(out.join(' '));
});
")"
if [ -n "$lang" ]; then
  langs="$lang"
fi

if [ -z "$langs" ]; then
  echo "no language suite is present in the tree"
  exit 0
fi

section "the language suites ($langs)"
# One fixture server for the whole run rather than one per language: it holds no
# state between requests, and starting thirteen is thirteen chances to collide
# with a port.
pid_file="$(mktemp)"
log="$(mktemp)"
node sdks/conformance/fixture-server.mjs --port 0 > "$log" 2>&1 &
echo "$!" > "$pid_file"
trap 'kill "$(cat "$pid_file")" 2>/dev/null || true; rm -f "$pid_file" "$log"' EXIT

endpoint=""
for _ in $(seq 1 100); do
  endpoint="$(grep -o '"url":"[^"]*"' "$log" | head -1 | cut -d '"' -f 4)"
  [ -n "$endpoint" ] && break
  if ! kill -0 "$(cat "$pid_file")" 2>/dev/null; then break; fi
  sleep 0.1
done
if [ -z "$endpoint" ]; then
  echo "the fixture server did not come up:" >&2
  cat "$log" >&2
  exit 2
fi
echo "replaying sdks/fixtures/recorded on $endpoint"
export LOAMS_TEST_ENDPOINT="$endpoint"

for language in $langs; do
  for test in \
    conformance_all_required_fixtures \
    retry_reuses_idempotency_key \
    error_reason_mapping \
    stream_resume_with_cursor \
    token_source_refresh \
    pagination_iterator
  do
    printf '%-12s %-40s ' "$language" "$test"
    if sdks/conformance/run-test.sh "$language" "$test" > /tmp/conformance-one.log 2>&1; then
      echo "pass"
    else
      code=$?
      echo "FAIL (exit $code)"
      sed 's/^/    /' /tmp/conformance-one.log | tail -25
      failures+=("${language}_${test}")
    fi
  done
done

# ---- the 100% bar and drift -------------------------------------------------

section "the 100% required-fixture bar"
for language in $langs; do
  if [ -f "sdks/fixtures/results/${language}.json" ]; then
    printf '%-12s ' "$language"
    if node sdks/conformance/check-languages.mjs --check "$language"; then
      :
    else
      failures+=("${language} required-fixture coverage")
    fi
  else
    printf '%-12s %s\n' "$language" "no report; its fixture coverage is unverified"
  fi
done

section "drift between languages"
drift_out="$(node sdks/conformance/check-languages.mjs --drift 2>&1)"
drift_status=$?
echo "$drift_out"
if [ "$strict" -eq 1 ] && [ "$drift_status" -ne 0 ]; then
  failures+=("conformance drift")
elif [ "$drift_status" -ne 0 ]; then
  echo "note: drift is a warning here; --strict would fail the run"
fi

section "summary"
if [ "${#failures[@]}" -gt 0 ]; then
  echo "failed:" >&2
  for failure in "${failures[@]}"; do
    echo "  - $failure" >&2
  done
  exit 1
fi
echo "everything passed"
if [ "$drift_status" -ne 0 ]; then
  echo "with the drift warnings printed above"
fi
exit 0
