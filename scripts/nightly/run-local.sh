#!/usr/bin/env bash
# The nightly, on this machine (owner, 2026-10-28: "do not run nightly in ci
# only local"). It runs the suites the GitHub nightly ran, one leg at a time,
# each heavy step through the shared build-slot limiter, writes JUnit and a
# results JSON per leg, regenerates the compute matrix page from them, and
# removes what it created (compose projects, their volumes, its caches).
#
#   scripts/nightly/run-local.sh                    # every leg
#   scripts/nightly/run-local.sh --legs durable,corpus,temporal-pg16
#   scripts/nightly/run-local.sh --list             # the legs and what they run
#   scripts/nightly/run-local.sh --results DIR      # default: target/nightly/<date>-<commit>
#   scripts/nightly/run-local.sh --no-matrix        # do not touch the matrix page
#   scripts/nightly/run-local.sh --keep             # leave the stacks up (debugging)
#
# Environment:
#   LOAMS_BUILDSLOT   the limiter (default: ~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh
#                     when it exists, else none); every heavy step runs under it
#   CARGO_BUILD_JOBS  cargo's parallelism (default 4)
#   NIGHTLY_LABEL     the matrix's run line (default "local run <date> <commit>")
#
# Results: <results>/compute-<leg>/ holds the leg's JUnit, logs and
# leg.exit; <results>/summary.json lists every leg with its exit status,
# duration and JUnit files. The exit status is non-zero when any leg failed.
# Guide: docs/guides/nightly-local.md. Timer: deploy/nightly/.
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"
DATE=$(date -u +%F)
SHA=$(git rev-parse --short=9 HEAD 2>/dev/null || echo unknown)
RESULTS="$ROOT/target/nightly/$DATE-$SHA"
LEGS=all
MATRIX=1
KEEP=0
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-4}
# Rootless podman without docker: compose (docker-compose or podman compose)
# talks to the user's podman socket.
if ! command -v docker >/dev/null 2>&1 && [ -z "${DOCKER_HOST:-}" ] &&
  [ -S "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock" ]; then
  export DOCKER_HOST="unix://${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/podman/podman.sock"
fi
SLOT=${LOAMS_BUILDSLOT:-$HOME/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh}
[ -x "$SLOT" ] || SLOT=

# --- The legs ------------------------------------------------------------------
# name | kind | command. `compute` legs run a conformance/compute suite in a
# compose project of their own; `cargo` legs are the nightly-only Rust suites
# of ci.yml and gt1-nightly.yml (the rest of the GitHub nightly re-ran the
# per-PR workflows, which the local runners already run on every PR).
LEG_TABLE=$(cat <<'EOF'
durable|compute|conformance/compute/durable/run.sh
corpus|compute|conformance/compute/temporal/corpus/replay.sh
temporal-pg16|compute|COMPUTE_PG_MAJOR=16 conformance/compute/temporal/run.sh
temporal-pg17|compute|COMPUTE_PG_MAJOR=17 conformance/compute/temporal/run.sh
dbos-py|compute|conformance/compute/dbos-py/run.sh
dbos-ts|compute|conformance/compute/dbos-ts/run.sh
dbos-go|compute|conformance/compute/dbos-go/run.sh
dbos-java|compute|conformance/compute/dbos-java/run.sh
airflow|compute|conformance/compute/airflow/run.sh
airflow-local|compute|AIRFLOW_EXECUTOR=LocalExecutor OC_SKIP=edge conformance/compute/airflow/run.sh
spec|cargo|scripts/spec/check.sh --all --nightly
sim|cargo|SIM_SEEDS=1000 cargo test -p loams-sim --release --locked --test it sim::
tail-merge|cargo|TAIL_PROP_CASES=256 cargo test -p loams-query --release --locked --test it tail_vs_rebuild::
git-seeds|cargo|LOAMS_GIT_LIN_SEEDS=1024 cargo test -p loams-git --locked --test linearizable
EOF
)

usage() { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
  case $1 in
    --legs) LEGS=$2; shift 2 ;;
    --results) RESULTS=$2; shift 2 ;;
    --no-matrix) MATRIX=0; shift ;;
    --keep) KEEP=1; shift ;;
    --list) echo "$LEG_TABLE" | awk -F'|' '{ printf "%-14s %-8s %s\n", $1, $2, $3 }'; exit 0 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "run-local.sh: unknown argument $1" >&2; usage >&2; exit 2 ;;
  esac
