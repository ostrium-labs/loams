#!/usr/bin/env bash
# The compute conformance harness's own tests (OC1 OC0 Tasks 1-2, DX1 DX0
# Task 1). No product runs and no image is pulled: these check the harness's
# files, its expected-failure and licence gates on fixtures, and the compose
# override's shape through `compose config`. The pull-request job runs it.
#
#   conformance/compute/selftest.sh            # every test
#   conformance/compute/selftest.sh <name>...  # only those
#   OC_SELFTEST_NO_COMPOSE=1 ...               # skip the compose tests
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=conformance/compute/lib.sh
. "$HERE/lib.sh"
ROOT=$OC_ROOT
TD=$HERE/testdata
XFAIL=(python3 "$ROOT/scripts/compute/xfail_check.py")
SCRATCH=$(mktemp -d)
trap 'rm -rf "$SCRATCH"' EXIT

# The files DX0 and OC0 add, for the content checks.
compute_files() {
  git -C "$ROOT" ls-files --cached --others --exclude-standard -- \
    conformance/compute deploy/release/compose.compute.yaml deploy/release/compute \
    docs/guides/durable docs/guides/compute scripts/compute |
    grep -v -e '^conformance/compute/selftest\.sh$' -e '^conformance/compute/testdata/'
}

# expect_fail <cmd...>: the command must exit non-zero.
expect_fail() { if "$@" >"$SCRATCH/out" 2>&1; then cat "$SCRATCH/out"; return 1; fi; }

# --- The expected-failure gate ----------------------------------------------

xfail_listed_run_passes() {
  "${XFAIL[@]}" "$TD/xfail/run.junit.xml" --xfail "$TD/xfail/listed.toml" >/dev/null
}

xfail_strict_unexpected_pass() {
  expect_fail "${XFAIL[@]}" "$TD/xfail/run.junit.xml" --xfail "$TD/xfail/unexpected-pass.toml"
  grep -q 'unexpected pass: tests.test_core::test_fixed_upstream' "$SCRATCH/out"
}

xfail_unlisted_failure_fails() {
  expect_fail "${XFAIL[@]}" "$TD/xfail/run.junit.xml" --xfail "$TD/xfail/unlisted.toml"
  grep -q 'unlisted failure: tests.test_kafka::test_consume' "$SCRATCH/out"
}

xfail_stale_entry_fails() {
  expect_fail "${XFAIL[@]}" "$TD/xfail/run.junit.xml" --xfail "$TD/xfail/stale.toml"
  grep -q 'stale entry: gone' "$SCRATCH/out"
}

xfail_entries_have_reason_and_issue() {
  expect_fail "${XFAIL[@]}" --validate "$TD/xfail/no-issue.toml"
  grep -qF 'issue` is required' "$SCRATCH/out"
  grep -qF 'reason` is required' "$SCRATCH/out"
  # And every committed xfail.toml passes the same validation.
  local -a lists
  mapfile -t lists < <(cd "$ROOT" && git ls-files --cached --others --exclude-standard -- 'conformance/compute/*/xfail.toml' 'conformance/compute/*/*/xfail.toml')
  if [ "${#lists[@]}" -gt 0 ]; then
    (cd "$ROOT" && "${XFAIL[@]}" --validate "${lists[@]}")
  fi
}

xfail_empty_run_fails() {
  printf '<testsuites/>\n' >"$SCRATCH/empty.xml"
  expect_fail "${XFAIL[@]}" "$SCRATCH/empty.xml" --xfail "$TD/xfail/listed.toml"
  grep -q 'ran nothing' "$SCRATCH/out"
}

junit_case_roundtrip() {
  local OC_OUT=$SCRATCH/junit
  oc_junit_case fixture tests.test_core::test_ok pass
  oc_junit_case fixture a_failure fail "boom <&>"
  oc_junit_case fixture a_skip skip
  python3 - "$OC_OUT/fixture.junit.xml" <<'EOF'
import sys, xml.etree.ElementTree as ET
s = ET.parse(sys.argv[1]).getroot().find("testsuite")
assert (s.get("tests"), s.get("failures"), s.get("skipped")) == ("3", "1", "1"), s.attrib
assert s.find("testcase[@name='a_failure']/failure").get("message") == "boom <&>"
EOF
}

# --- Licences -----------------------------------------------------------------

licence_gate_compute_pins() {
  "$ROOT/scripts/compute/licence_gate.sh" >/dev/null
}

