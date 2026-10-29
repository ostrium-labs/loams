#!/usr/bin/env bash
# The exposure test (BY0 Task 9, D1380): from outside the tailnet, a TCP connect
# scan of one address. Fails if any port answers. A Loams BYOC install has no
# inbound rule, so a correct install shows none open.
#
#   tests/byoc/exposure.sh <ip-or-name> [--timeout SECONDS] [--extra "p1 p2 ..."]
#
# Scans ports 1-1024, 1025-1100, the common higher service ports, the engine's
# own (8080, 8443, 6333, 9200, 5432, 4000, 7781, 3000, 7680, 9000, 8090) and
# --extra. A closed or filtered port is a pass; a refused connection from the
# host itself counts as closed (the packet got an answer, but nothing is open).
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
host="${1:-}"
[[ -n "$host" && "$host" != -* ]] || { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
shift
timeout=2; extra=""
while (($#)); do
  case "$1" in
    --timeout) timeout="${2:?}"; shift 2 ;;
    --extra) extra="${2-}"; shift 2 ;;
    *) echo "exposure.sh: unknown option $1" >&2; exit 2 ;;
  esac
done
exec python3 -I "$here/exposure_scan.py" "$host" --timeout "$timeout" --extra "$extra"
