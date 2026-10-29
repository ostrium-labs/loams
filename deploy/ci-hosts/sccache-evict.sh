#!/usr/bin/env bash
# Evicts the least recently used files from a cache directory once it is over
# its cap, down to 90% of the cap. Run by loams-sccache-evict.timer as the ci user.
#   sccache-evict DIR CAP_GIB
set -euo pipefail
dir=$1
cap=$(($2 * 1024 * 1024 * 1024))
target=$((cap * 9 / 10))
used=$(du -sb "$dir" | cut -f1)
((used > cap)) || exit 0
# Oldest access first (relatime still bumps atime once a day); ties by modification.
find "$dir" -type f -printf '%A@ %s %p\n' | sort -n | while read -r _ size path; do
  ((used <= target)) && break
  rm -f -- "$path" && used=$((used - size))
done
find "$dir" -mindepth 1 -type d -empty -delete
echo "sccache-evict: $dir now $(du -sh "$dir" | cut -f1) (cap $2 GiB)"
