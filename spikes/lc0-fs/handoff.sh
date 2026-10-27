#!/usr/bin/env bash
# Run 3: handoff from machine A to machine B through a layer.
#   handoff_after_drain_is_complete: layer -> edits on A -> drain ->
#     checkpoint -> fork on B from the checkpoint -> mount -> trees match.
#   undrained_handoff_is_detected: more edits on A, then kill A's mount
#     (no drain) -> B forks the last synced checkpoint -> B lacks the edits
#     -> A, on waking, still holds them (pending cache, then replay into its
#     own layer), and the gap is visible as layer progress past the
#     checkpoint B forked from.
# Needs: lib.sh env (LC0_SECRETS), machine B up (b_up), /runs/base seeded.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
. "$here/lib.sh"
FS=$(fs_id)
OWNER=$LC0_SECRETS/owner.tok
RUN=${RUN:-lc0-run2}
RA=$HOME_A/$RUN
CP=cp-$RUN-$(date +%s)  # checkpoint IDs are unique per file system, not per layer
pass=0 fail=0
ok() { echo "PASS $*"; pass=$((pass+1)); }
ko() { echo "FAIL $*"; fail=$((fail+1)); }

# A tree manifest. Listing a layer mount while its view changes can fail
# with EAGAIN ("mount view changed"), so retry until a clean pass and count
# the retries (a finding in itself).
MANI='set -o pipefail; export LC_ALL=C; find . \( -name .git -o -name node_modules \) -prune -o -type f -print0 | sort -z | xargs -0 sha256sum'
manifest_a() { local n; for n in $(seq 0 9); do
  if out=$(cd "$1" && bash -c "$MANI" 2>/dev/null) && (cd "$1" && find . >/dev/null 2>&1); then
    echo "$out"; [ "$n" -gt 0 ] && echo "manifest_a retries=$n" >&2; return 0; fi; sleep 2; done
  echo "manifest_a gave up" >&2; return 1; }
manifest_b() { tiB_sh "for n in \$(seq 0 9); do if out=\$(cd $1 && $MANI 2>/dev/null) && (cd $1 && find . >/dev/null 2>&1); then echo \"\$out\"; [ \$n -gt 0 ] && echo manifest_b retries=\$n >&2; exit 0; fi; sleep 2; done; echo manifest_b gave up >&2; exit 1"; }
# Preview backend calls fail transiently ("backend unavailable"); retry
# and count, so a flake is reported rather than read as a result.
retry() { local n; for n in 1 2 3 4 5; do if "$@"; then return 0; fi
  echo "retry $n: $1 ${2:-} ${3:-}" >&2; sleep 3; done; return 1; }
need() { [ -n "$2" ] || { echo "ABORT: $1 is empty"; exit 1; }; }
seq_of() { tiA fs describe-layer --file-system-id "$FS" --layer-id "$1" | json 'd.get("durable_seq")'; }

echo "== handoff_after_drain_is_complete"
L=$(tiA fs create-layer --file-system-id "$FS" --base-root-path /runs/base \
  --layer-name "$RUN" --actor-id machine-a --tag run=$RUN | json 'd["layer_id"]')
need layer "$L"; echo "layer $L"
mkdir -p "$RA"
tiA fs mount-file-system --file-system-id "$FS" --mount-path "$RA" --remote-path /runs/base \
  --driver fuse --layer-ref "$L" --mount-profile coding-agent | json 'd["status"]'
# A short run: 10 edits, 2 new files, 1 delete.
files=(shared/config.ts shared/rest.ts shared/value.ts examples/quickstart/index.ts
  examples/sql/index.ts examples/live-tail/index.ts)
for i in $(seq 1 10); do
  echo "// $RUN edit $i" >> "$RA/${files[$(( i % 6 ))]}"; sleep 1
done
mkdir -p "$RA/notes"; echo "run note 1" > "$RA/notes/n1.md"; echo "run note 2" > "$RA/notes/n2.md"
rm "$RA/examples/sql/README.md"
t0=$(now)
tiA fs drain-file-system --mount-path "$RA" --timeout 120s | json 'd["status"]'
t1=$(now); echo "drain_secs=$(elapsed "$t0" "$t1")"
retry tiA fs create-layer-checkpoint --file-system-id "$FS" --layer-id "$L" --checkpoint-id "$CP" --label "handoff" >/dev/null || { echo "ABORT: checkpoint"; exit 1; }
t2=$(now); echo "checkpoint_secs=$(elapsed "$t1" "$t2") seq_at_cp1=$(seq_of "$L")"
manifest_a "$RA" > "$LC0_ROOT/$RUN.manifest.a"
# Machine B (owner token, as in BYOG where the device runs the user's own ti session).
LB=$(tiB_sh "ti fs fork-layer --parent-layer-ref $L --checkpoint-id "$CP" --layer-name $RUN-b --actor-id machine-b" "$OWNER" | json 'd["layer_id"]')
need fork "$LB"
t3=$(now); echo "fork_secs=$(elapsed "$t2" "$t3") fork=$LB"
tiB_sh "mkdir -p ~/$RUN && ti fs mount-file-system --mount-path ~/$RUN --remote-path /runs/base --driver fuse --layer-ref $LB --mount-profile coding-agent >/dev/null" "$OWNER"
manifest_b "~/$RUN" > "$LC0_ROOT/$RUN.manifest.b"
t4=$(now); echo "b_mounted_and_listed_secs=$(elapsed "$t3" "$t4") handoff_total_secs=$(elapsed "$t0" "$t4")"
if diff "$LC0_ROOT/$RUN.manifest.a" "$LC0_ROOT/$RUN.manifest.b" >/dev/null; then
  ok "handoff_after_drain_is_complete ($(wc -l < "$LC0_ROOT/$RUN.manifest.a") files match)"
