#!/usr/bin/env bash
# DBOS Transact TypeScript on Loams Postgres (DX0 Task 5): its own unit suite
# (`npm run test:unit`, jest) at the pinned tag, then the Loams cases with
# dbos-common/crash/ts/app.mjs. As dbos-py/run.sh; runner dbos-ts.
# Endpoint mode: direct (endpoint-modes.md; checked by selftest.sh recipe_endpoint_mode_matches_suite)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"

suite=dbos-ts
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml" "$OC_OUT/$suite-suite.junit.xml"
dc() { oc_compose --profile dbos "$@"; }
in_runner() { dc exec -T dbos-ts bash -ec "$1"; }
case_() { oc_junit_case "$suite" "$@"; }
URL=postgresql://postgres:${DBOS_DB_PASSWORD:-dbos}@localhost:5432

oc_up dbos
in_runner "
  rm -rf /work/ts /work/appts && mkdir -p /work/ts /work/appts
  curl -fsSL https://codeload.github.com/dbos-inc/dbos-transact-ts/tar.gz/$DBOS_TS_COMMIT | tar xz --strip-components=1 -C /work/ts
"
dc exec -T dbos-ts sh -c 'cat > /work/appts/app.mjs' <"$here/../dbos-common/crash/ts/app.mjs"
dc exec -T dbos-ts sh -c 'cat > /work/appts/package.json' <"$here/../dbos-common/crash/ts/package.json"
in_runner "cd /work/appts && npm install --no-audit --no-fund -q >/dev/null"
app() {
  local -a env=()
  while [ "$1" != -- ]; do env+=(-e "$1"); shift; done
  shift
  dc exec -T "${env[@]}" -w /work/appts dbos-ts node app.mjs "$@"
}

if [[ $skip != *,suite,* ]]; then
  set +e
  in_runner "
    cd /work/ts
    npm ci --no-audit --no-fund >/work/ts.npm.log 2>&1
    npm run build >>/work/ts.npm.log 2>&1
    npm install --no-save --no-audit --no-fund jest-junit@$JEST_JUNIT >>/work/ts.npm.log 2>&1
    JEST_JUNIT_OUTPUT_FILE=/work/ts.junit.xml JEST_JUNIT_CLASSNAME='{filepath}' JEST_JUNIT_TITLE='{title}' \
      timeout 3000 npx jest ./tests --reporters=default --reporters=jest-junit --detectOpenHandles >/work/ts.jest.log 2>&1
  "
  set -e
  dc exec -T dbos-ts cat /work/ts.junit.xml >"$OC_OUT/$suite-suite.junit.xml" 2>/dev/null || true
  dc exec -T dbos-ts tail -n 60 /work/ts.jest.log >"$OC_OUT/$suite-suite.tail.log" 2>/dev/null || true
  if [ -s "$OC_OUT/$suite-suite.junit.xml" ]; then case_ dbos_ts_suite_ran pass; else case_ dbos_ts_suite_ran fail "jest wrote no JUnit"; fi
fi

if [[ $skip != *,cases,* ]]; then
  L=ts
  # shellcheck source=conformance/compute/dbos-common/cases.sh
  . "$here/../dbos-common/cases.sh"
  dbos_cases dbos-ts polling
fi

files=("$OC_OUT/$suite.junit.xml")
[ -s "$OC_OUT/$suite-suite.junit.xml" ] && files+=("$OC_OUT/$suite-suite.junit.xml")
oc_xfail_check "$suite" "${files[@]}"
