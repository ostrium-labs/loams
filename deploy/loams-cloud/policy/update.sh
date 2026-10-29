#!/usr/bin/env bash
# Fetches the pinned disposable-domain list and checks it against PINS.md.
#   policy/update.sh            verify the vendored file matches the pin
#   policy/update.sh <rev>      fetch <rev>, print its SHA-256 (then update PINS.md by hand)
set -euo pipefail
cd "$(dirname "$0")"
file=disposable_email_blocklist.conf
pinned_rev=$(grep -o '`[0-9a-f]\{40\}`' PINS.md | head -1 | tr -d '`')
pinned_sha=$(grep "^| \`$file\`" PINS.md | grep -o '`[0-9a-f]\{64\}`' | tr -d '`')
if [ $# -eq 0 ]; then
  echo "$pinned_sha  $file" | sha256sum -c -
  exit
fi
rev="$1"
curl -fsSL -o "$file.new" "https://raw.githubusercontent.com/disposable-email-domains/disposable-email-domains/$rev/$file"
if grep -qvE '^[a-z0-9.-]+$' "$file.new"; then echo "unexpected line in $file at $rev" >&2; rm -f "$file.new"; exit 1; fi
mv "$file.new" "$file"
echo "pinned was $pinned_rev; now $rev, sha256 $(sha256sum "$file" | cut -d' ' -f1)"
