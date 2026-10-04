#!/usr/bin/env bash
# Runs the conformance suites against **real** servers (SDK1 Task 4).
#
#   sdks/conformance/run.sh                    # both servers, every language
#   sdks/conformance/run.sh typescript         # one language
#   sdks/conformance/run.sh --no-server        # replay the committed corpus only
#   sdks/conformance/run.sh --keep             # leave the servers running
#   sdks/conformance/run.sh --drift            # re-record and fail on a difference
#
# `run-all.sh` is the runner for CI: it verifies the corpus, runs the harness
# tests and every language's six tests against the committed recordings, and costs
# no Rust build. **This** script is the other half — it boots `loams dev` and
# `loams-apps-mock` and runs the same suites against them, which is what catches a
# corpus that has drifted from the API. Design §44 §10.4 names the split: CI
# cannot afford to boot a Rust server for each of the thirteen SDKs on every PR.
#
# ## Failing loudly
#
# Three things make this exit non-zero, and all three used to be invisible:
#
# - **A missing required fixture.** The corpus is verified before anything runs,
#   and `verify-corpus.mjs` fails on a `manifest.json` entry with no file behind
#   it. A suite that passes against a corpus with a hole in it is not a pass.
# - **A case the live server answers differently.** `fixtures_pass_against_real_server`
#   replays every `loams dev` case against the server that is actually running.
# - **A recording that has drifted.** `--drift` re-records into a temporary
#   directory and compares, masking only the fields `manifest.json` declares
#   volatile. It never writes to the corpus.
#
# Rust builds are capped at -j 4 on a 15 GB machine. The target directory is not
# set here: the repository's cargo config already sends it outside the tree, and
# hard-coding it would be wrong on any machine that redirects it again.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

langs=""
keep=0
no_server=0
drift=0
for arg in "$@"; do
  case "$arg" in
    --keep) keep=1 ;;
    --no-server) no_server=1 ;;
    --drift) drift=1 ;;
    -*) echo "unknown flag: $arg" >&2; exit 2 ;;
    *) langs="$langs $arg" ;;
  esac
done
langs="${langs# }"

# ---- 0. the corpus, before anything else -------------------------------------

# Cheapest check first, and the one whose failure explains every other failure.
# A broken corpus makes a language suite meaningless, so nothing runs until it
# verifies.
echo "=== the conformance corpus ==="
if ! node sdks/conformance/verify-corpus.mjs; then
  echo "" >&2
  echo "the corpus does not verify; no server was started and no suite was run" >&2
  exit 1
fi

if [ "$no_server" -eq 1 ] && [ "$drift" -eq 1 ]; then
  echo "--no-server and --drift contradict each other: drift needs a server to compare against" >&2
  exit 2
fi

# ---- 1. the servers ----------------------------------------------------------

# The server's pid, in a file rather than a shell variable.
#
# `endpoint="$(start_server)"` runs the function in a subshell, so a `pid=$!`
# inside it never reaches the parent and cleanup has nothing to kill: the
# server survives the run and the next one collides with it. Writing the pid to a
# file is what makes the trap work.
PID_FILE=""
MOCK_PID_FILE=""
log=""

cleanup() {
  for file in "$PID_FILE" "$MOCK_PID_FILE"; do
    [ -n "$file" ] || continue
    [ -s "$file" ] || continue
    pid="$(cat "$file")"
    if [ "$keep" -eq 1 ]; then
      echo "leaving pid $pid running" >&2
      rm -f "$file"
      continue
    fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    rm -f "$file"
  done
  [ -n "$log" ] && rm -f "$log"
}
trap cleanup EXIT

