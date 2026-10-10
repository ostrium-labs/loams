#!/usr/bin/env bash
# loams-agentd must stay headless (D782, design §50 §4.3): no GUI, webview or
# audio crate may enter its normal or build dependency tree on any target.
#
#   scripts/ci/agentd-deps.sh            check the workspace's loams-agentd
#   scripts/ci/agentd-deps.sh --filter   check `cargo tree --format '{p}'`
#                                        lines read from stdin (the self-test,
#                                        scripts/ci/agentd-deps.test.sh)
#
# Exit 0: headless. Exit 1: GUI crates found; each is printed with the path
# that pulls it in (`cargo tree -i`). Exit 2: no dependency tree to check.
#
# `--target all` matters: macOS and Windows dependencies are otherwise
# invisible on a Linux runner.
set -euo pipefail

gui='^(gpui[a-z_-]*|zed[a-z_-]*|wry|webkit2gtk[a-z0-9_-]*|javascriptcore[a-z0-9_-]*|soup[0-9]*[a-z_-]*|gtk[a-z0-9_-]*|gdk[a-z0-9_-]*|cpal|alsa[a-z_-]*) '
tree_args=(-p loams-agentd -e normal,build --target all)

# Prints the GUI packages among the `{p}` lines on stdin, one `name vX.Y.Z`
# per line, de-duplicated. Exit 0 if none, 1 if some, 2 on empty input.
offenders() {
  local lines status=0
  lines=$(cat)
  if [[ -z ${lines//[[:space:]]/} ]]; then
    echo "agentd-deps: no dependency tree to check" >&2
    return 2
  fi
  grep -Ei "$gui" <<< "$lines" | sed 's/ (\*)$//; s/ (proc-macro)$//' | sort -u || status=$?
  # grep exits 1 for "no match" (headless) and 2 for an error.
  case $status in
    0) return 1 ;;
    1) return 0 ;;
    *) echo "agentd-deps: grep failed ($status)" >&2; return 2 ;;
  esac
}

report() {
  echo "loams-agentd must stay headless (D782): its dependency tree contains" >&2
  sed 's/^/  /' <<< "$1" >&2
}

if [[ ${1:-} == --filter ]]; then
  status=0
  found=$(offenders) || status=$?
  if ((status == 1)); then report "$found"; fi
  exit "$status"
fi

tree=$(cargo tree "${tree_args[@]}" --prefix none --format '{p}' --locked)
status=0
found=$(offenders <<< "$tree") || status=$?
if ((status != 1)); then
  exit "$status"
fi
report "$found"
while read -r name version _; do
  echo >&2
  echo "path to $name $version:" >&2
  cargo tree "${tree_args[@]}" --locked -i "$name@${version#v}" >&2 || true
done <<< "$found"
exit 1
