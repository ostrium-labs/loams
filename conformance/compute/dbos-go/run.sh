#!/usr/bin/env bash
# DBOS Transact Go on Loams Postgres (DX0 Task 5): its own suite for the SDK
# package (`./dbos/...`, as the "Run tests" step of its tests.yml, without
# -race; DX1 ruling T5.R2) at the pinned tag, then the Loams cases with
# dbos-common/crash/go. As dbos-py/run.sh; runner dbos-go. Go has no polling
# switch (R0.11), so there is no dbos_go_polling_mode.
# Endpoint mode: direct (endpoint-modes.md; checked by selftest.sh recipe_endpoint_mode_matches_suite)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"

suite=dbos-go
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml" "$OC_OUT/$suite-suite.junit.xml"
dc() { oc_compose --profile dbos "$@"; }
in_runner() { dc exec -T -e GOMAXPROCS=4 -e GOFLAGS=-p=4 dbos-go bash -ec "$1"; }
case_() { oc_junit_case "$suite" "$@"; }
URL=postgresql://postgres:${DBOS_DB_PASSWORD:-dbos}@localhost:5432

oc_up dbos
in_runner "
  rm -rf /work/go /work/appgo && mkdir -p /work/go /work/appgo
  curl -fsSL https://codeload.github.com/dbos-inc/dbos-transact-golang/tar.gz/$DBOS_GO_COMMIT | tar xz --strip-components=1 -C /work/go
"
for f in main.go go.mod go.sum; do
  dc exec -T dbos-go sh -c "cat > /work/appgo/$f" <"$here/../dbos-common/crash/go/$f"
done
in_runner "cd /work/appgo && go build -o /work/appgo/app ."
app() {
  local -a env=()
  while [ "$1" != -- ]; do env+=(-e "$1"); shift; done
  shift
  dc exec -T "${env[@]}" dbos-go /work/appgo/app "$@"
}

if [[ $skip != *,suite,* ]]; then
  set +e
  in_runner "
    go install gotest.tools/gotestsum@v$GOTESTSUM >/work/go.setup.log 2>&1
    # dbos/ is a module of its own; the SDK's CI tests it from there. Its
    # integration/ module (which needs mockery's generated mocks) is not run
    # (DX1 ruling T5.R7).
    cd /work/go/dbos
    timeout 3000 /root/go/bin/gotestsum --junitfile /work/go.junit.xml -- -count=1 -timeout 45m ./... >/work/go.test.log 2>&1
  "
  set -e
  dc exec -T dbos-go cat /work/go.junit.xml >"$OC_OUT/$suite-suite.junit.tmp" 2>/dev/null || true
  # Only a JUnit file with content counts (an empty one is no result).
  if [ -s "$OC_OUT/$suite-suite.junit.tmp" ]; then mv "$OC_OUT/$suite-suite.junit.tmp" "$OC_OUT/$suite-suite.junit.xml"; else rm -f "$OC_OUT/$suite-suite.junit.tmp"; fi
  dc exec -T dbos-go sh -c 'tail -n 30 /work/go.setup.log; tail -n 60 /work/go.test.log' >"$OC_OUT/$suite-suite.tail.log" 2>/dev/null || true
  if [ -s "$OC_OUT/$suite-suite.junit.xml" ]; then case_ dbos_go_suite_ran pass; else case_ dbos_go_suite_ran fail "gotestsum wrote no JUnit"; fi
fi

if [[ $skip != *,cases,* ]]; then
  L=go
  # shellcheck source=conformance/compute/dbos-common/cases.sh
  . "$here/../dbos-common/cases.sh"
  dbos_cases dbos-go
fi

files=("$OC_OUT/$suite.junit.xml")
[ -s "$OC_OUT/$suite-suite.junit.xml" ] && files+=("$OC_OUT/$suite-suite.junit.xml")
oc_xfail_check "$suite" "${files[@]}"
