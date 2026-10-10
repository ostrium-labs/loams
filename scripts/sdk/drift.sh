#!/usr/bin/env bash
# Fail when checked-in generated SDK output no longer matches the protos
# (SDK1 Task 1's `sdk-gen-drift` job; design §44 §10.1 point 3).
#
#   scripts/sdk/drift.sh                 # every language in sdks/templates
#   scripts/sdk/drift.sh python go       # only these
#   scripts/sdk/drift.sh --self-test     # check the checker
#
# "Generated code is never hand-edited" is only true if something notices, so
# this regenerates and compares. What it compares depends on the language, and
# the per-language answer lives in `sdks/templates/<lang>/template.env` rather
# than in this script:
#
#   FACADE_OUT      the generated facade. Always compared: it is one small file
#                   per language and it is the contract thirteen SDKs are
#                   generated from, so it is checked in everywhere.
#   STUBS_COMMITTED "yes" when the language also checks in its generated stubs
#                   (Go, whose mirror repository builds from a tag); "no" when
#                   the package builds them at release time (§44 §10.1 point 3:
#                   PyPI), in which case there is nothing checked in to drift.
#
# A language whose facade is not in the repository yet is **reported, not
# passed**: `sdks/<lang>/` belongs to that language's SDK2 task, and reporting
# "clean" for a file that does not exist would be a check that cannot fail.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

templates_dir="sdks/templates"
# Rust builds are capped at -j 4 on a 15 GB machine; the SDK pipeline's jobs are
# the light ones and must not oversubscribe it.
jobs="${LOAMS_BUILD_JOBS:-4}"

checked=0
skipped=0
problems=0
scratch=""

cleanup() { [ -n "$scratch" ] && rm -rf "$scratch"; }
trap cleanup EXIT

note() { echo "drift: warning: $*" >&2; }
problem() {
  echo "drift: $*" >&2
  problems=$((problems + 1))
}

# The buf CLI, pinned as a devDependency of @loams/client rather than taken from
# the machine: a generation difference must be a dependency bump in a PR, not
# whatever the runner had installed. Falls back to a buf on PATH, which is what
# a developer running this by hand has.
buf_bin() {
  local pinned="sdks/typescript/packages/client/node_modules/.bin/buf"
  if [ -x "$pinned" ]; then
    echo "$pinned"
  elif command -v buf >/dev/null 2>&1; then
    command -v buf
  else
    return 1
  fi
}

plugin_path() {
  cargo build -p loams-facade-gen -j "$jobs" --message-format=json 2>/dev/null \
    | grep -o '"executable":"[^"]*protoc-gen-loams-facade"' \
    | head -1 \
    | cut -d '"' -f 4
}

# Reads one manifest into the caller's namespace. A manifest is shell-safe
# `KEY=value` lines; a value may be empty (Rust has no stub template).
read_manifest() {
  local manifest="$1"
  LANG="" FACADE_TEMPLATE="" STUBS_TEMPLATE="" FACADE_OUT="" STUBS_OUT=""
  STUBS_COMMITTED="" ORDER="" EXTRA_TEMPLATES=""
  # shellcheck disable=SC1090
  . "$manifest"
}

manifests() {
  find "$templates_dir" -mindepth 2 -maxdepth 2 -name template.env | sort
}

# The comparison itself: does `$2` still match what the protos generate?
# `$3` is the facade's path, `$4` the stubs' root (empty when the language does
# not check them in). It is parameterised rather than reading the manifest so the
# self-test can drive it against a throwaway repository.
compare() {
  local name="$1" committed="$2" facade="$3" stubs="${4:-}"
  if [ ! -e "$facade" ]; then
    skipped=$((skipped + 1))
    echo "drift: $name: $facade is not in the repository yet (SDK2 owns sdks/$name/); the facade did generate cleanly, so there is nothing to drift from"
    return
  fi
  if git diff --quiet -- "$facade" && git diff --cached --quiet -- "$facade"; then
    echo "drift: $name: $facade is what the protos generate"
  else
    problem "$name: $facade is stale: run 'scripts/sdk/gen.sh $name' and commit the result"
  fi
  if [ "$committed" = "yes" ] && [ -n "$stubs" ] && [ -d "$stubs" ]; then
    if git diff --quiet -- "$stubs" && git diff --cached --quiet -- "$stubs"; then
      echo "drift: $name: $stubs is what the protos generate"
    else
      problem "$name: $stubs is stale: run 'scripts/sdk/gen.sh $name' and commit the result"
    fi
  fi
}

