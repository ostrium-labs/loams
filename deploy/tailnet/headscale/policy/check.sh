#!/usr/bin/env bash
# Check the tailnet policy with Headscale's own checker, including its `tests`.
#
#   deploy/tailnet/headscale/policy/check.sh [policy-file]
#
# Uses ./headscale if $HEADSCALE_BIN is set, else the pinned container image.
# A throwaway database is seeded with one node per tag and per operator (the
# checker can only evaluate tests against nodes that exist), so nothing here
# touches a running Headscale. Exit status is the checker's.
set -euo pipefail

DOCKER=${DOCKER:-docker}
here=$(cd "$(dirname "$0")" && pwd)
policy=$(realpath "${1:-$here/../../policy/policy.hujson}")
image="ghcr.io/juanfont/headscale:v0.29.4@sha256:8833f828b414c0907b7e5c71da76473216fe17cce0818a166b536ec552c0903f"

# Run the container as the caller so the scratch database it creates is writable
# by seed.py (Docker on a CI runner); rootless Podman needs keep-id for the same.
uopts="--user $(id -u):$(id -g)"
if "$DOCKER" --version 2>/dev/null | grep -qi podman; then uopts="--userns=keep-id $uopts"; fi

work=$(mktemp -d)
trap '[ "${pid:-0}" -gt 0 ] && kill "$pid" 2>/dev/null; [ -n "${cid:-}" ] && "$DOCKER" stop -t 2 "$cid" >/dev/null 2>&1; [ -n "${KEEP:-}" ] || rm -rf "$work"' EXIT

if [ -n "${HEADSCALE_BIN:-}" ]; then
  hs() { "$HEADSCALE_BIN" -c "$work/config.yaml" "$@"; }
else
  hs() { "$DOCKER" run --rm -i $uopts -v "$work:$work" -v "$(dirname "$policy"):$(dirname "$policy"):ro" "$image" -c "$work/config.yaml" "$@"; }
fi

cat >"$work/config.yaml" <<YAML
server_url: http://127.0.0.1:18080
listen_addr: 127.0.0.1:18080
metrics_listen_addr: 127.0.0.1:19090
grpc_listen_addr: 127.0.0.1:50443
unix_socket: $work/hs.sock
noise: {private_key_path: $work/noise.key}
prefixes: {v4: 100.64.0.0/10, v6: fd7a:115c:a1e0::/48}
derp:
  server:
    enabled: true
    region_id: 999
    region_code: check
    region_name: check
    stun_listen_addr: 127.0.0.1:13478
    private_key_path: $work/derp.key
    ipv4: 127.0.0.1
    automatically_add_embedded_derp_region: true
  urls: []
  paths: []
database: {type: sqlite, sqlite: {path: $work/db.sqlite}}
dns: {magic_dns: true, base_domain: check.invalid, nameservers: {global: [1.1.1.1]}}
policy: {mode: file, path: ""}
YAML

# 1. let a real server create and migrate the database, then stop it
if [ -n "${HEADSCALE_BIN:-}" ]; then
  "$HEADSCALE_BIN" -c "$work/config.yaml" serve >"$work/serve.log" 2>&1 &
  pid=$!
else
  cid=$("$DOCKER" run -d --rm $uopts -v "$work:$work" "$image" -c "$work/config.yaml" serve)
fi
for _ in $(seq 1 100); do [ -S "$work/hs.sock" ] && break; sleep 0.2; done
if [ ! -S "$work/hs.sock" ]; then
  [ -f "$work/serve.log" ] && cat "$work/serve.log" >&2
  [ -n "${cid:-}" ] && "$DOCKER" logs "$cid" >&2 || true
  echo "headscale did not start" >&2; exit 2
fi
if [ -n "${HEADSCALE_BIN:-}" ]; then
  kill "$pid" || true; wait "$pid" 2>/dev/null || true; pid=0
else
  "$DOCKER" stop -t 2 "$cid" >/dev/null || true; cid=
fi

# 2. seed one node per tag and per operator
python3 "$here/seed.py" "$policy" "$work/db.sqlite"

# 3. check, evaluating the tests against the seeded nodes
hs policy check --force --bypass-grpc-and-access-database-directly -f "$policy"
echo "policy ok: $policy"