licence_gate_refuses_bsl_fixture() {
  expect_fail "$ROOT/scripts/compute/licence_gate.sh" --pins "$TD/licence/pins.env" --licences "$TD/licence/licences.toml"
  grep -q 'BUSL_PIN: BUSL-1.1 is refused' "$SCRATCH/out"
  # A bump without a re-check fails too.
  sed 's/^GOOD_PIN=1.0.0$/GOOD_PIN=1.0.1/' "$TD/licence/pins.env" | grep -v BUSL >"$SCRATCH/pins.env"
  expect_fail "$ROOT/scripts/compute/licence_gate.sh" --pins "$SCRATCH/pins.env" --licences "$TD/licence/licences.toml"
  grep -q 'GOOD_PIN: the entry was checked for' "$SCRATCH/out"
}

no_licence_key_in_compute_files() {
  local f hits=0
  while IFS= read -r f; do
    [ -f "$ROOT/$f" ] || continue
    if grep -nE 'RW_LICENSE_KEY|BULLMQ_PRO|SPICE_[A-Z_]*(LICEN|KEY)|CONDUCTOR_KEY|conductor_key|dbos-conductor|DBOS_CONDUCTOR' "$ROOT/$f"; then
      echo "no_licence_key_in_compute_files: $f"; hits=1
    fi
  done < <(compute_files)
  return $hits
}

# --- Never on Loams SQL ---------------------------------------------------------

no_loams_sql_in_dx0() {
  local f hits=0
  # The Loams SQL service and port of compose.yaml, and Temporal's MySQL plugin.
  local rx='(^|[^A-Za-z0-9_])(tidb|TIDB)([^A-Za-z0-9_]|$)|:4000([^0-9]|$)|mysql8|MYSQL_SEEDS'
  while IFS= read -r f; do
    [ -f "$ROOT/$f" ] || continue
    case $f in *.md) continue ;; esac # prose may say why not
    if grep -nE "$rx" "$ROOT/$f"; then
      echo "no_loams_sql_in_dx0: $f names Loams SQL or the mysql8 plugin"; hits=1
    fi
  done < <(compute_files)
  return $hits
}

# --- lib.sh ---------------------------------------------------------------------

