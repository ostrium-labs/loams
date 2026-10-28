#!/usr/bin/env bash
# DBOS Transact Java on Loams Postgres (DX0 Task 5): its own build and tests
# (`./gradlew clean build`, as its test.yml) at the pinned tag on Temurin 21,
# then the Loams cases with dbos-common/crash/java. As dbos-py/run.sh; runner
# dbos-java. Its build reads its version from git history, so the source
# gets a throw-away repository with the pinned tag inside the runner (R0.12).
# Endpoint mode: direct (endpoint-modes.md; checked by selftest.sh recipe_endpoint_mode_matches_suite)
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$here/../lib.sh"

suite=dbos-java
skip=",${OC_SKIP:-},"
mkdir -p "$OC_OUT"
rm -f "$OC_OUT/$suite.junit.xml" "$OC_OUT/$suite-suite.junit.xml"
dc() { oc_compose --profile dbos "$@"; }
in_runner() { dc exec -T -e GRADLE_OPTS='-Dorg.gradle.workers.max=4' -e DBOS_JAVA="$DBOS_JAVA" dbos-java bash -ec "$1"; }
case_() { oc_junit_case "$suite" "$@"; }
URL=postgresql://postgres:${DBOS_DB_PASSWORD:-dbos}@localhost:5432

oc_up dbos
in_runner "
  command -v git >/dev/null || (apt-get update -qq && apt-get install -y -qq git >/dev/null)
  rm -rf /work/java /work/appjava && mkdir -p /work/java /work/appjava/src/main/java/loams
  curl -fsSL https://codeload.github.com/dbos-inc/dbos-transact-java/tar.gz/$DBOS_JAVA_COMMIT | tar xz --strip-components=1 -C /work/java
  cd /work/java && git init -q && git add -A && git -c user.email=ci@example.invalid -c user.name=ci commit -qm 'dbos-transact-java $DBOS_JAVA' && git tag $DBOS_JAVA
"
for f in settings.gradle.kts build.gradle.kts src/main/java/loams/App.java; do
  dc exec -T dbos-java sh -c "cat > /work/appjava/$f" <"$here/../dbos-common/crash/java/$f"
done
in_runner "cd /work/appjava && /work/java/gradlew -q --no-daemon -p /work/appjava installDist >/work/appjava.build.log 2>&1 || { tail -30 /work/appjava.build.log; exit 1; }"
app() {
  local -a env=()
  while [ "$1" != -- ]; do env+=(-e "$1"); shift; done
  shift
  dc exec -T "${env[@]}" dbos-java /work/appjava/build/install/app/bin/app "$@"
}

# The SDK's own suite is not run: at 1.2.0 every test gets its Postgres from
# Testcontainers (utils/PgContainer.java: postgres:18 or a prebaked DBOS image
# in Docker) and has no switch for an external database, so it cannot test
# Loams Postgres without a patch (second nightly: 945 of 1,284 tests failed
# finding Docker; DX1 ruling T5.R9). Recorded as a skipped case.
if [[ $skip != *,suite,* ]]; then
  case_ dbos_java_suite_ran skip "the suite runs only on Testcontainers' own Postgres (T5.R9)"
fi

if [[ $skip != *,cases,* ]]; then
  L=java
  # shellcheck source=conformance/compute/dbos-common/cases.sh
  . "$here/../dbos-common/cases.sh"
  dbos_cases dbos-java polling
fi

files=("$OC_OUT/$suite.junit.xml")
[ -s "$OC_OUT/$suite-suite.junit.xml" ] && files+=("$OC_OUT/$suite-suite.junit.xml")
oc_xfail_check "$suite" "${files[@]}"
