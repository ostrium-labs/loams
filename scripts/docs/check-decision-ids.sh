#!/usr/bin/env bash
# Validate declarations in the central decision log; references are not rows.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"

if [[ "${1:-}" == --self-test ]]; then
  "$0" "$root/scripts/docs/testdata/unique-decision-ids.md"
  if output=$("$0" "$root/scripts/docs/testdata/duplicate-decision-ids.md" 2>&1); then
    echo 'decision IDs: planted duplicates were accepted' >&2
    exit 1
  fi
  [[ "$output" == *'duplicate D1'* && "$output" == *'duplicate Q2'* && "$output" == *'duplicate Q-RT-1'* && "$output" == *'duplicate D-SC-1'* ]] || {
    echo "$output" >&2
    exit 1
  }
  echo 'decision IDs: self-test passed (D and Q duplicates rejected)'
  exit 0
fi

log_file="${1:-$root/docs/design/13-decision-log.md}"

if [[ ! -s "$log_file" ]]; then
  echo "Decision log is empty or not found: $log_file" >&2
  exit 1
fi

valid_ids=$(awk -F '|' '
  /^[[:space:]]*(\|[[:space:]]*)?[DQ]([0-9]+|-[A-Za-z0-9]+-[0-9]+)([[:space:]]+\([^|]*\))?[[:space:]]*\|/ {
    id = ($1 ~ /^[[:space:]]*$/) ? $2 : $1
    sub(/[[:space:]]+\(.*/, "", id)
    gsub(/[[:space:]]/, "", id)
    if (id in seen) {
      printf "%s:%d: duplicate %s (first declaration at line %d)\n", FILENAME, FNR, id, seen[id] > "/dev/stderr"
      failed = 1
    } else {
      seen[id] = FNR
      print id
    }
  }
  END { exit failed }
' "$log_file")

failed=0
if [[ "$log_file" == "$root/docs/design/13-decision-log.md" ]]; then
  declare -A declared_ids
  for id in $valid_ids; do
    declared_ids["$id"]=1
  done

  while IFS=: read -r file line match; do
    if [[ "$file" == *"/docs/design/13-decision-log.md" ]] || [[ "$file" == *"/scripts/docs/testdata/"* ]]; then
      continue
    fi
    if [[ -z "${declared_ids[$match]:-}" ]]; then
      rel_file="${file#$root/}"
      echo "$rel_file:$line: dangling citation $match (not found in log)" >&2
      failed=1
    fi
  done < <(grep -rI --exclude-dir={target,.git,node_modules,.venv,__pycache__,dist,gen,assets,gradle} -o -n -H -E '\b[DQ][0-9]+\b' "$root" || true)

  if [[ $failed -ne 0 ]]; then
    exit 1
  fi
fi
