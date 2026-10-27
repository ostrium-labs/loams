#!/usr/bin/env bash
# Run 4: scoped tokens. Test: scoped_token_cannot_escape_prefix.
#   Token: /t/a with read,list,write, TTL 1h, minted from the owner token on
#   machine A (the control plane's role). Used on machine B only.
#   Must work on /t/a; must fail on /t/b, /t/ab (prefix confusion), traversal,
#   delete (not granted), mounting /, fork, Git, token management; and must
#   fail after disable-file-system-token, with the propagation timed.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
FS=$(fs_id)
OWNER=$LC0_SECRETS/owner.tok
SC=$LC0_SECRETS/scoped.tok
pass=0 fail=0
ok() { echo "PASS $*"; pass=$((pass+1)); }
ko() { echo "FAIL $*"; fail=$((fail+1)); }
# expect allow|deny <label> <snippet run on B with the scoped token>
expect() {
  local want=$1 label=$2 snippet=$3 out rc
  out=$(tiB_sh "$snippet" "$SC" 2>&1); rc=$?
  local msg; msg=$(echo "$out" | grep -m1 -E 'ERROR|error|denied|forbidden|scope' | scrub | cut -c1-140)
  if [ "$want" = allow ] && [ $rc -eq 0 ]; then ok "$label (allowed)"
  elif [ "$want" = deny ] && [ $rc -ne 0 ]; then ok "$label (denied: ${msg:-rc=$rc})"
  else ko "$label (want $want, rc=$rc: $(echo "$out" | scrub | tail -2 | tr '\n' ' ' | cut -c1-200))"; fi
}

echo "== setup (owner token on A)"
echo seed-a | tiA fs copy-file --file-system-id "$FS" --from-stdin --to-remote /t/a/seed.txt >/dev/null
echo secret-b | tiA fs copy-file --file-system-id "$FS" --from-stdin --to-remote /t/b/secret.txt >/dev/null
echo secret-ab | tiA fs copy-file --file-system-id "$FS" --from-stdin --to-remote /t/ab/secret.txt >/dev/null
umask 077
TI_FS_TOKEN=$(cat "$OWNER") HOME=$HOME_A TI_REGION_CODE=$LC0_REGION "$TI_BIN" fs \
  generate-file-system-scoped-token --ttl 1h --allow /t/a:read,list,write --subject lc0-scoped-b \
  > "$LC0_SECRETS/scoped.json"
python3 -I -c "import json,sys; d=json.load(open(sys.argv[1])); open(sys.argv[2],'w').write(d.get('fs_token') or d.get('token'))" \
  "$LC0_SECRETS/scoped.json" "$SC"
TOKID=$(python3 -I -c "import json,sys; d=json.load(open(sys.argv[1])); print(d.get('token_id') or d.get('id'))" "$LC0_SECRETS/scoped.json")
echo "$TOKID" > "$LC0_SECRETS/scoped.id"
echo "scoped token id=$TOKID; metadata:"; scrub < "$LC0_SECRETS/scoped.json" | grep -vE '"(fs_token|token)"'

echo "== scoped_token_cannot_escape_prefix (on B)"
expect allow "read /t/a/seed.txt"      'ti fs read-file --path /t/a/seed.txt | grep -q seed-a'
expect allow "list /t/a"               'ti fs list-files --path /t/a >/dev/null'
expect allow "write /t/a/new.txt"      'echo by-b | ti fs copy-file --from-stdin --to-remote /t/a/new.txt >/dev/null'
expect deny  "read /t/b/secret.txt"    'ti fs read-file --path /t/b/secret.txt'
expect deny  "list /t/b"               'ti fs list-files --path /t/b'
expect deny  "write /t/b/x.txt"        'echo x | ti fs copy-file --from-stdin --to-remote /t/b/x.txt'
expect deny  "read /t/ab/secret.txt (prefix confusion)"  'ti fs read-file --path /t/ab/secret.txt'
expect deny  "write /t/ab/x.txt (prefix confusion)"      'echo x | ti fs copy-file --from-stdin --to-remote /t/ab/x.txt'
expect deny  "read /t/a/../b/secret.txt (traversal)"     'ti fs read-file --path /t/a/../b/secret.txt'
expect deny  "list /"                  'ti fs list-files --path /'
expect deny  "list /t"                 'ti fs list-files --path /t'
expect deny  "delete /t/a/seed.txt (delete not granted)" 'ti fs delete-file --path /t/a/seed.txt'
expect deny  "move /t/a/seed.txt -> /t/b/ (escape by move)" 'ti fs move-file --from-remote /t/a/seed.txt --to-remote /t/b/moved.txt'
expect deny  "copy /t/b/secret.txt -> /t/a/ (escape by copy)" 'ti fs copy-file --from-remote /t/b/secret.txt --to-remote /t/a/stolen.txt'
expect deny  "mount /"                 'mkdir -p ~/sc-root && ti fs mount-file-system --mount-path ~/sc-root --driver fuse --ready-timeout 20s'
expect deny  "list tokens"             'ti fs list-file-system-tokens'
expect deny  "mint a scoped token from a scoped token" 'ti fs generate-file-system-scoped-token --ttl 1h --allow /:read,list'
PARENT=$(cut -d' ' -f1 "$LC0_ROOT/lc0-run8.layers")
expect deny  "fork a layer (owner's layer $PARENT)" "ti fs fork-layer --parent-layer-ref $PARENT --layer-name sc-fork"
expect deny  "hydrate the Git workspace" 'ti fs-git hydrate-git-workspace --target-path ~/mnt/ws/loams-typescript --timeout 30s' 
expect deny  "create a layer on /t/b"  'ti fs create-layer --base-root-path /t/b --layer-name sc-layer'
# Positive control: a mount of its own prefix works with the scoped token.
expect allow "mount /t/a (own prefix)" 'mkdir -p ~/sc-a && ti fs mount-file-system --mount-path ~/sc-a --remote-path /t/a --driver fuse >/dev/null && grep -q seed-a ~/sc-a/seed.txt'
ti_owner_check=$(tiA fs list-files --file-system-id "$FS" --path /t/b | json '[e["name"] for e in d["entries"]]')
echo "owner view of /t/b after the attempts: $ti_owner_check"
case "$ti_owner_check" in *x.txt*|*moved*) ko "/t/b unchanged";; *) ok "/t/b unchanged";; esac
case "$(tiA fs list-files --file-system-id "$FS" --path /t/a | json '[e["name"] for e in d["entries"]]')" in
  *stolen*) ko "no copy of /t/b into /t/a";; *) ok "no copy of /t/b into /t/a";; esac

