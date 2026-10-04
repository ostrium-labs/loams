#!/usr/bin/env bash
# Generate one language's SDK (design §44 §10.1, SDK1 Task 1's
# `scripts/sdk/gen.sh <lang>`).
#
#   scripts/sdk/gen.sh typescript       # @loams/client's facade
#   scripts/sdk/gen.sh typescript-live  # @loams/live's stubs
#   scripts/sdk/gen.sh python           # the `loams` package's facade and stubs
#   scripts/sdk/gen.sh go               # module loams.dev/go's facade and stubs
#   scripts/sdk/gen.sh rust             # crate loams' facade
#
# The facade generator is a Rust binary (SDK1 Task 3), so the script builds it
# first and points buf at wherever cargo put it: the repository's cargo config
# sends build output outside the tree, so the path cannot be committed in the
# template, and buf does not expand environment variables in a `local:` plugin
# path. So the script substitutes it into a copy of the template.
#
# CI runs this and fails on a diff (SDK1's global constraint: generated code is
# never hand-edited). `scripts/sdk/drift.sh` is the CI-side wrapper; this script
# is the developer-side one, and the two run the same steps in the same order.
set -euo pipefail

lang="${1:-}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

# Rust builds are capped at -j 4 on a 15 GB machine; the SDK pipeline's jobs are
# the light ones and must not oversubscribe it.
jobs="${LOAMS_BUILD_JOBS:-4}"

# Substituted template copies live in temporary directories; they are removed on
# the way out however the script ends.
workdirs=()
cleanup() {
  local dir
  for dir in "${workdirs[@]:-}"; do
    [ -n "$dir" ] && rm -rf "$dir"
  done
}
trap cleanup EXIT

# buf is pinned as a devDependency of @loams/client (the package that owns
# generation) rather than taken from the machine: a generation difference must
# be a dependency bump in a PR, not whatever the runner had installed. A buf on
# PATH is the fallback for a developer who has not run `pnpm install` in web/.
BUF="sdks/typescript/packages/client/node_modules/.bin/buf"
if [ ! -x "$BUF" ]; then
  if command -v buf >/dev/null 2>&1; then
    BUF="$(command -v buf)"
  else
    echo "$BUF is missing: run 'pnpm install' in web/ first" >&2
    exit 1
  fi
fi

plugin_path() {
  cargo build -p loams-facade-gen -j "$jobs" --message-format=json 2>/dev/null \
    | grep -o '"executable":"[^"]*protoc-gen-loams-facade"' \
    | head -1 \
    | cut -d '"' -f 4
}

# buf only reads a template whose file name is buf.gen.yaml (or
# buf.<name>.yaml), so a substituted copy needs a directory of its own. Every path
# inside a template is relative to $root, not to the template, so a temporary
# directory changes nothing else.
facade_template() {
  local template="$1" plugin="$2" workdir
  workdir="$(mktemp -d)"
  workdirs+=("$workdir")
  sed "s|\${LOAMS_FACADE_PLUGIN}|$plugin|" "$template" > "$workdir/buf.gen.yaml"
  echo "$workdir/buf.gen.yaml"
}

generate_facade() {
  # `plugin` is deliberately NOT localised: it shadows the global the callers
  # set, and `set -u` then aborts on the empty local.
  local template="$1" out
  out="$(facade_template "$template" "$plugin")"
  "$BUF" generate --template "$out"
}

# The facade of a language, given its manifest. `sdk_templates` in
# `sdks/templates/<lang>/template.env` is the one place that says what runs.
case "$lang" in
  typescript)
    plugin="$(plugin_path)"
    if [ -z "$plugin" ]; then
      echo "could not find protoc-gen-loams-facade after cargo build" >&2
      exit 1
    fi
    generate_facade sdks/typescript/buf.gen.yaml "$plugin"
    echo "generated sdks/typescript/packages/client/src/gen/facade.ts"
    ;;
  typescript-live)
    # protoc-gen-es is a Node plugin pinned by @loams/client; buf.gen.live.yaml
    # points at its binary.
    "$BUF" generate --template sdks/typescript/buf.gen.live.yaml
    echo "generated sdks/typescript/packages/live/src/gen"
    ;;
  python | go | rust)
    plugin="$(plugin_path)"
    if [ -z "$plugin" ]; then
      echo "could not find protoc-gen-loams-facade after cargo build" >&2
      exit 1
    fi
    # Read the language's manifest, so the templates and the order come from
    # `sdks/templates/<lang>/template.env` rather than from a second list here.
    manifest="sdks/templates/$lang/template.env"
    if [ ! -f "$manifest" ]; then
      echo "$manifest is missing, so there is nothing to generate for $lang" >&2
      exit 1
    fi
    LANG="" FACADE_TEMPLATE="" STUBS_TEMPLATE="" FACADE_OUT="" STUBS_OUT=""
    STUBS_COMMITTED="" ORDER=""
    # shellcheck disable=SC1090
    . "$manifest"

    # ORDER exists because buf's `clean: true` empties a template's whole `out`
    # directory: Python's stub template cleans `sdks/python/src`, which contains
    # the facade's own directory, so the facade is regenerated after the stubs.
    case "$ORDER" in
      stubs,facade)
        if [ -n "$STUBS_TEMPLATE" ]; then
          "$BUF" generate --template "$STUBS_TEMPLATE"
        fi
        generate_facade "$FACADE_TEMPLATE" "$plugin"
        ;;
      *)
        generate_facade "$FACADE_TEMPLATE" "$plugin"
        if [ -n "$STUBS_TEMPLATE" ]; then
          "$BUF" generate --template "$STUBS_TEMPLATE"
        fi
        ;;
    esac
    echo "generated $FACADE_OUT"
    ;;
  *)
    echo "usage: scripts/sdk/gen.sh <typescript|typescript-live|python|go|rust>" >&2
    exit 2
    ;;
esac