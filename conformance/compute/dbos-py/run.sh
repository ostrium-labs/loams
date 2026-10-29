#!/usr/bin/env bash
# DBOS Transact Python on Loams Postgres (DX0 Task 5): its own test suite at
# the pinned tag, then the Loams cases with dbos-common/crash/py/app.py.
#
#   conformance/compute/dbos-py/run.sh
#
# Runs in the `dbos` profile's dbos-py runner, which reaches Loams Postgres
# on localhost:5432 through dbos-pg's forwarder (DX1 R0.12, T1.R1). Writes
# $OC_OUT/dbos-py.junit.xml and $OC_OUT/dbos-py-suite.junit.xml, then the
# strict expected-failure check against dbos-py/xfail.toml.
# OC_SKIP=suite,cases skips a part (its entries then read as stale).
# Endpoint mode: direct (endpoint-modes.md; checked by selftest.sh recipe_endpoint_mode_matches_suite)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"

suite=dbos-py
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml" "$OC_OUT/$suite-suite.junit.xml"
dc() { oc_compose --profile dbos "$@"; }
in_runner() { dc exec -T dbos-py bash -ec "$1"; }
case_() { oc_junit_case "$suite" "$@"; }
URL=postgresql://postgres:${DBOS_DB_PASSWORD:-dbos}@localhost:5432

oc_up dbos

# The SDK's source at the pinned commit (a read-only archive, not a clone),
# and the app's own environment with the released package.
in_runner "
  rm -rf /work/py && mkdir -p /work/py /work/app
  curl -fsSL https://codeload.github.com/dbos-inc/dbos-transact-py/tar.gz/$DBOS_PY_COMMIT | tar xz --strip-components=1 -C /work/py
  python -m venv /work/appenv && /work/appenv/bin/pip install -q dbos==$DBOS_PY
"
dc exec -T dbos-py sh -c 'cat > /work/app/app.py' <"$here/../dbos-common/crash/py/app.py"
app() { # app <env...> -- <args...>
  local -a env=()
  while [ "$1" != -- ]; do env+=(-e "$1"); shift; done
  shift
  dc exec -T "${env[@]}" dbos-py /work/appenv/bin/python /work/app/app.py "$@"
}

# 1. The SDK's own suite (pytest), against Loams Postgres (DBOS_DATABASE=POSTGRES).
if [[ $skip != *,suite,* ]]; then
  set +e
  # The suite's own lock first (pdm). The first nightly's pdm 2.29.2 failed
  # with TypeError super(type, obj) while resolving it, so fall back to pip
  # with the project's PEP 735 dev group (unlocked; DX1 ruling T5.R6).
  in_runner "
    cd /work/py
    export PDM_BUILD_SCM_VERSION=$DBOS_PY
    pip install -q pdm==$PDM >/work/py.setup.log 2>&1
    if pdm install -q -G otel >>/work/py.setup.log 2>&1; then
      runner='pdm run'
    else
      echo 'pdm install failed; pip --group dev' >>/work/py.setup.log
      python -m venv /work/pyenv && /work/pyenv/bin/pip install -q pip==$PIP_GROUPS >>/work/py.setup.log 2>&1
      /work/pyenv/bin/pip install -q -e '.[otel]' --group dev >>/work/py.setup.log 2>&1
      runner='/work/pyenv/bin/python -m'
    fi
    timeout 3000 \$runner pytest tests -p no:cacheprovider -q --junitxml=/work/py.junit.xml >/work/py.pytest.log 2>&1
  "
  set -e
  dc exec -T dbos-py cat /work/py.junit.xml >"$OC_OUT/$suite-suite.junit.tmp" 2>/dev/null || true
  # Only a JUnit file with content counts (an empty one is no result).
  if [ -s "$OC_OUT/$suite-suite.junit.tmp" ]; then mv "$OC_OUT/$suite-suite.junit.tmp" "$OC_OUT/$suite-suite.junit.xml"; else rm -f "$OC_OUT/$suite-suite.junit.tmp"; fi
  dc exec -T dbos-py sh -c 'tail -n 30 /work/py.setup.log; tail -n 40 /work/py.pytest.log' >"$OC_OUT/$suite-suite.tail.log" 2>/dev/null || true
  if [ -s "$OC_OUT/$suite-suite.junit.xml" ]; then case_ dbos_py_suite_ran pass; else case_ dbos_py_suite_ran fail "pytest wrote no JUnit"; fi
fi

# 2. The Loams cases (dbos-common/cases.sh); Python has the polling switch.
if [[ $skip != *,cases,* ]]; then
  L=py
  # shellcheck source=conformance/compute/dbos-common/cases.sh
  . "$here/../dbos-common/cases.sh"
  dbos_cases dbos-py polling
fi

files=("$OC_OUT/$suite.junit.xml")
[ -s "$OC_OUT/$suite-suite.junit.xml" ] && files+=("$OC_OUT/$suite-suite.junit.xml")
oc_xfail_check "$suite" "${files[@]}"
