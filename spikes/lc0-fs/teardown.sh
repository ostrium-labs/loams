#!/usr/bin/env bash
# Teardown and the test teardown_leaves_nothing.
#   Unmount everything on A and B, revoke any remaining scoped tokens,
#   delete the file system, stop machine B, remove local state for this
#   file system (credentials, companion homes, caches, mount logs, locators,
#   token files), then check that nothing is left:
#     - `ti fs list-file-systems` lists no file system in the region;
#     - no drive9 mount on A or B; no locator under any ~/.ti/mounts/;
#     - no ~/.ti/fs_credentials entry for the file system.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
FS=$(fs_id)
pass=0 fail=0
ok() { echo "PASS $*"; pass=$((pass+1)); }
ko() { echo "FAIL $*"; fail=$((fail+1)); }

echo "== unmount A"
for loc in "$HOME_A"/.ti/mounts/*.locator.json; do
  [ -e "$loc" ] || continue
  mp=$(json 'd.get("mount_path") or d.get("mountpoint") or ""' < "$loc")
  [ -n "$mp" ] && tiA fs unmount-file-system --mount-path "$mp" --timeout 60s >/dev/null 2>&1 \
    || tiA fs unmount-file-system --mount-path "$mp" --force >/dev/null 2>&1
  echo "unmounted $mp"
done
for mp in $(mount | awk '$1=="drive9"{print $3}' | grep "^$LC0_ROOT/" || true); do fusermount3 -u "$mp" 2>/dev/null || fusermount3 -uz "$mp"; done
echo "== unmount B"
if podman container exists "$B_CTR"; then
  tiB_sh 'for loc in ~/.ti/mounts/*.locator.json; do [ -e "$loc" ] || continue
    mp=$(python3 -c "import json,sys;d=json.load(open(sys.argv[1]));print(d.get(\"mount_path\") or d.get(\"mountpoint\") or \"\")" "$loc" 2>/dev/null || true)
    [ -n "$mp" ] && { ti fs unmount-file-system --mount-path "$mp" --timeout 60s >/dev/null 2>&1 || ti fs unmount-file-system --mount-path "$mp" --force >/dev/null 2>&1; }
    rm -f "$loc"; done
    for mp in $(mount | awk "\$1==\"drive9\"{print \$3}"); do fusermount3 -uz "$mp"; done
    echo "B mounts left: $(mount | grep -c drive9)"' 2>&1 | scrub
fi
echo "== revoke remaining non-owner tokens"
tiAPI fs list-file-system-tokens --file-system-id "$FS" --include-expired 2>/dev/null |
  json 'chr(10).join(t["token_id"] for t in d.get("tokens",[]) if t.get("scope_kind")=="fs_scoped")' |
  while read -r id; do [ -n "$id" ] && tiAPI fs delete-file-system-token --file-system-id "$FS" --token-id "$id" >/dev/null 2>&1 && echo "revoked $id"; done
echo "== delete the file system"
tiAPI fs delete-file-system --file-system-id "$FS" 2>&1 | scrub | json 'd.get("status")' || true
# Deletion is asynchronous: wait until the inventory is empty.
for _ in $(seq 1 60); do
  n=$(tiAPI fs list-file-systems | json 'len(d["file_systems"])'); [ "$n" = 0 ] && break; sleep 5
done
echo "== stop machine B and remove local state"
b_down
podman image rm "$LC0_IMAGE" >/dev/null 2>&1 || true
for h in "$HOME_A" "$HOME_B_HOST" "$LC0_ROOT/apihome"; do
  rm -rf "$h/.ti/fs_credentials" "$h/.ti/drive9-home" "$h/.ti/mounts" "$h/.cache/drive9"
done
# ti on A wrote mount logs under the passwd home's ~/.cache/drive9.
rm -rf "$REAL_HOME/.cache/drive9"
rm -f "$LC0_SECRETS"/*.tok "$LC0_SECRETS"/*.json "$LC0_SECRETS"/scoped.* "$LC0_SECRETS"/dbg.txt

echo "== teardown_leaves_nothing"
inv=$(tiAPI fs list-file-systems)
echo "list-file-systems: $(echo "$inv" | json 'd')"
[ "$(echo "$inv" | json 'len(d["file_systems"])')" = 0 ] && ok "no file system left in $LC0_REGION" || ko "file systems remain"
m=$(mount | grep -c 'fuse.drive9' || true)
[ "$m" = 0 ] && ok "no drive9 mount on A" || ko "$m drive9 mounts remain on A"
podman container exists "$B_CTR" && ko "machine B container remains" || ok "machine B container removed"
locs=$(find "$REAL_HOME/.ti/mounts" "$HOME_A/.ti/mounts" "$HOME_B_HOST/.ti/mounts" "$LC0_ROOT/apihome/.ti/mounts" -name '*.locator.json' 2>/dev/null | wc -l)
[ "$locs" = 0 ] && ok "no mount locator under any ~/.ti/mounts/" || ko "$locs locators remain"
creds=$(find "$REAL_HOME/.ti" "$LC0_ROOT" -path '*fs_credentials*' -type f 2>/dev/null | wc -l)
[ "$creds" = 0 ] && ok "no fs_credentials entry left" || ko "$creds fs credential files remain"
toks=$(ls "$LC0_SECRETS"/*.tok 2>/dev/null | wc -l)
[ "$toks" = 0 ] && ok "no token file left" || ko "$toks token files remain"
echo "result: pass=$pass fail=$fail"
