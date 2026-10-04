#!/usr/bin/env bash
# Runs **one** of a language's six named conformance tests (SDK1 Task 4).
#
#   sdks/conformance/run-test.sh typescript error_reason_mapping
#   sdks/conformance/run-test.sh typescript conformance_all_required_fixtures
#   sdks/conformance/run-test.sh python  pagination_iterator
#
# The six names are the contract: `conformance_all_required_fixtures`,
# `retry_reuses_idempotency_key`, `error_reason_mapping`,
# `stream_resume_with_cursor`, `token_source_refresh`, `pagination_iterator`,
# each prefixed with the language (`<lang>_<name>`). They are data in
# `required.mjs`, so a runner that accepted a seventh name would make two
# languages incomparable, and this script refuses one.
#
# The corpus is verified **before** the test runs. A suite that passes against a
# corpus with a missing or mislabelled fixture is not a pass, and finding that out
# from the suite's own failure message costs a debugging session every time.
#
# By default the test replays the committed corpus through the fixture server. With
# `--live` it runs against a real `loams dev`, which covers the `loams dev` half
# of the corpus only — the app-mock scenarios have no counterpart there, so a live
# run is a *narrower* run and says so.
#
# Exit codes: 0 pass, 1 the test failed, 2 this script could not run it.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

lang=""
test=""
live=0
for arg in "$@"; do
  case "$arg" in
    --live) live=1 ;;
    -*) echo "unknown flag: $arg" >&2; exit 2 ;;
    *)
      if [ -z "$lang" ]; then lang="$arg"; elif [ -z "$test" ]; then test="$arg"; fi
      ;;
  esac
done

if [ -z "$lang" ] || [ -z "$test" ]; then
  echo "usage: $0 <language> <test-short-name> [--live]" >&2
  echo "" >&2
  echo "the six test names:" >&2
  node -e "import('./sdks/conformance/required.mjs').then(m=>{for(const t of m.REQUIRED_TESTS)console.error('  <lang>_'+t)})"
  exit 2
fi

# The plan asks for `--` or a bare name; both are accepted so nobody has to know
# whether their language's runner wants the prefix.
full="$test"
short="$test"
case "$test" in
  "${lang}_"*) short="${test#"${lang}_"}" ;;
esac
full="${lang}_${short}"

# A test name that is not one of the six is refused **before** anything runs.
# Language runners treat a name that matches nothing as a pass in some
# configurations and as an error in others, so "I asked for a test that does not
# exist" is exactly the kind of gap that turns into a green CI run.
if ! node -e "
import('./sdks/conformance/required.mjs').then((m) => {
  process.exit(m.REQUIRED_TESTS.includes('$short') ? 0 : 1);
});
"; then
  echo "'$short' is not one of the six named conformance tests" >&2
  echo "the six are:" >&2
  node -e "import('./sdks/conformance/required.mjs').then(m=>{for(const t of m.REQUIRED_TESTS)console.error('  <lang>_'+t)})"
  exit 2
fi

# The corpus first, and it has to pass. `verify-corpus.mjs` exits non-zero and
# says which fixture, which is the only useful thing to print at this point.
echo "verifying the conformance corpus"
if ! node sdks/conformance/verify-corpus.mjs --quiet; then
  echo "" >&2
  echo "the corpus does not verify; ${lang}_${test} was not run" >&2
  exit 2
fi

# The fixture server, unless a live one was asked for. Its pid goes in a file
# rather than a shell variable: `endpoint="$(start)"` runs the function in a
# subshell, so a `pid=$!` inside it never reaches the parent and cleanup has
# nothing to kill — the server then outlives the run and the next one collides
# with it.
pid_file=""
endpoint=""
started=0

cleanup() {
  if [ -n "$pid_file" ] && [ -s "$pid_file" ]; then
    kill "$(cat "$pid_file")" 2>/dev/null || true
  fi
  rm -f "${pid_file:-/dev/null}" 2>/dev/null || true
}
trap cleanup EXIT

if [ "$live" -eq 0 ]; then
  pid_file="$(mktemp)"
  log="$(mktemp)"
  node sdks/conformance/fixture-server.mjs --port 0 > "$log" 2>&1 &
  echo "$!" > "$pid_file"
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
  started=1
  echo "replaying sdks/fixtures/recorded on $endpoint"
  export LOAMS_TEST_ENDPOINT="$endpoint"
else
  if [ -z "${LOAMS_TEST_ENDPOINT:-}" ]; then
    echo "--live needs LOAMS_TEST_ENDPOINT, or run sdks/conformance/run.sh --live which boots one" >&2
    exit 2
  fi
  echo "running against the live server at $LOAMS_TEST_ENDPOINT"
  echo "note: the loams-apps-mock fixtures have no counterpart on a live server, so this"
  echo "      run covers less than the replay does"
fi

# Which command runs this language's one test, and whether anybody has run it.
# Three fields separated by `|`, because the middle one is a whole argument list
# and `read` splits on whitespace.
IFS='|' read -r program rest verified < <(node -e "
import('./sdks/conformance/required.mjs').then((m) => {
  const spec = m.LANGUAGES['$lang'];
  if (!spec) { console.log('none||0'); return; }
  console.log([spec.runOne[0], spec.runOne[1].join(' '), spec.verified ? '1' : '0'].join('|'));
});
")

if [ "$program" = "none" ]; then
  echo "no conformance suite is registered for $lang" >&2
  exit 2
fi
if [ "$verified" != "1" ]; then
  echo "the command for $lang ('$program $rest $full') has not been verified against this" >&2
  echo "repository, so it is not run. Flip \`verified\` in sdks/conformance/required.mjs" >&2
  echo "when your suite is green, and this will run it." >&2
  exit 2
fi
if ! command -v "$program" > /dev/null 2>&1; then
  echo "$lang's runner is '$program', which is not on PATH" >&2
  exit 2
fi

dir="$(node -e "import('./sdks/conformance/required.mjs').then(m=>process.stdout.write(m.LANGUAGES['$lang'].dir))")"
echo "running $full in $dir"
echo ""

( cd "$dir" && "$program" $rest "$full" )
status=$?

if [ "$status" -ne 0 ]; then
  echo "" >&2
  echo "$full FAILED (exit $status)" >&2
  exit 1
fi

# A green test is not the whole bar. If the suite wrote a report, it is checked
# against the manifest here so "passed but skipped three required fixtures" is
# caught in the same run rather than at release.
report="sdks/fixtures/results/${lang}.json"
if [ -f "$report" ]; then
  echo ""
  if ! node sdks/conformance/check-languages.mjs --check "$lang"; then
    echo "" >&2
    echo "$full passed but $lang does not meet the 100% required-fixture bar" >&2
    exit 1
  fi
else
  echo ""
  echo "note: $lang wrote no report at $report, so its fixture coverage was not checked."
  echo "      A suite writes one listing the fixtures it ran; without it the 100% bar"
  echo "      (design §44 §10.4) cannot be verified for $lang."
fi

exit 0
