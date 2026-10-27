#!/usr/bin/env bash
# Check that TAILNET_PROVIDER and TAILNET_LOGIN_SERVER in an env file agree.
#
#   deploy/tailnet/check-env.sh [path/to/.env]     (default: ./.env)
#
#   tailscale  TAILNET_LOGIN_SERVER must be empty (hosted Tailscale)
#   headscale  TAILNET_LOGIN_SERVER must be an https:// URL
set -euo pipefail
f=${1:-.env}
# Compose-compatible subset of .env syntax: optional spaces around "=", inline
# "# comments" after an unquoted value, one pair of matching single or double quotes.
get() {
  awk -v key="$1" '
    { line=$0; sub(/^[ \t]+/, "", line) }
    line ~ "^" key "[ \t]*=" {
      v=line; sub("^" key "[ \t]*=[ \t]*", "", v)
      if (v ~ /^"/)      { sub(/^"/, "", v); sub(/".*$/, "", v) }
      else if (v ~ /^\x27/) { sub(/^\x27/, "", v); sub(/\x27.*$/, "", v) }
      else               { sub(/[ \t]+#.*$/, "", v); sub(/^#.*$/, "", v); sub(/[ \t]+$/, "", v) }
      last=v
    }
    END { print last }' "$f"
}
provider=$(get TAILNET_PROVIDER); provider=${provider:-tailscale}
server=$(get TAILNET_LOGIN_SERVER)
case "$provider" in
  tailscale)
    [ -z "$server" ] || { echo "TAILNET_PROVIDER=tailscale but TAILNET_LOGIN_SERVER=$server (must be empty)" >&2; exit 1; } ;;
  headscale)
    case "$server" in https://*) ;; *) echo "TAILNET_PROVIDER=headscale needs TAILNET_LOGIN_SERVER=https://<your headscale>" >&2; exit 1 ;; esac ;;
  *) echo "unknown TAILNET_PROVIDER=$provider (tailscale | headscale)" >&2; exit 1 ;;
esac
echo "tailnet env ok: provider=$provider login_server=${server:-<hosted Tailscale>}"
