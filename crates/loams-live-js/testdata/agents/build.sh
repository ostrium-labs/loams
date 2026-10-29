#!/bin/sh
# Builds the agent-profile fixtures (SPIKE, MS1 Task 1): each
# `<name>/index.{js,ts}` into `<name>/agent.js`, one ES module with
# `loams:agent` left external, and prints the sizes (raw, minified,
# minified + gzip). Bun (`bun build --target=browser --format=esm`) when
# `bun` is on PATH, esbuild 0.25.12 otherwise.
#
# The npm dependencies are installed once, pinned and with install scripts
# off, into $AGENT_FIXTURE_DEPS (default: ../../../../target/agent-fixture-deps).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
deps=${AGENT_FIXTURE_DEPS:-$here/../../../../target/agent-fixture-deps}
if [ ! -d "$deps/node_modules/@agentclientprotocol/sdk" ]; then
  mkdir -p "$deps"
  npm install --prefix "$deps" --no-audit --no-fund --ignore-scripts \
    esbuild@0.25.12 typescript@5.9.3 @agentclientprotocol/sdk@1.8.0 \
    web-streams-polyfill@4.3.0 ai@7.0.137 @ai-sdk/openai-compatible@3.0.67
fi
ln -sfn "$deps/node_modules" "$here/node_modules"
trap 'rm -f "$here/node_modules"' EXIT
esbuild="$deps/node_modules/.bin/esbuild"
for dir in "$here"/*/; do
  name=$(basename "$dir")
  [ "$name" = node_modules ] && continue
  entry=$(ls "$dir"index.ts "$dir"index.js 2>/dev/null | head -1)
  [ -n "$entry" ] || continue
  if command -v bun >/dev/null 2>&1; then
    bun build "$entry" --target=browser --format=esm --external 'loams:agent' --outfile "$dir/agent.js" >/dev/null
    bun build "$entry" --target=browser --format=esm --external 'loams:agent' --minify --outfile "$dir/.min.js" >/dev/null
  else
    "$esbuild" "$entry" --bundle --format=esm --platform=browser --target=es2023 \
      --external:loams:agent --preserve-symlinks --log-level=warning --outfile="$dir/agent.js"
    "$esbuild" "$entry" --bundle --format=esm --platform=browser --target=es2023 \
      --external:loams:agent --preserve-symlinks --log-level=warning --minify --outfile="$dir/.min.js"
  fi
  raw=$(wc -c <"$dir/agent.js")
  min=$(wc -c <"$dir/.min.js")
  gz=$(gzip -9 -c "$dir/.min.js" | wc -c)
  rm -f "$dir/.min.js"
  printf '%-12s raw %8d  min %8d  min+gz %7d\n' "$name" "$raw" "$min" "$gz"
done