else
  ko "handoff_after_drain_is_complete"; diff "$LC0_ROOT/$RUN.manifest.a" "$LC0_ROOT/$RUN.manifest.b" | head
fi
# Forks are independent: a write on B must not reach A's layer.
tiB_sh "echo from-b > ~/$RUN/notes/b-only.md && ti fs drain-file-system --mount-path ~/$RUN >/dev/null" "$OWNER"
sleep 12
if [ -e "$RA/notes/b-only.md" ]; then ko "fork_is_independent"; else ok "fork_is_independent"; fi

echo "== undrained_handoff_is_detected"
seq_cp1=$(seq_of "$L")
entries_cp1=$(tiA fs diff-layer --file-system-id "$FS" --layer-id "$L" | json 'len(d.get("entries") or [])')
# Expected state = the drained tree plus the undrained edits, built in a
# local mirror so it does not depend on reading the dying mount.
MIR=$LC0_ROOT/$RUN.mirror; rm -rf "$MIR"; mkdir -p "$MIR"
(cd "$RA" && tar -c --exclude=.git --exclude=node_modules .) | tar -x -C "$MIR"
for d in "$RA" "$MIR"; do
  for i in 1 2 3; do echo "undrained $i" > "$d/notes/u$i.md"; done
  echo "// undrained edit" >> "$d/shared/value.ts"
done
# Kill A's mount processes outright, immediately (no drain, no unmount),
# as a laptop that is killed or loses its network would.
pids=$(pgrep -f -- "$RA( |$)" || true)
echo "killing mount pids: $pids"
[ -n "$pids" ] && kill -9 $pids
fusermount3 -uz "$RA" 2>/dev/null || true
manifest_a "$MIR" > "$LC0_ROOT/$RUN.manifest.a2"
entries_kill=$(tiA fs diff-layer --file-system-id "$FS" --layer-id "$L" | json 'len(d.get("entries") or [])')
echo "layer entries: at cp1=$entries_cp1 after kill=$entries_kill (server-side, before A wakes)"
pending=$(find "$HOME_A/.ti/drive9-home" -path '*pending*' -type f 2>/dev/null | wc -l)
shadow=$(find "$HOME_A/.ti/drive9-home" -path '*shadow*' -type f 2>/dev/null | wc -l)
echo "A local after kill: pending_files=$pending shadow_files=$shadow"
seq_after_kill=$(seq_of "$L")
echo "layer seq: at cp1=$seq_cp1 after kill=$seq_after_kill"
# B resumes from the last synced checkpoint (cp1).
LB2=$(tiB_sh "ti fs fork-layer --parent-layer-ref $L --checkpoint-id "$CP" --layer-name $RUN-b2 --actor-id machine-b" "$OWNER" | json 'd["layer_id"]')
need fork2 "$LB2"
tiB_sh "mkdir -p ~/$RUN-2 && ti fs mount-file-system --mount-path ~/$RUN-2 --remote-path /runs/base --driver fuse --layer-ref $LB2 --mount-profile coding-agent >/dev/null" "$OWNER"
manifest_b "~/$RUN-2" > "$LC0_ROOT/$RUN.manifest.b2"
missing=$(diff "$LC0_ROOT/$RUN.manifest.a2" "$LC0_ROOT/$RUN.manifest.b2" | grep -c '^<' || true)
echo "B (forked from cp1) lacks $missing of A's undrained file states"
# A wakes: remount its layer; drive9 recovers pending writes, then drain.
tiA fs mount-file-system --file-system-id "$FS" --mount-path "$RA" --remote-path /runs/base \
  --driver fuse --layer-ref "$L" --mount-profile coding-agent | json 'd["status"]'
tiA fs drain-file-system --mount-path "$RA" --timeout 120s | json 'd["status"]'
seq_wake=$(seq_of "$L")
entries_wake=$(tiA fs diff-layer --file-system-id "$FS" --layer-id "$L" | json 'len(d.get("entries") or [])')
echo "layer entries after wake: $entries_wake"
manifest_a "$RA" > "$LC0_ROOT/$RUN.manifest.a3"
recovered=$(diff "$LC0_ROOT/$RUN.manifest.a2" "$LC0_ROOT/$RUN.manifest.a3" >/dev/null && echo yes || echo no)
echo "after wake: layer seq=$seq_wake, A's undrained edits recovered into its layer: $recovered"
diff "$LC0_ROOT/$RUN.manifest.a2" "$LC0_ROOT/$RUN.manifest.a3" | head -8
tiA fs diff-layer --file-system-id "$FS" --layer-id "$L" | json 'len(d.get("entries") or [])' | sed 's/^/diff-layer entries on A layer: /'
# Detected = B is missing the work AND the gap is visible (A's layer moved
# past cp1, or A still holds it locally), i.e. not lost silently.
if [ "$missing" -gt 0 ] && { [ "$recovered" = yes ] || [ "$pending" -gt 0 ] || [ "$entries_wake" != "$entries_cp1" ]; }; then
  ok "undrained_handoff_is_detected"
else
  ko "undrained_handoff_is_detected (missing=$missing recovered=$recovered pending=$pending)"
fi
echo "layers: A=$L B=$LB B2=$LB2"
echo "$L $LB $LB2" > "$LC0_ROOT/$RUN.layers"
echo "result: pass=$pass fail=$fail"