# Regenerates one language in place and compares.
check_language() {
  local name="$1" out
  read_manifest "$templates_dir/$name/template.env"
  if ! out="$(buf_bin)"; then
    problem "$name: no buf CLI (run 'pnpm install' in web/, or put buf on PATH)"
    return
  fi
  local plugin
  if ! plugin="$(plugin_path)" || [ -z "$plugin" ]; then
    problem "$name: could not find protoc-gen-loams-facade after cargo build"
    return
  fi

  # The facade plugin is a cargo binary and its path cannot be committed (the
  # repository's cargo config sends build output outside the tree), so the
  # template's placeholder is substituted into a copy.
  [ -n "$scratch" ] || scratch="$(mktemp -d)"
  local facade="$scratch/$name.buf.gen.yaml"
  sed "s|\${LOAMS_FACADE_PLUGIN}|$plugin|" "$FACADE_TEMPLATE" > "$facade"

  # Python's stub template cleans the directory the facade lives in, so ORDER
  # says which runs first; `stubs,facade` regenerates the facade after them.
  local -a steps=()
  case "$ORDER" in
    facade,stubs) steps=("$facade" "${STUBS_TEMPLATE:+$scratch/$name.stubs.yaml}") ;;
    stubs,facade) steps=("${STUBS_TEMPLATE:+$scratch/$name.stubs.yaml}" "$facade") ;;
    *) steps=("$facade") ;;
  esac

  [ -n "$STUBS_TEMPLATE" ] || true
  if [ -n "$STUBS_TEMPLATE" ]; then
    cp "$STUBS_TEMPLATE" "$scratch/$name.stubs.yaml"
  fi
  for template in "${steps[@]}"; do
    [ -n "$template" ] || continue
    if ! "$out" generate --template "$template" >/dev/null 2>"$scratch/buf.err"; then
      sed 's/^/  /' "$scratch/buf.err" >&2 || true
      problem "$name: buf generate failed on $template"
      return
    fi
  done
  for extra in $EXTRA_TEMPLATES; do
    [ -n "$extra" ] || continue
    "$out" generate --template "$extra" >/dev/null 2>&1 \
      || note "$name: $extra did not generate (a buf.build rate limit is the usual cause); that output is not drift-checked this run"
  done

  checked=$((checked + 1))
  compare "$name" "$STUBS_COMMITTED" "$FACADE_OUT" "$STUBS_OUT"
}

# --- --self-test -------------------------------------------------------------
# The check has to be able to fail: a drift runner that cannot fail is worse
# than not having one.
self_test() {
  local passed=0 total=0 repo
  scratch="$(mktemp -d)"
  repo="$scratch/repo"

  echo "drift --self-test"
  check() {
    local label="$1" want="$2" got="$3"
    total=$((total + 1))
    if [ "$want" = "$got" ]; then
      passed=$((passed + 1))
    else
      echo "  FAIL $label: wanted $want, got $got" >&2
    fi
  }

  # `compare` asks git whether the file in the working tree still matches what is
  # committed, so the self-test runs against a throwaway repository with one
  # committed file rather than against this one. Nothing outside $scratch is
  # touched, which matters because four agents share this working tree.
  mkdir -p "$repo/gen"
  git -C "$repo" init -q
  printf 'generated\n' > "$repo/gen/facade.py"
  git -C "$repo" add gen/facade.py
  git -C "$repo" -c user.email=drift@example -c user.name=drift commit -qm committed

  # `compare` runs in the caller's directory because it asks git about the
  # working tree, so the self-test changes directory rather than calling it in a
  # subshell: a subshell would discard the counters the assertions read.
  probe() {
    local name="$1" committed="$2" facade="$3" stubs="${4:-}"
    local here="$PWD"
    cd "$repo"
    compare "$name" "$committed" "$facade" "$stubs" >>"$scratch/drift.out" 2>&1
    cd "$here"
  }

  # A file whose regeneration changes nothing is clean.
  : > "$scratch/drift.out"
  problems=0
  skipped=0
  probe python yes "$repo/gen/facade.py"
  check "a regenerated file with no diff is clean" 0 "$problems"

  # A hand-edited file is drift, and the message names the command that fixes it.
  printf 'hand-edited\n' > "$repo/gen/facade.py"
  problems=0
  skipped=0
  probe python yes "$repo/gen/facade.py"
  check "a hand-edited file is drift" 1 "$problems"
  total=$((total + 1))
  if grep -q "scripts/sdk/gen.sh python" "$scratch/drift.out"; then
    passed=$((passed + 1))
  else
    echo "  FAIL the drift message does not name the regeneration command" >&2
  fi

  # A file that is not in the repository is reported, never passed silently.
  problems=0
  skipped=0
  probe python no "$repo/gen/absent.py"
  check "a missing facade is counted as skipped, not as clean" 1 "$skipped"
  check "a missing facade is not a failure on its own" 0 "$problems"

  # The stubs of a language that checks them in are compared too: clean is clean,
  # and an edited one is drift.
  git -C "$repo" checkout -q -- gen/facade.py
  mkdir -p "$repo/genstubs"
  printf 'package genstubs\n' > "$repo/genstubs/a.go"
  git -C "$repo" add genstubs/a.go
  git -C "$repo" -c user.email=drift@example -c user.name=drift commit -qm stubs
  problems=0
  skipped=0
  probe go yes "$repo/gen/facade.py" "$repo/genstubs"
  check "a clean stubs tree adds no problem" 0 "$problems"
  printf '// hand-edited\n' >> "$repo/genstubs/a.go"
  problems=0
  skipped=0
  probe go yes "$repo/gen/facade.py" "$repo/genstubs"
  check "an edited stubs tree is drift" 1 "$problems"

  echo "  $passed/$total passed"
  [ "$passed" -eq "$total" ]
}

selected=()
mode="run"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --self-test)
      mode="self-test"
      shift
      ;;
    *)
      selected+=("$1")
      shift
      ;;
  esac
done

if [ "$mode" = "self-test" ]; then
  self_test
  exit
fi

for manifest in $(manifests); do
  name="$(basename "$(dirname "$manifest")")"
  if [ "${#selected[@]}" -gt 0 ]; then
    found=no
    for wanted in "${selected[@]}"; do
      [ "$wanted" = "$name" ] && found=yes
    done
    [ "$found" = yes ] || continue
  fi
  check_language "$name"
done

if [ "$checked" -eq 0 ] && [ "$problems" -eq 0 ]; then
  problem "no language was checked; is $templates_dir populated?"
fi

echo "drift: $checked language(s) checked, $skipped with no committed output, $problems problem(s)"
if [ "$problems" -ne 0 ]; then
  exit 1
fi