echo "== disable-file-system-token and propagation"
# The live /t/a mount on B: drain it, keep it mounted, to see what a live
# mount does after its token is disabled (the cohort kill switch case).
tiB_sh 'ti fs drain-file-system --mount-path ~/sc-a >/dev/null' "$SC"
tiB_sh 't0=$(date +%s.%N); n=0; until ! ti fs read-file --path /t/a/seed.txt >/dev/null 2>&1; do n=$((n+1)); sleep 0.5; done; echo api_denied_at $(date +%s.%N) polls=$n' "$SC" > "$LC0_ROOT/disable.poll" &
poll=$!
sleep 3
td0=$(now)
tiAPI fs disable-file-system-token --file-system-id "$FS" --token-id "$TOKID" | scrub | json 'd.get("status") or d.get("state") or d'
td1=$(now)
timeout 300 tail --pid=$poll -f /dev/null
denied=$(awk '{print $2}' "$LC0_ROOT/disable.poll")
echo "disable call took $(elapsed "$td0" "$td1")s; API denial seen $(elapsed "$td1" "$denied")s after it returned ($(elapsed "$td0" "$denied")s after it started)"
expect deny "read /t/a after disable" 'ti fs read-file --path /t/a/seed.txt'
expect deny "write /t/a after disable" 'echo y | ti fs copy-file --from-stdin --to-remote /t/a/after.txt'
# The live mount: cached read, new write.
out=$(tiB_sh 'cat ~/sc-a/seed.txt 2>&1; echo after > ~/sc-a/after-mount.txt 2>&1; echo rc=$?; sleep 5; ti fs drain-file-system --mount-path ~/sc-a 2>&1 | tail -1' "$SC")
echo "live mount after disable: $(echo "$out" | scrub | tr '\n' ' ' | cut -c1-300)"
case "$(tiA fs list-files --file-system-id "$FS" --path /t/a | json '[e["name"] for e in d["entries"]]')" in
  *after-mount*) ko "a live mount cannot write after disable";; *) ok "a live mount cannot write after disable";; esac
tiB_sh 'ti fs unmount-file-system --mount-path ~/sc-a --force >/dev/null 2>&1; ti fs unmount-file-system --mount-path ~/sc-root --ignore-absent >/dev/null 2>&1; true'
echo "== re-enable propagation"
tiB_sh 'until ti fs read-file --path /t/a/seed.txt >/dev/null 2>&1; do sleep 0.5; done; echo api_allowed_at $(date +%s.%N)' "$SC" > "$LC0_ROOT/enable.poll" &
poll=$!
sleep 2
te0=$(now)
tiAPI fs enable-file-system-token --file-system-id "$FS" --token-id "$TOKID" >/dev/null
te1=$(now)
timeout 300 tail --pid=$poll -f /dev/null
echo "enable: allowed again $(elapsed "$te1" "$(awk '{print $2}' "$LC0_ROOT/enable.poll")")s after the call returned"
echo "== revoke (delete) the scoped token"
tiAPI fs delete-file-system-token --file-system-id "$FS" --token-id "$TOKID" >/dev/null && echo "deleted $TOKID"
expect deny "read /t/a after delete" 'ti fs read-file --path /t/a/seed.txt'
rm -f "$SC" "$LC0_SECRETS/scoped.json"
echo "result: pass=$pass fail=$fail"