started=0
if [ "$no_server" -eq 0 ]; then
  echo ""
  echo "=== starting loams dev ==="
  bin="$(cargo build -j 4 -p loams --message-format=json 2>/dev/null \
    | grep -o '"executable":"[^"]*/loams"' \
    | head -1 \
    | cut -d '"' -f 4)"
  if [ -z "$bin" ]; then
    echo "could not build loams" >&2
    exit 2
  fi

  log="$(mktemp)"
  PID_FILE="$(mktemp)"
  # A conformance run wants the API and nothing else, so the optional listeners
  # are switched off: a dev stack that also binds Flight SQL, Qdrant and
  # Elasticsearch fails on a developer machine that already has something on 8082.
  datadir="$(mktemp -d)"
  # `loams dev` does not take port 0, so a free port is chosen here. Two
  # concurrent runs must not collide, which is why this is a probe and not a
  # constant.
  endpoint=""
  for _ in $(seq 1 10); do
    port=$((20000 + RANDOM % 20000))
    "$bin" dev --listen "127.0.0.1:$port" --data-dir "$datadir" \
      --no-flight-sql --no-qdrant --no-es >> "$log" 2>&1 &
    echo "$!" > "$PID_FILE"
    pid="$!"
    for _ in $(seq 1 100); do
      endpoint="$(grep -o "http://127\.0\.0\.1:$port" "$log" | head -1 || true)"
      [ -n "$endpoint" ] && break
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.2
    done
    if [ -n "$endpoint" ]; then
      # The catalogue is the SDK's first call, so wait for the port to answer it
      # rather than for the log line alone.
      for _ in $(seq 1 100); do
        curl -sf -X POST "$endpoint/loams.instance.v1.InstanceService/GetInstance" \
          -H 'content-type: application/json' -d '{}' > /dev/null && break
        sleep 0.2
      done
      break
    fi
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    : > "$PID_FILE"
    endpoint=""
  done
  if [ -z "$endpoint" ]; then
    echo "loams dev did not come up:" >&2
    cat "$log" >&2
    exit 2
  fi
  export LOAMS_TEST_ENDPOINT="$endpoint"
  echo "loams dev on $endpoint"
  started=1

  echo ""
  echo "=== starting loams-apps-mock ==="
  # The second conformance target, and the one `loams dev` cannot replace: the
  # app packages, a real idempotency-key replay, and a resumable watch stream
  # (design §44 §10.4, D617). It binds loopback only, on its own port.
  mock_bin="$(LOAMS_APPS_MOCK_BIN="${LOAMS_APPS_MOCK_BIN:-}" node -e "
import('./sdks/conformance/apps-mock.mjs').then(async (m) => {
  process.stdout.write((await m.findAppsMockBin()) ?? '');
});
")"
  if [ -z "$mock_bin" ]; then
    echo "could not build loams-apps-mock; the app-mock fixtures cannot be recorded or live-tested" >&2
    exit 2
  fi
  MOCK_PID_FILE="$(mktemp)"
  mock_log="$(mktemp)"
  mock_port=$((20000 + RANDOM % 20000))
  "$mock_bin" --listen "127.0.0.1:$mock_port" --heartbeat-secs 2 > "$mock_log" 2>&1 &
  echo "$!" > "$MOCK_PID_FILE"
  mock_endpoint=""
  for _ in $(seq 1 100); do
    mock_endpoint="$(grep -o 'http://127\.0\.0\.1:'"$mock_port" "$mock_log" | head -1 || true)"
    [ -n "$mock_endpoint" ] && break
    kill -0 "$(cat "$MOCK_PID_FILE")" 2>/dev/null || break
    sleep 0.1
  done
  if [ -z "$mock_endpoint" ]; then
    echo "loams-apps-mock did not come up:" >&2
    cat "$mock_log" >&2
    rm -f "$mock_log"
    exit 2
  fi
  echo "loams-apps-mock on $mock_endpoint"
  export LOAMS_APPS_MOCK_ENDPOINT="$mock_endpoint"
  rm -f "$mock_log"
else
  echo ""
  echo "--no-server: replaying sdks/fixtures/recorded"
  unset LOAMS_TEST_ENDPOINT || true
fi

# ---- 2. the suites -----------------------------------------------------------

echo ""
echo "=== the language suites ==="
if [ -n "$langs" ]; then
  # One language, one command: the per-language path so a failure names the
  # language rather than the runner.
  sdks/conformance/run-all.sh $langs
  status=$?
else
  sdks/conformance/run-all.sh
  status=$?
fi

# ---- 3. has the corpus drifted? ----------------------------------------------

if [ "$drift" -eq 1 ]; then
  echo ""
  echo "=== drift ==="
  # Re-records into a temporary directory and compares with the volatile fields
  # masked. Nothing is written to the corpus, so a failure leaves the tree clean.
  if node sdks/conformance/verify-corpus.mjs --drift; then
    drift_status=0
  else
    drift_status=1
    status=1
  fi
elif [ "$started" -eq 1 ] && [ "$keep" -eq 0 ]; then
  echo ""
  echo "the corpus should still match what the servers just answered; run"
  echo "  sdks/conformance/run.sh --drift"
  echo "to re-record into a temporary directory and fail on a difference."
fi

exit "$status"
