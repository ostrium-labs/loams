#!/usr/bin/env bash
# HE0 (plan HE1 Tasks 3 and 9): run pages.mjs, Langfuse's main pages in a
# headless browser, after driver.sh has ingested. Playwright is installed at a
# pinned version into $HE0_OUT/.playwright (never committed), with its
# Chromium. Works for langfuse-v3 and langfuse-v4 alike.
#
#   HE0_OUT            the harness's dump directory (default apps/.out)
#   HE0_LANGFUSE_URL   as pages.mjs
#   HE0_DETAIL         as pages.mjs
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT=${HE0_OUT:-$HERE/../.out}
PW="$OUT/.playwright"
PLAYWRIGHT=1.63.0

mkdir -p "$PW"
if [ ! -f "$PW/node_modules/playwright/package.json" ] ||
  ! grep -q "\"version\": \"$PLAYWRIGHT\"" "$PW/node_modules/playwright/package.json"; then
  printf '{"name": "he0-pages", "private": true}\n' >"$PW/package.json"
  (cd "$PW" && npm install --no-audit --no-fund --silent "playwright@$PLAYWRIGHT")
fi
# On a CI runner the browser's system libraries come too (passwordless sudo).
if [ "${CI:-}" = true ]; then
  (cd "$PW" && npx --no-install playwright install --with-deps chromium >/dev/null)
else
  (cd "$PW" && npx --no-install playwright install chromium >/dev/null)
fi

cp "$HERE/pages.mjs" "$PW/pages.mjs"
cd "$PW" && exec node pages.mjs
