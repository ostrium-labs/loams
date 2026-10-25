#!/usr/bin/env bash
# Run 2: a scripted "agent session" in a mounted Git workspace.
#   30 edits (27 changes to existing files, 3 new files), a test run
#   (npm ci + tsc), and a commit. Edits are spaced like an agent's tool
#   calls (EDIT_GAP seconds, default 3, longer than drive9's 2 s flush
#   debounce so edits are not coalesced artificially).
# Usage: session.sh <repo-dir-in-mount> <label>
set -euo pipefail
repo=$1 label=${2:-s1}
gap=${EDIT_GAP:-3}
cd "$repo"
existing=(README.md shared/config.ts shared/rest.ts shared/value.ts
  examples/quickstart/index.ts examples/sql/index.ts examples/live-tail/index.ts
  examples/quickstart/README.md examples/sql/README.md)
for i in $(seq 1 30); do
  case $i in
    10|20|30) f=notes/$label-$i.md; mkdir -p notes
              printf '# %s note %d\n\nWritten by the scripted agent session.\n' "$label" "$i" > "$f" ;;
    *) f=${existing[$(( i % ${#existing[@]} ))]}
       # Read-modify-write the whole file, as an agent's edit tool does.
       python3 -I - "$f" "$label" "$i" <<'PY'
import sys
p, label, i = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(p).read()
mark = "// " if p.endswith(".ts") else "<!-- "
end = "" if p.endswith(".ts") else " -->"
s += f"{mark}lc0 {label} edit {i}{end}\n"
open(p, "w").write(s)
PY
       ;;
  esac
  sleep "$gap"
done
echo "edits done"
# The test run: dependencies and build output stay local (coding-agent).
npm ci --ignore-scripts --no-audit --no-fund --loglevel=error >/dev/null
npx tsc -p tsconfig.json && echo "tsc ok"
git add -A
git -c user.name=lc0 -c user.email=lc0@ostriumlabs.org commit -q -m "lc0 $label: scripted agent session"
git log --oneline -1