pg_mode_urls() {
  [ "$(oc_pg_mode direct)" = postgresql://cloud_admin:cloud_admin@compute1:55433/postgres ]
  [ "$(oc_pg_mode direct temporal temporal:temporal)" = postgresql://temporal:temporal@compute1:55433/temporal ]
  [ "$(LOAMS_PG_DIRECT_URL=postgresql://a:b@ep-01abc:55433/x oc_pg_mode direct airflow)" = postgresql://a:b@ep-01abc:55433/airflow ]
  [ "$(oc_pg_mode txn airflow airflow:airflow)" = postgresql://airflow:airflow@pgdog-txn:6432/airflow ]
  expect_fail oc_pg_mode txn airflow
  # No session-pooled endpoint yet: the failure names the issue (#1129).
  expect_fail oc_pg_mode session
  grep -q 'issues/1129' "$SCRATCH/out"
  expect_fail oc_pg_mode pooled
}

pins_are_literal() {
  local line
  while IFS= read -r line; do
    case $line in '' | '#'*) continue ;; esac
    [[ $line =~ ^[A-Z][A-Z0-9_]*=[^[:space:]\$\"\']+$ ]] || { echo "not literal: $line"; return 1; }
  done <"$OC_PINS"
}

# --- DBOS (DX0 Task 5; no stack) -------------------------------------------------

# The scripts and the apps parse (no SDK is installed or built here).
dbos_files_parse() {
  local f
  for f in "$HERE"/dbos-common/cases.sh "$HERE"/dbos-*/run.sh; do
    bash -n "$f" || { echo "does not parse: $f"; return 1; }
  done
  python3 -I -c 'import ast, sys; ast.parse(open(sys.argv[1], encoding="utf-8").read())' "$HERE/dbos-common/crash/py/app.py"
  python3 -I -c 'import json, sys; json.load(open(sys.argv[1]))' "$HERE/dbos-common/crash/ts/package.json"
  grep -q '^package main' "$HERE/dbos-common/crash/go/main.go"
  grep -q 'class App' "$HERE/dbos-common/crash/java/src/main/java/loams/App.java"
}

# Each SDK's expected failures are well formed, and its app pins the same
# SDK release as pins.env.
dbos_xfail_and_pins() {
  local l
  for l in py ts go java; do
    "${XFAIL[@]}" --validate "$HERE/dbos-$l/xfail.toml"
  done
  grep -q "\"@dbos-inc/dbos-sdk\": \"$DBOS_TS\"" "$HERE/dbos-common/crash/ts/package.json" || { echo "ts app is not on $DBOS_TS"; return 1; }
  grep -q "dbos-transact-golang v$DBOS_GO\$" "$HERE/dbos-common/crash/go/go.mod" || { echo "go app is not on v$DBOS_GO"; return 1; }
  grep -q "dbos==\$DBOS_PY" "$HERE/dbos-py/run.sh" || { echo "py app does not install dbos==\$DBOS_PY"; return 1; }
  grep -q '"dev.dbos:transact:"' "$HERE/dbos-common/crash/java/build.gradle.kts"
}

# --- The Temporal history corpus (DX0 Task 4; per PR, no server) ----------------

corpus_manifest_complete() {
  python3 "$HERE/temporal/corpus/manifest.py" check
}

normalize_selftest() {
  python3 "$HERE/temporal/corpus/normalize.py" --self-test
}

# --- Airflow (DX0 Task 6; no stack) ----------------------------------------------

# The DAGs and the REST client parse (ast only: nothing is imported or written).
airflow_python_parses() {
  local f
  for f in "$HERE"/airflow/af.py "$HERE"/airflow/dags/*.py "$HERE"/airflow/edge/dags/*.py; do
    python3 -I -c 'import ast, sys; ast.parse(open(sys.argv[1], encoding="utf-8").read(), sys.argv[1])' "$f" ||
      { echo "does not parse: $f"; return 1; }
  done
}

# The two bundles carry the same effects module (common.sh judges both).
airflow_effects_copies_match() {
  cmp "$HERE/airflow/dags/_oc_effects.py" "$HERE/airflow/edge/dags/_oc_effects.py"
}

# af.py turns Airflow 3's structured log events into text lines.
airflow_af_log_lines() {
  python3 -I - "$HERE/airflow/af.py" <<'EOF2'
import importlib.util, sys
spec = importlib.util.spec_from_file_location("af", sys.argv[1])
af = importlib.util.module_from_spec(spec)
spec.loader.exec_module(af)
assert af._log_line({"event": "OC-LOG-MARKER r1", "level": "info"}) == "OC-LOG-MARKER r1"
assert af._log_line("plain line") == "plain line"
assert "level" in af._log_line({"level": "info"})
assert af.run_path("d", "manual__2026-10-27T00:00:00+00:00") == "/api/v2/dags/d/dagRuns/manual__2026-10-27T00%3A00%3A00%2B00%3A00"
EOF2
}

# --- Matrix, docs and the nightly (DX0 Task 7) ------------------------------------

DOCS=("$ROOT/docs/guides/durable" "$ROOT/docs/guides/compute")

# The durable section of the matrix renders the fixture's run as committed (a leg that exited early is red):
# green with an expected failure, red (an unlisted failure, an unexpected
# pass), not run and not merged.
matrix_durable_from_fixture() {
  local m=$TD/matrix-durable
  python3 -I "$ROOT/scripts/compute/matrix.py" durable --compute-dir "$m/compute" --junit-dir "$m/junit" \
    --date 2026-10-27 --loams-version 0.0.0-fixture --commit 0000000 >"$SCRATCH/durable.md"
  diff -u "$m/expected.md" "$SCRATCH/durable.md"
}

# Every `<!-- snippet: path -->` is followed by a code block equal to that
# file, and every snippet file is used by a page.
docs_compute_snippets_match_files() {
  python3 -I - "${DOCS[@]}" <<'EOF2'
import pathlib, re, sys
bad, used = [], set()
for d in map(pathlib.Path, sys.argv[1:]):
    for md in sorted(d.glob("*.md")):
        text = md.read_text(encoding="utf-8")
        for m in re.finditer(r"<!-- snippet: (\S+) -->\n```[a-z]*\n(.*?)```\n", text, re.S):
            f = (md.parent / m.group(1)).resolve()
            used.add(f)
            if not f.is_file():
                bad.append(f"{md}: snippet {m.group(1)} does not exist")
            elif f.read_text(encoding="utf-8") != m.group(2):
                bad.append(f"{md}: the block after {m.group(1)} differs from the file")
        if text.count("<!-- snippet:") != len(re.findall(r"<!-- snippet: (\S+) -->\n```", text)):
            bad.append(f"{md}: a snippet marker not followed by a code block")
    for f in sorted((d / "snippets").glob("*")) if (d / "snippets").is_dir() else []:
        if f.resolve() not in used:
            bad.append(f"{f}: no page uses it")
print("\n".join(bad))
sys.exit(1 if bad else 0)
EOF2
}

# Design 74 D1512 and §68 D1275: the pages never call anything of Loams
# "Temporal-compatible" (the gateway does not exist yet), never promise a
# drop-in, and each states its status.
docs_compute_wording() {
  local d f hits=0 n=0
  for d in "${DOCS[@]}"; do
  for f in "$d"/*.md; do
    [ -f "$f" ] || continue
    n=$((n + 1))
    if grep -niE 'temporal[- ]compatible|drop-in|production[- ]ready' "$f"; then
      echo "docs_compute_wording: $f"; hits=1
    fi
    # DX0's own pages state their status (the gateway's pages, DX1 Task 11's,
    # carry theirs in prose).
    case $(basename "$f") in README.md | airflow.md | dbos.md | temporal-compat-mode.md | compatibility.md) ;; *) continue ;; esac
    if ! grep -q '^Status: ' "$f"; then
      echo "docs_compute_wording: $f has no 'Status:' line"; hits=1
    fi
  done
  done
  [ "$n" -gt 0 ] || { echo "docs_compute_wording: no pages found"; return 1; }
  return $hits
}

# Each recipe's "Endpoint mode: **x**" equals the product's row in
# endpoint-modes.md and the "# Endpoint mode: x" line of its suite's run.sh
# (a suite not on this branch is skipped).
recipe_endpoint_mode_matches_suite() {
  python3 -I - "$ROOT" <<'EOF2'
import pathlib, re, sys
root = pathlib.Path(sys.argv[1])
modes_md = (root / "conformance/compute/endpoint-modes.md").read_text(encoding="utf-8")
table = {m.group(1): m.group(2) for m in re.finditer(r"^\| \*\*([^*]+)\*\*[^|]*\| (\w+) \|", modes_md, re.M)}
recipes = {
    "docs/guides/durable/temporal-compat-mode.md": ("Temporal", ["conformance/compute/temporal/run.sh"]),
    "docs/guides/durable/dbos.md": ("DBOS Transact", [f"conformance/compute/dbos-{l}/run.sh" for l in ("py", "ts", "go", "java")]),
    "docs/guides/durable/airflow.md": ("Airflow", ["conformance/compute/airflow/run.sh"]),
    "docs/guides/compute/airflow.md": ("Airflow", ["conformance/compute/airflow/run.sh"]),
}
bad = []
for page, (product, suites) in recipes.items():
    m = re.search(r"^Endpoint mode: \*\*(\w+)\*\*", (root / page).read_text(encoding="utf-8"), re.M)
    if not m:
        bad.append(f"{page}: no 'Endpoint mode: **x**' line"); continue
    mode = m.group(1)
    if table.get(product) != mode:
        bad.append(f"{page}: {mode}, but endpoint-modes.md says {table.get(product)!r} for {product}")
    for s in suites:
        p = root / s
        if not p.is_file():
            continue
        sm = re.search(r"^# Endpoint mode: (\w+)", p.read_text(encoding="utf-8"), re.M)
        if not sm or sm.group(1) != mode:
            bad.append(f"{page}: {mode}, but {s} says {sm.group(1) if sm else 'nothing'}")
print("\n".join(bad))
sys.exit(1 if bad else 0)
EOF2
}

# The local nightly (scripts/nightly/run-local.sh; owner, 2026-10-28: no
# nightly in CI) runs every product suite on this branch, and the matrix has
# a row for each of its compute legs.
compute_compat_workflow_runs_every_suite() {
  local runner="$ROOT/scripts/nightly/run-local.sh" f hits=0
  for f in durable/run.sh temporal/run.sh temporal/corpus/replay.sh airflow/run.sh \
    dbos-py/run.sh dbos-ts/run.sh dbos-go/run.sh dbos-java/run.sh; do
    grep -qF "conformance/compute/$f" "$runner" || { echo "run-local.sh does not run $f"; hits=1; }
  done
  "$runner" --list | awk '$2 == "compute" { print $1 }' | while read -r leg; do
    grep -qF "leg=\"$leg\"" "$ROOT/scripts/compute/matrix.py" || { echo "matrix.py has no row for leg $leg"; exit 1; }
  done || hits=1
  return $hits
}

# --- The compatibility matrix (DX1 Task 11; DX0 Task 7 adds durable rows) --------

MATRIX=(python3 "$ROOT/scripts/compute/matrix.py")

# A fixture run renders the committed Markdown: a pass, an expected failure
# with its issue, an unlisted failure, a skipped and a missing case, a method
# with no SDK case, and the methods outside the set.
matrix_from_fixture() {
  local m=$TD/matrix
  "${MATRIX[@]}" temporal-gateway --junit "$m/run.junit.xml" --xfail "$m/xfail.toml" \
    --methods "$m/methods.toml" --service "$m/service.proto" --run "fixture, 2026-10-27" >"$SCRATCH/matrix.md"
  diff -u "$m/expected.md" "$SCRATCH/matrix.md"
  # A methods file naming a method the service lacks is refused.
  sed 's/^name = "RegisterNamespace"$/name = "NoSuchMethod"/' "$m/methods.toml" >"$SCRATCH/bad.toml"
  expect_fail "${MATRIX[@]}" temporal-gateway --junit "$m/run.junit.xml" --methods "$SCRATCH/bad.toml" --service "$m/service.proto"
  grep -q NoSuchMethod "$SCRATCH/out"
}

# The guide's matrix is what matrix.py renders from the committed last run,
# and methods.toml lists exactly the frontend's DX1 set.
matrix_guide_current() {
  local g=$HERE/temporal/gateway
  # last-run.junit.xml and last-run.txt are the combine of last-run.toml's runs.
  python3 "$ROOT/scripts/compute/last_run.py" --check
  "${MATRIX[@]}" temporal-gateway --junit "$g/last-run.junit.xml" --run "$(cat "$g/last-run.txt")" \
    --check "$ROOT/docs/guides/durable/temporal-gateway.md"
  python3 - "$g/methods.toml" "$ROOT/crates/loams-temporal-frontend/src/capabilities.rs" <<'PY'
import re, sys, tomllib
listed = [m["name"] for m in tomllib.load(open(sys.argv[1], "rb"))["method"]]
src = open(sys.argv[2], encoding="utf-8").read()
impl = re.findall(r'"(\w+)"', src.split("pub const IMPLEMENTED")[1].split("];")[0])
assert sorted(listed) == sorted(impl), sorted(set(listed) ^ set(impl))
PY
}

# --- The compose override (OC0 Task 2's tests) -----------------------------------

PROFILES=(temporal dbos airflow)

# compose_json [--txn] [profile]: the resolved config as JSON.
compose_json() {
  local txn=()
  [ "${1:-}" = --txn ] && { txn=(--txn); shift; }
  if [ -n "${1:-}" ]; then
    oc_compose "${txn[@]}" --profile "$1" config --format json
  else
    oc_compose "${txn[@]}" config --format json
  fi
}

compose_compute_config_valid() {
  local p
  for p in "${PROFILES[@]}"; do
    oc_compose --profile "$p" config -q
    oc_compose --txn --profile "$p" config -q
  done
  gw_release config -q
  oc_compose --gateway --profile temporal-gateway config -q
}

# No service starts without a compute profile: the stack's services are the
# same with and without the override, and every service the override adds
# has a profile.
compose_compute_requires_profile() {
  local base with
  base=$( (cd "$OC_RELEASE" && "${COMPOSE[@]}" --env-file "$(oc_env_file)" -f compose.yaml config --services) | sort)
  with=$(oc_compose --txn --gateway config --services | sort)
  [ "$base" = "$with" ] || { diff <(echo "$base") <(echo "$with"); return 1; }
  python3 - "$OC_RELEASE/compose.compute.yaml" "$OC_DIR/pgdog-txn.yaml" "$OC_DIR/temporal-gateway.yaml" <<'EOF'
import json, subprocess, sys
# Read the files themselves (PyYAML is not assumed): every top-level service
# block must carry a `profiles:` line, directly or through a merge anchor.
for path in sys.argv[1:]:
    lines = open(path, encoding="utf-8").read().splitlines()
    in_services, name, has = False, None, {}
    for line in lines:
        if line.startswith("services:"):
            in_services = True; continue
        if in_services and line and not line.startswith(" ") and not line.startswith("#"):
            in_services = False
        if not in_services:
            continue
        if line.startswith("  ") and not line.startswith("   ") and line.rstrip().endswith(":") and not line.lstrip().startswith("#"):
            name = line.strip()[:-1]; has[name] = False; continue
        if name and ("profiles:" in line or "<<: *" in line):
            has[name] = True
    missing = [n for n, ok in has.items() if not ok]
    if missing:
        print(f"{path}: services without a profile: {missing}"); sys.exit(1)
EOF
}

# The override publishes no host port beyond the stack's own.
compose_compute_no_new_host_ports() {
  local p
  compose_json >"$SCRATCH/base.json"
  for p in "${PROFILES[@]}"; do
    compose_json --txn "$p" >"$SCRATCH/$p.json"
    python3 - "$SCRATCH/base.json" "$SCRATCH/$p.json" "$p" <<'EOF'
import json, sys
def ports(path):
    cfg = json.load(open(path))
    return {(s, json.dumps(p, sort_keys=True)) for s, svc in cfg.get("services", {}).items() for p in svc.get("ports", []) or []}
base, prof = ports(sys.argv[1]), ports(sys.argv[2])
new = sorted(prof - base)
if new:
    print(f"profile {sys.argv[3]} publishes new host ports: {new}"); sys.exit(1)
EOF
  done
}

# Every image the override and the overlay add is pinned by digest.
compose_compute_images_pinned_by_digest() {
  local p
  compose_json >"$SCRATCH/base.json"
  for p in "${PROFILES[@]}"; do
    compose_json --txn "$p" >"$SCRATCH/$p.json"
    python3 - "$SCRATCH/base.json" "$SCRATCH/$p.json" "$p" <<'EOF'
import json, re, sys
base = json.load(open(sys.argv[1])).get("services", {})
prof = json.load(open(sys.argv[2])).get("services", {})
bad = [f"{s}: {svc.get('image')}" for s, svc in prof.items()
       if s not in base and not re.search(r"@sha256:[0-9a-f]{64}$", svc.get("image") or "")]
if bad:
    print(f"profile {sys.argv[3]}: images not pinned by digest: {bad}"); sys.exit(1)
EOF
  done
}

# The products reach Postgres only through the direct endpoint by default.
compose_compute_uses_direct_endpoint() {
  compose_json temporal >"$SCRATCH/t.json"
  compose_json airflow >"$SCRATCH/a.json"
  compose_json dbos >"$SCRATCH/d.json"
  python3 - "$SCRATCH/t.json" "$SCRATCH/a.json" "$SCRATCH/d.json" <<'EOF'
import json, sys
t, a, d = (json.load(open(p))["services"] for p in sys.argv[1:])
for s in ("temporal-frontend", "temporal-history", "temporal-matching", "temporal-worker"):
    e = t[s]["environment"]
    assert (e["DB"], e["POSTGRES_SEEDS"], e["DB_PORT"], e["NUM_HISTORY_SHARDS"]) == ("postgres12", "compute1", "55433", "16"), (s, e)
for s in ("airflow-scheduler-1", "airflow-scheduler-2", "airflow-edge-worker-1", "airflow-edge-worker-2"):
    e = a[s]["environment"]
    assert "@compute1:55433/airflow" in e["AIRFLOW__DATABASE__SQL_ALCHEMY_CONN"], (s, e)
    assert e["AIRFLOW__CORE__EXECUTOR"] == "airflow.providers.edge3.executors.EdgeExecutor"
assert d["dbos-pg"]["command"][-1] == "TCP:compute1:55433", d["dbos-pg"]["command"]
for s in ("dbos-py", "dbos-ts", "dbos-go", "dbos-java"):
    assert d[s]["network_mode"] == "service:dbos-pg", (s, d[s].get("network_mode"))
EOF
}

# --- The Temporal gateway's profile (DX1 Task 10; D2030, D2031) ---------------

GW_DIR=$HERE/temporal/gateway

# gw_release <compose args...>: the release profile alone (no conformance
# overlay), with the shipped example config as its bind source.
gw_release() {
  LOAMS_TEMPORAL_CONFIG=$OC_RELEASE/temporal-gateway.toml.example \
    oc_compose --profile temporal-gateway "$@"
}

# The profile publishes no host port, with and without the overlay: the
# gateway has no `ports`, shares the engine's namespace, and the engine
# publishes nothing new with it.
gateway_profile_no_host_ports() {
  compose_json >"$SCRATCH/base.json"
  gw_release config --format json >"$SCRATCH/rel.json"
  oc_compose --gateway --profile temporal-gateway config --format json >"$SCRATCH/gw.json"
  python3 - "$SCRATCH/base.json" "$SCRATCH/rel.json" "$SCRATCH/gw.json" "$GW_DIR/gateway.toml" "$OC_RELEASE/temporal-gateway.toml.example" <<'EOF'
import json, sys, tomllib
base = json.load(open(sys.argv[1]))["services"]
def ports(cfg):
    return {(s, json.dumps(p, sort_keys=True)) for s, svc in cfg.items() for p in svc.get("ports", []) or []}
for path in sys.argv[2:4]:
    gw = json.load(open(path))["services"]
    new = sorted(ports(gw) - ports(base))
    assert not new, f"the temporal-gateway profile publishes host ports: {new}"
    g = gw["temporal-gateway"]
    assert not g.get("ports") and not g.get("expose"), g
    assert g["network_mode"] == "service:loams", g.get("network_mode")
    assert g["profiles"] == ["temporal-gateway"], g["profiles"]
# 0.0.0.0 of the engine's namespace is the compose network, not the host.
for path in sys.argv[4:6]:
    cfg = tomllib.load(open(path, "rb"))
    assert cfg["listen"] == "0.0.0.0:7233" and cfg["insecure_dev"] is False, (path, cfg)
EOF
}

# The profile's shape (Task 10), in compose.compute.yaml itself: the
# `loams_temporal` database that durable-init.sh creates, on the direct
# endpoint, the engine's Resonate on its loopback, a read-only bind of the
# config, and API keys with a task-token key (not insecure_dev).
gateway_profile_shape() {
  gw_release config --format json >"$SCRATCH/rel.json"
  python3 - "$SCRATCH/rel.json" "$GW_DIR/gateway.toml" "$OC_RELEASE/temporal-gateway.toml.example" <<'EOF'
import json, sys, tomllib
svc = json.load(open(sys.argv[1]))["services"]
g, init = svc["temporal-gateway"], svc["compute-init"]
assert "temporal-gateway" in init["profiles"], init["profiles"]
assert init["environment"]["LOAMS_TEMPORAL_DB_USER"] == "loams_temporal", init["environment"]
assert g["depends_on"]["compute-init"]["condition"] == "service_completed_successfully", g["depends_on"]
assert g["depends_on"]["loams"]["condition"] == "service_healthy", g["depends_on"]
assert g["environment"]["LOAMS_TEMPORAL_DATABASE_URL"].endswith("@compute1:55433/loams_temporal"), g["environment"]
assert "LOAMS_TEMPORAL_TOKEN_KEY_1" in g["environment"], g["environment"]
assert g["read_only"] is True and g["cap_drop"] == ["ALL"], g
[v] = [v for v in g["volumes"] if v["target"] == "/etc/loams-temporal/gateway.toml"]
# Some compose versions drop `create_host_path: false` from `config`; the
# source line is checked below.
assert v["type"] == "bind" and v["read_only"] is True and v.get("bind", {}).get("create_host_path", False) is False, v
for path in sys.argv[2:4]:
    cfg = tomllib.load(open(path, "rb"))
    assert cfg["store"] == {"kind": "postgres", "url_env": "LOAMS_TEMPORAL_DATABASE_URL"}, (path, cfg["store"])
    d = cfg["durability"]
    assert d["kind"] == "resonate" and d["url"] == "http://127.0.0.1:8001", (path, d)
    assert cfg["keys"] and cfg["task_token_keys"], f"{path}: the profile runs with API keys and its own task-token key"
EOF
  grep -q '^  database loams_temporal "\$LOAMS_TEMPORAL_DB_USER"$' "$OC_RELEASE/compute/durable-init.sh"
  grep -q '^          create_host_path: false$' "$OC_RELEASE/compose.compute.yaml"
}

# The gateway's image is a Loams release image (D2030): compose.compute.yaml
# names <registry>/loams-temporal at the engine's tag, build.sh and both
# release workflows build and attest it from
# deploy/release/Containerfile.loams-temporal, and the conformance overlay
# builds that same Containerfile locally (never pulled, named localhost/).
gateway_image_is_release_image() {
  gw_release config --format json >"$SCRATCH/rel.json"
  oc_compose --gateway --profile temporal-gateway config --format json >"$SCRATCH/gw.json"
  python3 - "$SCRATCH/rel.json" "$SCRATCH/gw.json" <<'EOF'
import json, re, sys
rel = json.load(open(sys.argv[1]))["services"]
gw = json.load(open(sys.argv[2]))["services"]["temporal-gateway"]
engine = rel["loams"]["image"]
m = re.fullmatch(r"(.+)/loams:(\S+)", engine)
assert m, engine
want = f"{m.group(1)}/loams-temporal:{m.group(2)}"
assert rel["temporal-gateway"]["image"] == want, (rel["temporal-gateway"]["image"], want)
assert gw["image"].startswith("localhost/loams-temporal:"), gw["image"]
assert gw["pull_policy"] == "never", gw.get("pull_policy")
assert gw["build"]["dockerfile"] == "deploy/release/Containerfile.loams-temporal", gw["build"]
EOF
  test -f "$OC_RELEASE/Containerfile.loams-temporal"
  # build.sh accepts loams-temporal and builds it by default; both release
  # workflows build and attest it (other images may sit beside it).
  grep -qE '^    loams\|[a-z|-]*\|loams-temporal\|' "$OC_RELEASE/build.sh"
  grep -qE '^\(\(\$\{#images\[@\]\}\)\) \|\| images=\(.* loams-temporal( |\))' "$OC_RELEASE/build.sh"
  local wf
  for wf in release-images release-github; do
    grep -qE 'image: \[loams, .*loams-temporal(, |\])' "$ROOT/.github/workflows/$wf.yml" ||
      { echo "$wf.yml does not build loams-temporal"; return 1; }
  done
  # The first draft's conformance-only image name is gone.
  if grep -rn --exclude=selftest.sh 'localhost/loams-temporal-conformance' "$ROOT/conformance" "$ROOT/deploy" "$ROOT/.github"; then return 1; fi
}

# The conformance overlay (airflow/compose.test.yaml) on top of the profile:
# it resolves, publishes no host port at all, and the edge workers skip the
# database check while the schedulers keep the direct endpoint.
compose_airflow_test_overlay_valid() {
  OC_COMPOSE_EXTRA="$HERE/airflow/compose.test.yaml" oc_compose --profile airflow --profile rustfs config --format json >"$SCRATCH/af.json"
  OC_COMPOSE_EXTRA="$HERE/airflow/compose.test.yaml" oc_compose --txn --profile airflow --profile rustfs config -q
  python3 - "$SCRATCH/af.json" <<'EOF2'
import json, sys
s = json.load(open(sys.argv[1]))["services"]
for name in ("git-server", "airflow-init", "airflow-api-server", "airflow-scheduler-1", "airflow-scheduler-2",
             "airflow-dag-processor", "airflow-triggerer", "airflow-edge-worker-1", "airflow-edge-worker-2",
             "compute1", "rustfs"):
    assert name in s, f"{name} missing"
    assert not s[name].get("ports"), f"{name} publishes {s[name]['ports']}"
for name in ("airflow-edge-worker-1", "airflow-edge-worker-2"):
    e = s[name]["environment"]
    assert e["CONNECTION_CHECK_MAX_COUNT"] == "0", name
    assert "GitDagBundle" in e["AIRFLOW__DAG_PROCESSOR__DAG_BUNDLE_CONFIG_LIST"], name
for name in ("airflow-scheduler-1", "airflow-scheduler-2", "airflow-triggerer"):
    assert "@compute1:55433/airflow" in s[name]["environment"]["AIRFLOW__DATABASE__SQL_ALCHEMY_CONN"], name
    assert s[name]["environment"]["AIRFLOW__SCHEDULER__TASK_INSTANCE_HEARTBEAT_TIMEOUT"] == "60", name
EOF2
}

TESTS=(
  xfail_listed_run_passes xfail_strict_unexpected_pass xfail_unlisted_failure_fails
  xfail_stale_entry_fails xfail_entries_have_reason_and_issue xfail_empty_run_fails
  junit_case_roundtrip
  licence_gate_compute_pins licence_gate_refuses_bsl_fixture no_licence_key_in_compute_files
  no_loams_sql_in_dx0 pg_mode_urls pins_are_literal
  corpus_manifest_complete normalize_selftest
  airflow_python_parses airflow_effects_copies_match airflow_af_log_lines
  matrix_from_fixture matrix_guide_current matrix_durable_from_fixture
  docs_compute_snippets_match_files docs_compute_wording
  recipe_endpoint_mode_matches_suite compute_compat_workflow_runs_every_suite
)
COMPOSE_TESTS=(
  compose_compute_config_valid compose_compute_requires_profile
  compose_compute_no_new_host_ports compose_compute_images_pinned_by_digest
  compose_compute_uses_direct_endpoint compose_airflow_test_overlay_valid
  gateway_profile_no_host_ports gateway_profile_shape gateway_image_is_release_image
)

TESTS+=(dbos_files_parse dbos_xfail_and_pins)

main() {
  local -a want=("$@") run=() t
  if [ "${#want[@]}" -eq 0 ]; then
    run=("${TESTS[@]}")
    if [ -z "${OC_SELFTEST_NO_COMPOSE:-}" ]; then run+=("${COMPOSE_TESTS[@]}"); fi
  else
    run=("${want[@]}")
  fi
  for t in "${run[@]}"; do
    case " ${COMPOSE_TESTS[*]} " in
      *" $t "*) mapfile -t COMPOSE < <(oc_compose_cmd) ;;
    esac
  done
  local fail=0
  for t in "${run[@]}"; do
    if ! declare -F "$t" >/dev/null; then
      echo "FAIL $t (no such test)"; fail=1; continue
    fi
    # Not in an `if`: errexit is ignored there, even in a subshell.
    set +e
    (set -e; "$t") >"$SCRATCH/$t.log" 2>&1
    local rc=$?
    set -e
    if [ "$rc" -eq 0 ]; then
      echo "ok   $t"
    else
      echo "FAIL $t"; sed 's/^/     /' "$SCRATCH/$t.log"; fail=1
    fi
  done
  return $fail
}

main "$@"
