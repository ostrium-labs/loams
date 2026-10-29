#!/usr/bin/env bash
# scripts/byoc/confluent-tables.sh (plan CC1 Task 5) against a recorded, trimmed
# env_metadata response (tests/byoc/fixtures/confluent/env_metadata.json: seven
# regions, zones and account fields removed). No network, no key.
# shellcheck disable=SC2015 # A && B || C reports a failed check
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
s="$repo/scripts/byoc/confluent-tables.sh"
fx="$repo/tests/byoc/fixtures/confluent/env_metadata.json"
tmp="$(mktemp -d "${TMPDIR:-/var/tmp}/confluent-tables.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }

# the committed tables pass
"$s" check >"$tmp/out" 2>&1 && ok "committed tables pass check" || { bad "committed tables fail check"; cat "$tmp/out"; }

# scratch runs below never touch the module's committed copy
export CONFLUENT_TABLES_MODULE_JSON="$tmp/module.json"

# the module's JSON copy: gen writes it, check fails when it is stale
CONFLUENT_TABLES_MODULE_JSON="$tmp/gen.json" "$s" gen >/dev/null
diff -q "$tmp/gen.json" "$repo/deploy/tofu/modules/confluent/confluent-tables.json" >/dev/null && ok "module copy matches the tables" || bad "deploy/tofu/modules/confluent/confluent-tables.json is stale"
jq '.prices_read = "1999-01-01"' "$tmp/gen.json" >"$tmp/stale.json"
! CONFLUENT_TABLES_MODULE_JSON="$tmp/stale.json" "$s" check >"$tmp/out" 2>&1 && grep -q 'FAIL module_copy_is_current' "$tmp/out" \
  && ok "check fails when the module copy is stale" || { bad "a stale module copy passed check"; cat "$tmp/out"; }

# refresh from the fixture into a scratch copy
mkdir -p "$tmp/t"
cp "$repo/deploy/byoc/confluent/prices.toml" "$tmp/t/"
CONFLUENT_TABLES_DIR="$tmp/t" CONFLUENT_TABLES_TODAY=2026-10-26 "$s" refresh --from "$fx" >/dev/null
r="$tmp/t/regions.toml"
python3 -I - "$r" <<'PY' && ok "refresh_keeps_cloud_region_types_schedulable" || bad "refresh output: $(cat "$r")"
import sys, tomllib
t = tomllib.load(open(sys.argv[1], "rb"))
rows = {(x["cloud"], x["region"]): x for x in t["region"]}
assert t["read"] == "2026-10-26" and t["source"].startswith("https://")
assert set(rows) == {("aws","us-east-1"),("aws","me-south-1"),("aws","ap-south-1"),("gcp","us-central1"),("gcp","europe-north2"),("azure","eastus"),("azure","jioindiacentral")}, rows.keys()
assert rows[("aws","us-east-1")]["types"] == ["basic","dedicated","enterprise","freight","standard"]
assert rows[("aws","me-south-1")] == {"cloud":"aws","region":"me-south-1","schedulable":False,"types":[]}
assert "basic" not in rows[("gcp","europe-north2")]["types"] and "standard" not in rows[("gcp","europe-north2")]["types"]
assert rows[("azure","jioindiacentral")]["types"] == ["dedicated"] and not rows[("azure","jioindiacentral")]["schedulable"]
assert all("unknown" not in x["types"] for x in t["region"])
assert all(set(x) == {"cloud","region","schedulable","types"} for x in t["region"])
PY
CONFLUENT_TABLES_DIR="$tmp/t" CONFLUENT_TABLES_TODAY=2026-10-26 "$s" check >/dev/null 2>&1 && ok "regions_cover_by1_defaults (fixture)" || bad "the fixture's BY1 defaults are not covered"

# a BY1 default missing fails
python3 -I - "$r" "$tmp/t/regions.toml.new" <<'PY'
import sys
src = open(sys.argv[1]).read().split("\n\n")
open(sys.argv[2], "w").write("\n\n".join(b for b in src if 'region = "eastus"' not in b))
PY
mv "$tmp/t/regions.toml.new" "$r"
! CONFLUENT_TABLES_DIR="$tmp/t" CONFLUENT_TABLES_TODAY=2026-10-26 "$s" check >"$tmp/out" 2>&1 && grep -q 'azure eastus is missing' "$tmp/out" \
  && ok "check fails when a BY1 default region is missing" || { bad "missing eastus not caught"; cat "$tmp/out"; }

# stale prices fail; fresh pass
! CONFLUENT_TABLES_TODAY=2027-02-01 "$s" check >"$tmp/out" 2>&1 && grep -q 'prices_are_fresh' "$tmp/out" \
  && ok "prices_are_fresh fails past 90 days" || { bad "stale prices not caught"; cat "$tmp/out"; }

# a price without a source fails
CONFLUENT_TABLES_DIR="$tmp/t" CONFLUENT_TABLES_TODAY=2026-10-26 "$s" refresh --from "$fx" >/dev/null
grep -v '^source = "https://www.confluent.io' "$repo/deploy/byoc/confluent/prices.toml" >"$tmp/t/prices.toml"
! CONFLUENT_TABLES_DIR="$tmp/t" CONFLUENT_TABLES_TODAY=2026-10-26 "$s" check >"$tmp/out" 2>&1 && grep -q 'prices_have_source_and_date' "$tmp/out" \
  && ok "prices_have_source_and_date fails without a source" || { bad "missing source not caught"; cat "$tmp/out"; }

# diff: unchanged 0, changed 3; a changed shape exits 4
cp "$repo/deploy/byoc/confluent/prices.toml" "$tmp/t/"
CONFLUENT_TABLES_DIR="$tmp/t" CONFLUENT_TABLES_TODAY=2026-10-27 "$s" diff --from "$fx" >/dev/null && ok "diff: same rows on another day exit 0" || bad "diff reported a change for the same rows"
jq '(.clouds[0].regions[0].type) |= map(select(.name != "BASIC"))' "$fx" >"$tmp/changed.json"
set +e; CONFLUENT_TABLES_DIR="$tmp/t" "$s" diff --from "$tmp/changed.json" >/dev/null; rc=$?; set -e
[[ "$rc" == 3 ]] && ok "diff: a changed region list exits 3" || bad "diff exit $rc, want 3"
jq '{clouds: [{id: "aws", regions: [{name: "x"}]}]}' "$fx" >"$tmp/shape.json"
set +e; CONFLUENT_TABLES_DIR="$tmp/t" "$s" diff --from "$tmp/shape.json" >/dev/null 2>&1; rc=$?; set -e
[[ "$rc" == 4 ]] && ok "a changed response shape exits 4 (the weekly job opens an issue)" || bad "shape change exit $rc, want 4"

# refresh never runs without a key, and never echoes one
set +e; out="$(env -u CONFLUENT_CLOUD_API_KEY -u CONFLUENT_CLOUD_API_SECRET CONFLUENT_TABLES_DIR="$tmp/t" "$s" refresh 2>&1)"; rc=$?; set -e
[[ "$rc" != 0 ]] && grep -q 'CONFLUENT_CLOUD_API_KEY is not set' <<<"$out" && ok "refresh needs the Cloud key" || bad "refresh without a key: $out"
exit "$fail"
