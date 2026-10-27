#!/usr/bin/env bash
# Every deploy/... path and every relative link in docs/guides/byoc exists, and
# the guides never open an inbound port or ask for a static key.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
guides="$repo/docs/guides/byoc"
fail=0
for f in "$guides"/*.md; do
  # repo-relative paths named in prose or commands
  while IFS= read -r p; do
    p="${p%[.,;:)\`\"\']}"
    p="${p%%\$*}"
    p="${p%/}"
    [[ -n "$p" ]] || continue
    # a template (render.sh's output name, <cloud> placeholders) is not a file
    [[ "$p" == *"<"* || "$p" == *"{"* ]] && continue
    [[ -e "$repo/$p" ]] || { echo "FAIL $(basename "$f"): $p does not exist"; fail=1; }
  done < <(grep -oE '(deploy/(byoc|tofu|release)|tests/byoc|docs/(design|release|guides))/[A-Za-z0-9_./<>*-]+' "$f" | sort -u)
  # relative markdown links
  while IFS= read -r l; do
    l="${l%%#*}"
    [[ -n "$l" && "$l" != http* ]] || continue
    [[ -e "$guides/$l" ]] || { echo "FAIL $(basename "$f"): link $l does not resolve"; fail=1; }
  done < <(grep -oE '\]\([^)]+\)' "$f" | sed 's/^](//; s/)$//' | sort -u)
done
if grep -nE 'authorize-security-group-ingress|0\.0\.0\.0/0|AWS_ACCESS_KEY_ID|AWS_SECRET_ACCESS_KEY|GOOGLE_SERVICE_ACCOUNT_KEY|AZURE_STORAGE_ACCOUNT_KEY|service-accounts keys create|--allow-shared-key-access true' "$guides"/*.md; then
  echo "FAIL a guide opens an inbound port or uses a static key"; fail=1
fi
[[ "$fail" -eq 0 ]] && echo "guides reference existing files"
exit "$fail"
