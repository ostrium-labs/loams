#!/usr/bin/env bash
# Run 5: the no-card limits (2,000 files, 2 GiB, 500 MiB per file).
#   Phase files: write small files through A's mount until the file-count
#     limit, then 10 more; record where writes stop, how the app sees it,
#     and that reads still work. Then delete the fill (one recursive delete).
#   Phase bytes: upload files of FILE_MB MiB until the storage limit; one
#     file over 500 MiB; same checks. Then delete.
# Usage reading comes from the drive9 admin tenant API (fs_quota in lib.sh).
# Budget: about (2000 - current files) + 20 writes for phase files.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
FS=$(fs_id)
usage() { fs_quota | json 'd["quota"]["usage"]'; }
phase=${1:-files}

if [ "$phase" = files ]; then
  start=$(fs_quota | json 'd["quota"]["usage"]["file_count"]')
  limit=$(fs_quota | json 'd["quota"]["config"]["max_file_count"]')
  n=$(( limit - start + 10 ))
  echo "file_count=$start limit=$limit; writing $n files"
  mkdir -p "$MNT_A/fill"
  t0=$(now); errs=0; first_err=
  for i in $(seq 1 "$n"); do
    if ! echo "f$i" > "$MNT_A/fill/f$i.txt" 2>"$LC0_ROOT/fill.err"; then
      errs=$((errs+1)); [ -z "$first_err" ] && first_err="$i: $(cat "$LC0_ROOT/fill.err")"
    fi
  done
  t1=$(now); echo "wrote in $(elapsed "$t0" "$t1")s; local write errors=$errs first=${first_err:-none}"
  tiA fs drain-file-system --mount-path "$MNT_A" --timeout 600s 2>&1 | scrub | tail -2
  t2=$(now); echo "drain $(elapsed "$t1" "$t2")s"
  echo "usage: $(usage)"
  remote=$(tiA fs list-files --file-system-id "$FS" --path /fill | json 'len(d["entries"])')
  echo "remote /fill entries: $remote of $n written locally"
  echo "-- writes after the limit (API):"
  echo over | tiA fs copy-file --file-system-id "$FS" --from-stdin --to-remote /fill/over-api.txt 2>&1 | scrub | tail -1
  echo "-- overwrite of an existing file at the limit (API):"
  echo changed | tiA fs copy-file --file-system-id "$FS" --from-stdin --to-remote /fill/f1.txt --overwrite 2>&1 | scrub | tail -1
  echo "-- a new file through the mount (close status, then after drain):"
  echo over > "$MNT_A/fill/over-mount.txt"; echo "close rc=$?"
  tiA fs drain-file-system --mount-path "$MNT_A" --timeout 60s 2>&1 | scrub | tail -1
  ls -la "$MNT_A/fill/over-mount.txt" 2>&1 | tail -1
  echo "-- reads at the limit:"
  tiA fs read-file --file-system-id "$FS" --path /fill/f1.txt 2>&1 | head -1
  cat "$MNT_A/fill/f2.txt"
  tiA fs read-file --file-system-id "$FS" --path /t/a/seed.txt 2>&1 | head -1
  grep -h -iE 'quota|limit|exceed|413|507|403' "$REAL_HOME"/.cache/drive9/mount-logs/mount-59561af33e3b7986.log | tail -5 | cut -c1-220
fi

if [ "$phase" = cleanup-files ]; then
  t0=$(now)
  tiA fs delete-file --file-system-id "$FS" --path /fill --recursive 2>&1 | scrub | tail -3
  echo "recursive delete $(elapsed "$t0" "$(now)")s; usage: $(usage)"
fi

if [ "$phase" = bytes ]; then
  mb=${FILE_MB:-500}
  mkdir -p "$LC0_ROOT/blob"
  limit=$(fs_quota | json 'd["quota"]["config"]["max_storage_size"]')
  echo "usage: $(usage); storage limit ${limit} MiB; file size ${mb} MiB"
  f=$LC0_ROOT/blob/blob.bin; head -c $((mb*1024*1024)) /dev/urandom > "$f"
  i=0
  while :; do
    i=$((i+1))
    t0=$(now)
    out=$(tiA fs copy-file --file-system-id "$FS" --from-local "$f" --to-remote "/big/b$i.bin" 2>&1); rc=$?
    echo "upload b$i rc=$rc in $(elapsed "$t0" "$(now)")s: $(echo "$out" | scrub | grep -E 'status|ERROR|error' | head -2 | tr '\n' ' ' | cut -c1-200)"
    echo "  usage: $(usage)"
    [ $rc -ne 0 ] && break
    [ $i -ge 6 ] && break
  done
  echo "-- small write at the storage limit:"
  echo tiny | tiA fs copy-file --file-system-id "$FS" --from-stdin --to-remote /big/tiny.txt 2>&1 | scrub | tail -1
  echo "-- read at the storage limit:"
  tiA fs read-file --file-system-id "$FS" --path /big/b1.bin --length 16 | od -An -tx1 | head -1
  tiA fs read-file --file-system-id "$FS" --path /t/a/seed.txt 2>&1 | head -1
  echo "-- one file over 500 MiB (501 MiB), after freeing space:"
  tiA fs delete-file --file-system-id "$FS" --path /big --recursive >/dev/null 2>&1
  echo "  usage after delete: $(usage)"
  head -c $((501*1024*1024)) /dev/urandom > "$f"
  tiA fs copy-file --file-system-id "$FS" --from-local "$f" --to-remote /big/over500.bin 2>&1 | scrub | grep -E 'status|ERROR|error' | head -2
  echo "  usage: $(usage)"
  tiA fs delete-file --file-system-id "$FS" --path /big --recursive >/dev/null 2>&1
  rm -rf "$LC0_ROOT/blob"
  echo "  usage after cleanup: $(usage)"
fi
