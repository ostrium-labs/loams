#!/usr/bin/env bash
# The upgrade gate (design §54 §14, D941; UP1 Task 20): the previous release's
# stack, with fixture data in every product, upgraded to the candidate, checked,
# rolled back and checked again.
#
#   scripts/release/upgrade-gate.sh WORKDIR
#
# The candidate is the newest release of release/upgrade-path.toml and the
# previous release its `from`. The script downloads the previous release's
# compose bundle (gh, GH_TOKEN), starts it with deploy/release/ci/compose.upgrade.yaml
# as compose.override.yaml, writes the fixtures (conformance/upgrade), builds
# the candidate's `loams` image from this checkout (build.sh) under the
# candidate's version, and drives `loams-upgrade`:
#
#   1. upgrade, check every fixture, write one new item per product, check it;
#   2. rollback from the snapshot; the fixtures read as written, the new items
#      are gone;
#   3. when every migration of the step is Reversible: upgrade again, write
#      new items, `rollback --keep-writes`: the previous release reads the
#      fixtures and the new items (the reversibility check).
#
# WORKDIR gets state.json, check.json, upgrade-*.json, rollback-*.json,
# timings.json and every service's logs. Containers and volumes are the
# compose project's (COMPOSE_PROJECT_NAME, default `loams`); `down -v` at the end.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="${1:?usage: upgrade-gate.sh WORKDIR}"
mkdir -p "$work/state"
work="$(cd "$work" && pwd)"
engine="$(command -v docker || command -v podman)"
timings="$work/timings.json"
echo "{}" > "$timings"

train() {
  python3 - "$root/release/upgrade-path.toml" "$1" <<'PY'
import re, sys, tomllib
releases = tomllib.load(open(sys.argv[1], "rb"))["release"]
newest = releases[0]
src = newest["from"]
if sys.argv[2] == "candidate":
    print(newest["version"])
elif re.fullmatch(r"\d+\.\d+", src):
    # A patch release declares its line (`0.1`): the previous release is the
    # newest earlier stable release of that line.
    print(next(r["version"] for r in releases[1:]
               if r["version"].startswith(src + ".") and "-" not in r["version"]))
else:
    print(src)
PY
}
candidate="$(train candidate)"
previous="$(train previous)"
classes="$(python3 - "$root/release/upgrade-path.toml" "$candidate" <<'PY'
import sys, tomllib
path = tomllib.load(open(sys.argv[1], "rb"))
rel = next(r for r in path["release"] if r["version"] == sys.argv[2])
print(" ".join(sorted({m["class"] for m in rel.get("migration", [])})) or "reversible")
PY
)"
echo "upgrade-gate: $previous -> $candidate (migration classes: $classes)"
# The House and agentd run on the previous release's published images: the
# gate builds only the candidate's `loams` image, and the candidate's fabric
# and agentd images are published with the release. (Releases before 0.1.1
# run neither by default, and read neither variable.)
export HOUSE_IMAGE="ghcr.io/ostrium-labs/loams-fabric:$previous"
export AGENTD_IMAGE="ghcr.io/ostrium-labs/loams-agentd:$previous"

timed() {
  local name="$1"; shift
  local started; started="$(date +%s)"
  "$@"
  python3 - "$timings" "$name" "$(( $(date +%s) - started ))" <<'PY'
import json, sys
t = json.load(open(sys.argv[1])); t[sys.argv[2]] = int(sys.argv[3])
json.dump(t, open(sys.argv[1], "w"), indent=2)
PY
}

# 1. The previous release, from its published bundle.
dl="$work/download"
mkdir -p "$dl"
gh release download "v$previous" -R "${GITHUB_REPOSITORY:-ostrium-labs/loams}" -D "$dl" --clobber \
  -p "loams-compose-$previous.tar.gz" -p SHA256SUMS
(cd "$dl" && sha256sum -c --ignore-missing SHA256SUMS)
tar -xzf "$dl/loams-compose-$previous.tar.gz" -C "$work"
stack="$work/loams-compose-$previous/release"
cp "$stack/.env.example" "$stack/.env"
# A previous release from before the light default (0.1.0 and earlier) has no
# `tikv` or `rustfs` profile: PD, TiKV and RustFS always run and hold its data.
# An upgrade never moves a stack's stores (D1340, D1343), and the candidate's
# default is the embedded store and local folders, so the gate names both
# profiles in .env, as such a stack must before it upgrades. A previous
# release with the profiles (0.1.1 on) defaults to the embedded stores, which
# `loams upgrade` does not upgrade yet (stack_unrecognised; the manual path is
# docs/release/upgrades.md), so the gate adds `tikv` and `rustfs` to its
# profiles too: it gates the path `loams upgrade` supports.
if ! grep -q '^COMPOSE_PROFILES=' "$stack/.env"; then
  echo "COMPOSE_PROFILES=tikv,rustfs" >> "$stack/.env"
else
  python3 - "$stack/.env" <<'PY'
import sys
path = sys.argv[1]
lines = open(path).read().splitlines()
for i, line in enumerate(lines):
    if line.startswith("COMPOSE_PROFILES="):
        have = [p for p in line.split("=", 1)[1].split(",") if p.strip()]
        lines[i] = "COMPOSE_PROFILES=" + ",".join(have + [p for p in ("tikv", "rustfs") if p not in have])