done
case $RESULTS in /tmp/*) echo "run-local.sh: not under /tmp (a small RAM tmpfs here)" >&2; exit 2 ;; esac
LABEL=${NIGHTLY_LABEL:-local run $DATE $SHA}
mkdir -p "$RESULTS"

selected() { [ "$LEGS" = all ] || [[ ",$LEGS," == *",$1,"* ]]; }
slot() { if [ -n "$SLOT" ]; then "$SLOT" "$@"; else "$@"; fi; }
log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*"; }

# --- Cleaning up ---------------------------------------------------------------
# A compute leg's compose project, every profile and overlay it may have used,
# with its volumes. Only that project: never a prune, never another stack.
clean_compute() {
  local project=$1
  # shellcheck source=conformance/compute/lib.sh
  (
    . "$ROOT/conformance/compute/lib.sh"
    export OC_PROJECT=$project OC_NETWORK=${project}_default
    export OC_COMPOSE_EXTRA=$ROOT/conformance/compute/airflow/compose.test.yaml
    oc_compose --txn --profile temporal --profile dbos --profile airflow --profile rustfs \
      down -v --remove-orphans --timeout 10 >/dev/null 2>&1
  ) || true
}
# The harness's own named volumes (Temporal's source and Go cache) and the
# Git DAG repository volume; removed after the run unless --keep.
clean_volumes() {
  local rt v
  rt=$(command -v docker || command -v podman) || return 0
  for v in $("$rt" volume ls -q 2>/dev/null | grep -E '^(dx0-temporal-src-|dx0-gocache$|ln-[a-z0-9-]+_)'); do
    "$rt" volume rm -f "$v" >/dev/null 2>&1 || true
  done
}

# --- Running a leg -------------------------------------------------------------
SUMMARY="$RESULTS/summary.jsonl"
: >"$SUMMARY"
failed=0

run_leg() {
  local name=$1 kind=$2 cmd=$3 dir rc t0 t1
  dir="$RESULTS/compute-$name"
  rm -rf "$dir"
  mkdir -p "$dir"
  log "---- $name ($kind): $cmd"
  t0=$(date +%s)
  if [ "$kind" = compute ]; then
    local project="ln-$name"
    clean_compute "$project"
    # Each leg in a project of its own, beside any running stack.
    ( export OC_OUT=$dir OC_PROJECT=$project OC_NETWORK=${project}_default
      slot bash -c "set -e; cd '$ROOT'; $cmd" ) >"$dir/leg.log" 2>&1
    rc=$?
    if [ "$KEEP" = 0 ]; then clean_compute "$project"; fi
  else
    ( slot bash -c "set -e; cd '$ROOT'; $cmd" ) >"$dir/leg.log" 2>&1
    rc=$?
    # One JUnit case per cargo leg, so the summary and the matrix agree.
    if [ "$rc" = 0 ]; then
      python3 "$ROOT/scripts/compute/junit.py" add "$dir/$name.junit.xml" "nightly-$name" "$name" pass ""
    else
      python3 "$ROOT/scripts/compute/junit.py" add "$dir/$name.junit.xml" "nightly-$name" "$name" fail "$(tail -n 20 "$dir/leg.log")"
    fi
  fi
  t1=$(date +%s)
  echo "$rc" >"$dir/leg.exit"
  python3 - "$SUMMARY" "$name" "$kind" "$cmd" "$rc" "$((t1 - t0))" "$dir" <<'PY'
import json, pathlib, sys
summary, name, kind, cmd, rc, secs, d = sys.argv[1:8]
junit = sorted(str(p.relative_to(pathlib.Path(d).parent)) for p in pathlib.Path(d).glob("*.junit.xml"))
with open(summary, "a", encoding="utf-8") as f:
    f.write(json.dumps({"leg": name, "kind": kind, "command": cmd, "exit": int(rc),
                        "seconds": int(secs), "junit": junit}) + "\n")
PY
  if [ "$rc" = 0 ]; then log "ok   $name ($((t1 - t0)) s)"; else log "FAIL $name (exit $rc, $((t1 - t0)) s; $dir/leg.log)"; failed=1; fi
}

log "local nightly: $LABEL; results in $RESULTS; limiter ${SLOT:-none}; CARGO_BUILD_JOBS=$CARGO_BUILD_JOBS"
while IFS='|' read -r name kind cmd; do
  [ -n "$name" ] || continue
  selected "$name" || continue
  run_leg "$name" "$kind" "$cmd"
done <<<"$LEG_TABLE"

# --- The results ---------------------------------------------------------------
python3 - "$SUMMARY" "$RESULTS/summary.json" "$LABEL" "$SHA" "$DATE" <<'PY'
import json, sys
legs = [json.loads(l) for l in open(sys.argv[1], encoding="utf-8") if l.strip()]
out = {"label": sys.argv[3], "commit": sys.argv[4], "date": sys.argv[5], "legs": legs,
       "failed": [l["leg"] for l in legs if l["exit"] != 0]}
json.dump(out, open(sys.argv[2], "w", encoding="utf-8"), indent=1)
print(f"summary: {len(legs)} legs, {len(out['failed'])} failed: {', '.join(out['failed']) or 'none'}")
PY
rm -f "$SUMMARY"

if [ "$MATRIX" = 1 ]; then
  # Only the durable section is rendered from these results; the Temporal
  # gateway's section keeps its own committed run (DX1 Task 11).
  python3 "$ROOT/scripts/compute/matrix.py" durable --junit-dir "$RESULTS" --run-label "$LABEL" \
    --splice "$ROOT/docs/guides/compute/compatibility.md" &&
    log "matrix: docs/guides/compute/compatibility.md regenerated ($LABEL)"
  python3 "$ROOT/scripts/compute/matrix.py" durable --junit-dir "$RESULTS" --run-label "$LABEL" \
    --out "$RESULTS/compatibility-durable.md" >/dev/null
fi

if [ "$KEEP" = 0 ]; then clean_volumes; fi
exit "$failed"
