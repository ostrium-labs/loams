#!/usr/bin/env bash
# Temporal's own SQL persistence tests at the pinned server tag, from source
# in the pinned Go image, against Loams Postgres's direct endpoint (DX1
# rulings R0.5, R0.7): TestPQ (plugin postgres12), TestPGX (postgres12_pgx)
# and TestPGXSP (postgres12_pgx in simple-protocol mode). They log in as
# temporal/temporal and create and drop test_* databases (durable-init.sh
# gives the role CREATEDB).
#
#   persistence.sh <suite> [go test -run regexp]
#
# Writes $OC_OUT/<suite>.junit.xml, one case per test and subtest. The
# source and the module cache live in two named volumes (dx0-temporal-src,
# dx0-gocache) so a rerun neither downloads nor rebuilds what it has; remove
# them with `<runtime> volume rm` when done.
set -euo pipefail
# shellcheck source=conformance/compute/lib.sh
. "$(dirname "$0")/../lib.sh"

suite=${1:?persistence.sh <suite> [regexp]}
pattern=${2:-'^(TestPQ|TestPGX|TestPGXSP)$'}
rt=$(oc_container_cmd)
src=dx0-temporal-src-${TEMPORAL_SERVER_COMMIT:0:12}
"$rt" volume create "$src" >/dev/null 2>&1 || true
"$rt" volume create dx0-gocache >/dev/null 2>&1 || true
mkdir -p "$OC_OUT"

# The tag's source, once per commit (a read-only archive, not a clone).
"$rt" run --rm -v "$src:/src" "$GO_IMAGE" sh -ec "
  if [ ! -f /src/.loams-commit ] || [ \"\$(cat /src/.loams-commit)\" != '$TEMPORAL_SERVER_COMMIT' ]; then
    rm -rf /src/* /src/.[!.]* 2>/dev/null || true
    curl -fsSL https://codeload.github.com/temporalio/temporal/tar.gz/$TEMPORAL_SERVER_COMMIT | tar xz --strip-components=1 -C /src
    echo '$TEMPORAL_SERVER_COMMIT' > /src/.loams-commit
  fi"

# -p 4 and GOMAXPROCS=4: the machines this runs on are shared (DX1 T3.R2).
# -parallel 1 (PERSISTENCE_PARALLEL) runs TestPQ, TestPGX and TestPGXSP one
# after another: run together on a shared 4-core runner, some cases hit their
# own 2 s and 30 s deadlines (T3.R5).
set +e
"$rt" run --rm --network "$OC_NETWORK" -v "$src:/src" -v dx0-gocache:/go/pkg -w /src \
  -e POSTGRES_SEEDS="${LOAMS_PG_HOST:-compute1}" -e POSTGRES_PORT="${LOAMS_PG_PORT:-55433}" \
  -e GOMAXPROCS=4 -e GOFLAGS=-p=4 -e CGO_ENABLED=0 \
  "$GO_IMAGE" go test -json -count=1 -timeout 90m -parallel "${PERSISTENCE_PARALLEL:-1}" ./common/persistence/tests/ -run "$pattern" \
  >"$OC_OUT/$suite.gotest.json" 2>"$OC_OUT/$suite.gotest.stderr"
rc=$?
set -e
python3 "$OC_ROOT/scripts/compute/gotest_junit.py" "$suite" "$OC_OUT/$suite.junit.xml" <"$OC_OUT/$suite.gotest.json"
echo "persistence.sh: go test exited $rc (the JUnit file carries the verdict)"