open(path, "w").write("\n".join(lines) + "\n")
PY
fi
grep '^COMPOSE_PROFILES=' "$stack/.env"
cp "$root/deploy/release/ci/compose.upgrade.yaml" "$stack/compose.override.yaml"
compose() { "$engine" compose --project-directory "$stack" "$@"; }

# The key file is written as this user and made readable to the engine's uid
# (it holds key hashes only).
key="$("$engine" run --rm -u "$(id -u):$(id -g)" -v "$stack/secrets:/s" "ghcr.io/ostrium-labs/loams:$previous" \
  auth new-key --label upgrade-gate --append /s/api-keys 2>/dev/null)"
echo "::add-mask::$key"
chmod 0644 "$stack/secrets/api-keys" 2>/dev/null || true
# The House gives no access to a principal HOUSE_PRINCIPALS does not list
# (HS1 Task 20), so the fixtures' key (`lk_<id>_<secret>`, principal
# `apikey:<id>`) gets the scopes the House fixture uses. A release whose
# compose file does not read HOUSE_PRINCIPALS ignores the line.
key_id="${key#lk_}"; key_id="${key_id%%_*}"
if grep -q '^HOUSE_PRINCIPALS=' "$stack/.env"; then
  sed -i '/^HOUSE_PRINCIPALS=/d' "$stack/.env"
fi
echo "HOUSE_PRINCIPALS=apikey:$key_id=read+write+ddl" >> "$stack/.env"

cleanup() {
  compose ps -a > "$work/ps.txt" 2>&1 || true
  for s in $(compose config --services 2>/dev/null); do
    compose logs --no-color "$s" > "$work/log-$s.txt" 2>&1 || true
  done
  # And every container of the project, including those of services the
  # restored release does not define (the candidate's new ones).
  for c in $("$engine" ps -a --filter "label=com.docker.compose.project=${COMPOSE_PROJECT_NAME:-loams}" \
               --format '{{.Names}}' 2>/dev/null); do
    "$engine" logs "$c" > "$work/container-$c.txt" 2>&1 || true
  done
  compose down -v --remove-orphans > /dev/null 2>&1 || true
}
trap cleanup EXIT

timed previous_up compose up -d --wait

fixtures() {
  local loams_id; loams_id="$(compose ps -q loams)"
  "$engine" run --rm --network "container:$loams_id" \
    -v "$root/conformance/upgrade:/fx:ro" -v "$work/state:/state" -e LOAMS_API_KEY="$key" \
    docker.io/library/python:3.12-slim \
    sh -c "pip install -q --disable-pip-version-check --root-user-action ignore -r /fx/requirements.txt && python /fx/run.py $*"
}
timed fixtures_write fixtures write --out /state/state.json

# The candidate: its image from this checkout, and the client.
# Into the store the stack runs from.
case "$engine" in */docker) export LOAMS_BUILD_ENGINE=buildx ;; *) export LOAMS_BUILD_ENGINE=podman ;; esac
timed build_candidate "$root/deploy/release/build.sh" --platforms linux/amd64 --version "$candidate" loams
timed build_client cargo build --locked -p loams-upgrade --bin loams-upgrade
export LOAMS_UPGRADE_TRACE=1
up="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug/loams-upgrade"
bundle="$root/deploy/release"

"$up" check --dir "$stack" --to "$candidate" --bundle "$bundle" > "$work/check.json" 2> "$work/check.log"
timed upgrade "$up" --dir "$stack" --to "$candidate" --bundle "$bundle" --yes > "$work/upgrade-1.json" 2> "$work/upgrade-1.log"
timed fixtures_check fixtures check --state /state/state.json
fixtures write-new --state /state/state.json
fixtures check --state /state/state.json --expect-new-writes present
# The candidate's `migrate` adds the store flags the profiles select.
compose run --rm --no-deps migrate migrate status \
  --bucket=file:///var/lib/loams/bucket --data-dir=/var/lib/loams --output json > "$work/ledger.json" || true

# 2. Rollback from the snapshot.
timed rollback "$up" rollback --dir "$stack" --yes > "$work/rollback-1.json" 2> "$work/rollback-1.log"
fixtures check --state /state/state.json --expect-new-writes absent

# 3. Reversibility, and --keep-writes.
# 0.1.0's startup gate cannot serve data a later release stamped (it predates
# design §54 §17 T14-2), so `--keep-writes` back to it is refused
# (KEEP_WRITES_UNSERVED in crates/loams-upgrade/src/rollback.rs); every later
# previous release gets the reversibility check.
if [ "$classes" = "reversible" ] && [ "$previous" = "0.1.0" ]; then
  echo "upgrade-gate: no --keep-writes run: 0.1.0 cannot start on data a later release stamped"
elif [ "$classes" = "reversible" ]; then
  "$up" --dir "$stack" --to "$candidate" --bundle "$bundle" --yes > "$work/upgrade-2.json" 2> "$work/upgrade-2.log"
  fixtures write-new --state /state/state.json
  timed rollback_keep_writes "$up" rollback --dir "$stack" --keep-writes --yes > "$work/rollback-2.json" 2> "$work/rollback-2.log"
  # The previous release reads the fixtures and the candidate's new writes.
  fixtures check --state /state/state.json --expect-new-writes present
else
  echo "upgrade-gate: a point-of-no-return migration: no --keep-writes run"
fi
echo "upgrade-gate: passed ($previous -> $candidate)"
cat "$timings"
