#!/usr/bin/env bash
# Validate config/config.yaml with Headscale's own `configtest`, against a
# scratch directory (the real paths are /var/lib/headscale and friends).
#
#   deploy/tailnet/headscale/check-config.sh
#
# Uses $HEADSCALE_BIN if set, else the pinned container image. Fails if the
# configuration does not load. (Policy tests are checked by policy/check.sh.)
set -euo pipefail
DOCKER=${DOCKER:-docker}
here=$(cd "$(dirname "$0")" && pwd)
policy_dir=$(cd "$here/../policy" && pwd)
image="ghcr.io/juanfont/headscale:v0.29.4@sha256:8833f828b414c0907b7e5c71da76473216fe17cce0818a166b536ec552c0903f"
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
sed -e "s|/var/lib/headscale|$work|g" -e "s|/var/run/headscale|$work|g" \
    -e "s|/etc/headscale/policy.hujson|$policy_dir/policy.hujson|" \
    "$here/config/config.yaml" >"$work/config.yaml"
export HEADSCALE_OIDC_CLIENT_SECRET=check HEADSCALE_DERP_SERVER_IPV4=203.0.113.7
if [ -n "${HEADSCALE_BIN:-}" ]; then
  "$HEADSCALE_BIN" configtest -c "$work/config.yaml" >/dev/null
else
  "$DOCKER" run --rm -e HEADSCALE_OIDC_CLIENT_SECRET -e HEADSCALE_DERP_SERVER_IPV4 \
    -v "$work:$work" -v "$policy_dir:$policy_dir:ro" "$image" configtest -c "$work/config.yaml" >/dev/null
fi
echo "config ok"
