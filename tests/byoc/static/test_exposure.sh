#!/usr/bin/env bash
# The exposure scan finds an open port and passes a closed host.
# shellcheck disable=SC2015,SC2154 # A && B || C reports a failed check
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scan="$here/../exposure.sh"
tmp="$(mktemp -d "${TMPDIR:-/var/tmp}/byoc-exp.XXXXXX")"
pids=()
trap 'for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; rm -rf "$tmp"' EXIT
fail=0
ok() { echo "ok   $*"; }
bad() { echo "FAIL $*"; fail=1; }

# a listener on a free port above 1100, which only --extra scans
python3 -I -c '
import socket, sys, time
s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(8)
open(sys.argv[1], "w").write(str(s.getsockname()[1]))
time.sleep(60)' "$tmp/port" &
pids+=("$!")
for _ in $(seq 50); do [[ -s "$tmp/port" ]] && break; sleep 0.1; done
port="$(cat "$tmp/port")"

if "$scan" 127.0.0.1 --timeout 1 --extra "$port" >"$tmp/out" 2>&1; then bad "an open port was not found: $(cat "$tmp/out")"
else grep -q "open ports: .*$port" "$tmp/out" && ok "an open port is reported and fails the run" || bad "message: $(cat "$tmp/out")"; fi

# the same host, the listener's port not scanned: whatever else is open on this machine is not ours to judge,
# so scan an address that has nothing: the unroutable TEST-NET-1 with a short timeout, a few ports only
if python3 -I "$here/../exposure_scan.py" 192.0.2.1 --timeout 0.3 --extra "" --concurrency 1000 >"$tmp/out2" 2>&1; then
  grep -q "ok: no open port" "$tmp/out2" && ok "a host with nothing listening passes" || bad "pass message: $(cat "$tmp/out2")"
else bad "a quiet host failed: $(cat "$tmp/out2")"; fi

"$scan" 2>/dev/null && bad "no host should be a usage error" || ok "no host is a usage error"
exit "$fail"
