# LC0 Task 2: Filesystem workspaces, handoff and token scoping (spike)

Spike code for [LC1 plan](../../docs/plans/2026-10-23-lc1-loams-cloud.md) Task 2 and [design 78](../../docs/design/78-loams-cloud-on-tidb-x.md) §9.4, §14.5 and §15.2. Not product code: it is never wired into a crate, the CLI or the release, and LC1's first task keeps it as a fixture or deletes it.

Run on 2026-10-23 against the owner's no-card TiDB Cloud organization, region `aws-us-east-1`, with `ti` 0.2.10 (e6fbd15, 2026-10-15) and its companion `ti-drive9` 572cac6 (2026-10-20).

## The two machines

There is no macOS machine, so both machines are Linux runtimes on one CachyOS host (kernel 7.2.4, `fusermount3`, `/dev/fuse`):

- **Machine A:** the host itself, with its own `HOME` (`~/lc0-fs/a`). A separate `HOME` keeps A's `~/.ti` state apart from the real `~/.ti`, which other agents use.
- **Machine B:** a rootless podman container (`Containerfile.machine-b`: Node 22, `fuse3`, `git`) started with `--device /dev/fuse --cap-add SYS_ADMIN --init`. Its `HOME` is `/home/b`, bind-mounted from `~/lc0-fs/b`. It has its own mount namespace, process tree and `~/.ti`, and it gets only `TI_FS_TOKEN` and `TI_REGION_CODE`. That is the setup the `ti` docs recommend for a sandbox.

A container is closer to a second machine than a second `HOME` on the host would be: B's mounts, caches and companion processes are invisible to A. Both machines share the host's clock, so the latency figures need no clock correction. They also share its network path, so a real second machine on another network would add its own round-trip time.

**macOS was not run.** It needs the owner's Mac; see the runbook at the end.

## How to run

Every script sources `lib.sh` and needs `LC0_SECRETS`, a `0700` directory such as the agent scratchpad, for the token files.

```bash
export LC0_SECRETS=/path/to/0700/dir
. spikes/lc0-fs/lib.sh

# Set up: one HOME per machine, with ti's binaries linked in.
for h in "$HOME_A" "$LC0_ROOT/apihome"; do mkdir -p "$h/.ti"; ln -sfn "$REAL_HOME/.ti/bin" "$h/.ti/bin"
  printf 'schema_version = 1\n[telemetry]\nenabled = false\n' > "$h/.ti/.preferences"; done
# Create the file system (API keys) with HOME=$HOME_A, so A stores the owner
# credential; keep a 0600 copy of the token for B and the ID.
umask 077
( set -a; . ~/tidbcloud-lc0.env; set +a; HOME=$HOME_A "$TI_BIN" fs create-file-system \
  --display-name lc0-fs --label purpose=lc0-spike --wait ) > "$LC0_SECRETS/create.json"  # holds fs_token
json 'd["fs_token"]' < "$LC0_SECRETS/create.json" | tr -d '\n' > "$LC0_SECRETS/owner.tok"
json 'd["file_system_id"]' < "$LC0_SECRETS/create.json" | tr -d '\n' > "$LC0_SECRETS/fs_id"
podman build -t lc0-fs-b -f spikes/lc0-fs/Containerfile.machine-b spikes/lc0-fs
b_up
tiB_sh 'mkdir -p ~/.ti && ln -sfn /opt/ti/bin ~/.ti/bin'

# Optional: count requests (see "Counting writes").
spikes/lc0-fs/proxy.sh
export HTTPS_PROXY=http://127.0.0.1:18089 SSL_CERT_FILE=$LC0_ROOT/pki/bundle.pem

# Run 1: mount A with coding-agent, clone a real repository.
tiA fs mount-file-system --file-system-id "$(fs_id)" --mount-path "$MNT_A" --driver fuse --mount-profile coding-agent
tiA fs-git clone-git-workspace --file-system-id "$(fs_id)" \
  --repo-url https://github.com/ostrium-labs/loams-typescript.git \
  --target-path "$MNT_A/ws/loams-typescript" --blobless --hydrate sync
# Run 2: the scripted agent session (30 edits, npm ci + tsc, commit).
spikes/lc0-fs/session.sh "$MNT_A/ws/loams-typescript" s1
tiA fs drain-file-system --mount-path "$MNT_A"
# Run 3: seed a plain tree for layers, then handoff (two tests).
mkdir -p "$MNT_A/runs/base" && git -C "$MNT_A/ws/loams-typescript" archive HEAD~1 | tar -x -C "$MNT_A/runs/base"
tiB_sh 'mkdir -p ~/mnt && ti fs mount-file-system --mount-path ~/mnt --driver fuse --mount-profile coding-agent' "$LC0_SECRETS/owner.tok"
RUN=lc0-run7 spikes/lc0-fs/handoff.sh
spikes/lc0-fs/propagation.sh
# Run 4: scoped tokens.
spikes/lc0-fs/tokens.sh
# Run 5: limits.
spikes/lc0-fs/limits.sh files; spikes/lc0-fs/limits.sh cleanup-files; spikes/lc0-fs/limits.sh bytes
# Run 6: usage visibility.
fs_quota; bill 2026-10; bill 2026-10 billsDetails
# Teardown, and the test teardown_leaves_nothing.
spikes/lc0-fs/teardown.sh
```

Never print a token. Tokens reach processes only through `TI_FS_TOKEN` in the environment, never as flags. `scrub` (in `lib.sh`) redacts JWTs and token-like JSON fields from anything shown.

## Counting writes

**Nothing in the API or the CLI counts requests per file system.** The console shows usage. The Billing API (`GET https://billing.tidbapi.com/v1beta1/bills/{YYYY-MM}` and `.../billsDetails/{YYYY-MM}`, digest auth with the org API key) answers, but only per organization. After about 2,800 counted writes over roughly an hour, it still showed `runningTotal: "0"` and `summaryByService: null`, so it lags by an hour or more and can't split one session from another.

So `countproxy.ts` counts them. It is an HTTPS proxy (`HTTPS_PROXY`) that terminates TLS for `*.drive9.ai` with a throwaway CA (`SSL_CERT_FILE`, which Go honours) and forwards each request upstream. For each request it logs one JSON line with the method, host, path (query dropped), status and byte counts; it never logs headers or bodies. Other hosts, such as the object-store part uploads to S3, are tunnelled and counted only as CONNECTs. `count.sh` sums a time window.

A "write" here is a mutating HTTP request to the Filesystem API: `PUT`, `POST`, `PATCH` or `DELETE`, except `POST fs:batch-stat`, which is a read. The pricing page doesn't define a billable write beyond "a write stores data", so these figures are client-side estimates. LC1 should check them against the console's usage page or a later bill.

Only machine A runs through the proxy, so B's few requests (forks, mounts, a handful of small writes) are not in the totals.

## Results

### Run 1: create, clone and mount

| Step | Time | Writes | Reads | Notes |
|---|---|---|---|---|
| `create-file-system --wait` | a few seconds | n/a (control plane) | | The file system is a drive9 "tenant" (`POST /v1/admin/tenants`) |
| `mount-file-system --mount-profile coding-agent` (A) | 7.9 s | 0 | 6 | |
| `fs-git clone-git-workspace --blobless --hydrate sync` (`ostrium-labs/loams-typescript`, 16 files, 31 KB) | 14.7 s | 13 | 4 | Clean blobs come from `codeload.github.com`, not from the file system |

The Git workspace's files **don't count toward the file system's file count**: `file_count` stayed at 5 after the clone and after the session. The tree and every later edit live behind `/v1/git-workspaces/<id>/overlay`, not as files in the namespace. `ti fs list-files /ws/loams-typescript` returns no entries.

### Run 2: one scripted agent session

`session.sh`:
- 30 edits, 3 s apart: 27 read-modify-write edits to existing files and 3 new files;
- `npm ci` plus `tsc -p` (dependencies and build output stay local under `coding-agent`);
- `git add -A` and a commit;
- then one drain.

| Measure | Value |
|---|---|
| Session wall time | 110 s, plus a 4.8 s drain |
| **Writes** | **77**: 73 `POST …/overlay`, 2 `POST …/object-packs`, 2 `POST …/git-state` |
| Reads | 42 |
| Files created in the file system | 0 (`file_count` 5 → 5; the 3 new files are Git-workspace overlay entries) |
| **Cost per session** | **77 × $0.50 / 1,000 = $0.0385** in writes, plus $0.0017 in reads: about **$0.04** |
| Sessions inside the $5 monthly allowance | about 125, if nothing else writes |

**Drains on a Git workspace aren't free.** Each `drain-file-system` on a mount holding the Git workspace re-uploads the workspace's whole dirty overlay: **14 writes per drain, even with nothing new**, measured on two idle drains in a row, at 4.9–6.4 s each. At D1713's interval of one drain every 5 minutes, that is 168 writes, or $0.084, per active hour on top of the edits themselves. A layer mount without a Git workspace drains in 0.02–0.08 s and adds nothing.

### Run 3: handoff (`handoff.sh`, `propagation.sh`)

#### Layers and Git workspaces don't compose

The plan's step is "create a layer for the run; drain; checkpoint; fork on B; hydrate Git". With the run's layer on the Git workspace path (`create-layer --base-root-path /ws/loams-typescript`, mounted with `--layer-ref`):

- the layer mount is empty until `fs-git hydrate-git-workspace` runs in it;
- after hydration, **edits in the layer mount go to the shared Git-workspace overlay** (`POST /v1/git-workspaces/<id>/overlay`), not into the layer. `diff-layer` lists no entries, and B's plain view of the base workspace showed the "layer-only" file and edit. The layer isolates nothing for Git-workspace content.
- A's own base mount of that workspace still did not show the change after 120 s, so two mounts of one Git workspace on one machine can see different overlays.

So the handoff tests below use a layer over a plain tree (`/runs/base`, the repository's files without `.git`). Git state crosses machines through the Git workspace (`hydrate-git-workspace` on B reproduced the tree and the commit, `44f806c`) or through a pushed WIP ref, but not through a layer.

#### `handoff_after_drain_is_complete`

**PASS** (runs 7 and 8; run 6 also matched once sorted by path). On A:
- a layer over `/runs/base`, mounted with `coding-agent`;
- 10 edits, 2 new files and 1 delete;
- drain, then a checkpoint.

On B: `fork-layer --checkpoint-id`, mount the fork, then compare SHA-256 manifests. All 17 files matched.

| Step | Time |
|---|---|
| `drain-file-system` (layer mount, small edits) | **0.02–0.08 s** (`interactive` sync mode had already uploaded on close) |
| `create-layer-checkpoint` | 0.36–0.43 s |
| `fork-layer` on B | 0.83–0.89 s (the same as any `ti` call: `list-files` takes 0.81–0.85 s) |
| Mount the fork on B | 5.6–8.2 s |
| First read plus a full listing on B (17 files) | 2.0–2.4 s |
| **Handoff critical path (drain → B can read the tree)** | **about 10–12 s** |

`fork_is_independent`: **PASS**. A file B wrote in its fork never appeared in A's layer.

#### `undrained_handoff_is_detected`

**PASS**. A made 3 new files and 1 edit in its layer, and its mount processes were then killed with `kill -9` within milliseconds, with no drain and no unmount.

- **Server side:** the layer's `diff-layer` entry count stayed at 10, the same as at the checkpoint, so none of the edits had reached the service.
- **B:** a fork from the last checkpoint lacked all 4 changes. Nothing on the service side marks them as missing.
- **A, still asleep:** the data survives locally in the mount's shadow cache under `~/.ti/drive9-home/…/.cache/drive9/<hash>/shadow`. The `pending` index was empty.
- **A, on waking** (remounting the same layer): drive9 replayed the writes into **A's own layer**. Entries went from 10 to 13, and A's tree matched the expected manifest. B's fork was untouched, because forks are independent.
- **Detection:** A's layer has moved past the checkpoint B forked from. `diff-layer` shows the new entries. `durable_seq` does not show them: it stayed at 14 through the kill and the wake. This confirms §9.4 step 4: nothing is lost silently, and the sleeping device brings its work back on waking. Loams has to compare the layer against the checkpoint (entries or `--max-seq`) and offer the divergent work as a side branch.

If the kill comes more than about 2 s after the last close, the writes are usually already uploaded; that was the first attempt's "undrained" run. Without the write-back cache's local copy, a lost or wiped disk loses exactly the writes since the last upload.

#### How fast B sees a change (base namespace, no layer)

| Case | B's mount sees it | B's API (`read-file`) sees it |
|---|---|---|
| A writes and closes a file, no drain (3 rounds) | 2.3–3.3 s | 2.66–2.67 s |
| A writes, closes and drains (3 rounds) | 2.3–2.6 s | 2.67–2.72 s |
| Drain call itself (base mount, with the Git workspace) | 6.2–6.4 s | |
| 20 small files plus one 20 MiB file, then drain | 26.5 s | |

A closed file reaches other machines in **about 2.5 s without any drain**, because drive9's `interactive` sync mode uploads after close with a 2 s debounce. Draining doesn't make it faster; it only proves that the upload finished.

**`drain-file-system` reported failure for good after a large upload.** After the 20 MiB file it returned "pending work remains after drain: … `uploader_cached=1`" on every later drain, although the file was fully uploaded (`describe-file` gave the right size). After a quota rejection it added `commit_queue_conflicts=1`. Loams can't treat a drain error as "not durable" without checking which counter caused it.

**Listing a layer mount while its view changes fails.** Listing, or running `find`, on a layer mount right after writes, or on a fresh mount, often failed with `EAGAIN` ("mount view changed: resource temporarily unavailable"). The manifest needed a retry in 1 of 3 runs. Agents' `ls`, `find` and `git status` will hit this.

### Run 4: scoped tokens (`tokens.sh`): `scoped_token_cannot_escape_prefix` PASS (27/27)

The token was minted from the owner token on A with `--ttl 1h --allow /t/a:read,list,write` and used only on B.

| Check | Result |
|---|---|
| read, list and write in `/t/a` | allowed |
| read, list or write `/t/b` | denied ("fs access denied") |
| `/t/ab/…` (prefix confusion) | denied: the prefix is a path, not a string prefix |
| `/t/a/../b/secret.txt` | denied (normalised to `/t/b/…` first) |
| list `/` or `/t` | denied |
| delete in `/t/a` (not granted) | denied |
| move `/t/a/x` → `/t/b/`; copy `/t/b/x` → `/t/a/` | denied |
| **mount `/`** | **denied** |
| mount `/t/a` (its own prefix) | allowed |
| list tokens; mint a scoped token from a scoped token | denied ("requires an owner FS token") |
| fork an owner's layer; create a layer on `/t/b` | denied |
| `fs-git hydrate-git-workspace` | denied ("scoped token cannot access endpoint") |
| after `disable-file-system-token` (by API key): read, write | denied ("invalid API key") |
| a live `/t/a` mount on B after disable | cached reads and new writes both fail at once with `EACCES`; nothing reached the service |
| after `delete-file-system-token` | denied |

**Propagation:**
- **Disable:** the API call took 0.75 s, and B's next read was denied **0.41 s after the call returned** (1.16 s after it started), against the docs' "about 10 s".
- **Re-enable:** B was allowed again 1.15 s after the call returned.

**Side effects:**
- After a disable, `unmount-file-system` on that mount needs `--force` and leaves its locator in `~/.ti/mounts/`.
- Scoped tokens can't fork layers or use Git-workspace APIs, so with scoped device tokens the control plane (holding the owner token) has to do the fork, and the device hydrates Git from a pushed WIP ref instead of `hydrate-git-workspace`.

### Run 5: limits (`limits.sh`)

**File count, with the limit at 2,000 and the file system holding 52 files:**
- 1,958 files written through A's mount in 2.7 s, then drained in 21 s: 1,961 `PUT`s, all `200`, so 1,991 writes in total.
- **The usage counter lags 30–60 s, and enforcement uses it, so the limit overshot to 2,012 files.**
- Once the counter caught up:
  - a new file through the API was rejected with **HTTP 507** ("tenant file count quota exceeded: server limit=2000 used=2012");
  - overwriting an existing file was allowed;
  - `create-directory` was allowed;
  - **Git-workspace overlay writes were still allowed**, because they don't count as files.
- **Through the mount, `close()` returned 0.** The upload then failed 5 times with 507, and the mount log recorded `commit_terminal_failure … local_data_preserved`. The file stays visible locally, only `drain` (`commit_queue_conflicts=1`) and the mount log show the failure, and the application never sees an error.
- Reads worked throughout: the API, the mount, and other paths.
- `delete-file --recursive /fill` removed 2,012 files in **one request** (9.7 s). The counter took about 60 s to fall.

**Storage, with the limit at 2,048 MiB and one file at most 500 MiB:** 500 MiB files were uploaded with `copy-file --from-local`, each taking 122–132 s at about 5 MB/s upstream. Each upload made 3–6 drive9 calls (`/v2/uploads` initiate, presign batches, complete); the parts go straight to S3.

| Upload | Result | `storage_bytes` after |
|---|---|---|
| b1–b4 (500 MiB each) | copied | 545 MB → 1,069 MB → 1,594 MB → **2,118 MB** |
| b5 | **rejected in 2.2 s**: "tenant storage quota exceeded: server limit=2147483648 used=2118271662 reserved=0 pending=0 delta=524288000" | 2,118 MB |
| a 5-byte file at 2,118 MB | accepted (still under the limit) | |
| one 501 MiB file, after freeing space | **rejected**: "tenant file size quota exceeded: server limit=524288000 requested=525336576" | |

- The storage limit is checked before an upload, against the exact `delta`, so it doesn't overshoot the way the file-count limit does.
- Reads at the storage limit worked: `/t/a/seed.txt` read back. The script's ranged read of `b1.bin` was called wrongly (`--length` without `--offset`), so the large file itself wasn't read back; the file-count phase covers reads at a limit.
- Phase total: 42 counted writes and 64 S3 connections. Whether the S3 part uploads are billed as writes is unknown.

### Run 6: usage visibility

- **Per-file-system usage and limits are API-reachable, but not through a documented `ti` command.** `ti-drive9 admin tenant get --tenant-id <file-system-id> --region-code aws-us-east-1 --json`, authenticated with the org API key, returns:
  - `quota.config`: `max_storage_size` 2048 (MiB), `max_file_size` 500, `max_file_count` 2000, and media and video LLM file limits;
  - `quota.usage`: `storage_bytes`, `reserved_bytes`, `file_count`, and media and video counts.

  The keys go in through `DRIVE9_PUBLIC_KEY` and `DRIVE9_PRIVATE_KEY`, not as flags. The same CLI has `admin tenant set-quota` (`--max-storage-size`, `--max-file-size`, `--max-file-count`, `--tidbcloud-spending-limit`). On this no-card organization, `set-quota` returns **HTTP 402**, "quota updates are not available for free TiDB Cloud organizations; add a payment method or upgrade to change quota", even when *lowering* `max_file_count` from 2,000 to 1,999. Nothing changed. With a card, per-file-system limits are therefore likely settable by API, which the docs don't mention. That wasn't tested, since it needs a card.
- `ti fs describe-file-system` shows status only, with no usage.
- **Request counts (reads, writes) are available nowhere per file system.** The Billing API is per organization and lags (see "Counting writes").

## Totals

| | Count | Cost at list price |
|---|---|---|
| Writes counted on A (the whole task) | 2,512 | $1.26 |
| Reads counted on A | 3,258 | $0.13 |
| Storage | at most 2.1 GB for about 15 minutes | under $0.01 |

All of it falls inside the $5 monthly allowance; B's few uncounted requests add little. The Billing API still showed $0 at the end.

Raw outputs (scrubbed) are in `results/`.

## Teardown: `teardown_leaves_nothing`

**PASS (6/6)** (`results/teardown.txt`):
- `ti fs list-file-systems` returns `{"region_code": "aws-us-east-1", "file_systems": []}`;
- no drive9 mount is left on A;
- the machine-B container and its image are removed;
- no locator is left under any `~/.ti/mounts/`;
- no `fs_credentials` entry is left;
- no token file is left.

The scoped token had already been deleted by `tokens.sh`. Local state was removed as well:
- the spike CA;
- the per-machine `HOME`s;
- drive9's mount logs. `ti` on A wrote those under the passwd home's `~/.cache/drive9`, not under the overridden `HOME`, so overriding `HOME` doesn't fully isolate `ti`'s state.

Stale locators: an unmount that fails, such as `--force` after a token disable or a container without `--init`, leaves its locator behind; the teardown removes them.

## macOS runbook (not run: needs the owner's Mac)

On a Mac with `ti` 0.2.10 installed (`~/.ti/bin`), `TI_TELEMETRY=off`, and the same file system still in place (or a new one):

1. **WebDAV (no macFUSE).**
   ```bash
   export TI_FS_TOKEN="$(cat ~/lc0-owner.tok)" TI_REGION_CODE=aws-us-east-1   # token file 0600, never echo it
   mkdir -p ~/lc0-mnt
   ti fs mount-file-system --mount-path ~/lc0-mnt --driver webdav --mount-profile coding-agent
   ```
   Then:
   - run `spikes/lc0-fs/session.sh` against a plain directory (WebDAV has no Git-workspace integration);
   - time how long the Linux machine takes to see a closed file;
   - check that `ti fs drain-file-system` errors on WebDAV (expected), and that `unmount-file-system` flushes;
   - `kill -9` the WebDAV process mid-write, then remount, and record what survives.
2. **FUSE (macFUSE installed).** Repeat with `--driver fuse`, then run `RUN=lc0-mac spikes/lc0-fs/handoff.sh` with machine A as the Mac. `lib.sh`'s `tiA` works with `HOME_A=$HOME`; use the Linux host or container as B. Check:
   - layer and checkpoint mounts, which need FUSE;
   - `fs-git clone-git-workspace` and `hydrate-git-workspace`;
   - the `EAGAIN` listings.
3. Record the mount time, drain time, propagation, and whether `coding-agent`'s local overlay paths behave the same, especially `.git` and `node_modules` on APFS.
4. Unmount and remove `~/.ti/mounts/*.locator.json`, `~/.ti/fs_credentials/…` and `~/.ti/drive9-home/…` for the file system.

## Answers for the LC0 report

- **Writes per session:** about 77 for 30 edits, a test run and a commit, in a Git workspace. That is **$0.04 per session**, and about 125 sessions fit in the $5 allowance. Add 14 writes for every drain of a mounted Git workspace.
- **Drain latency:**
  - 0.02–0.08 s on a layer mount;
  - 4.9–6.4 s on a mount holding a Git workspace;
  - 26.5 s after a 20 MiB write.

  B sees a closed file about 2.5 s after the close, with or without a drain.
- **D1713's 5-minute drain:**
  - Keep the drains at handoff and at approval.
  - Drop the timer, or drain only when dirty. Durability doesn't need it, because a closed file is uploaded in about 2.5 s and a killed mount replays its local shadow on the next mount.
  - On Git-workspace mounts, each timed drain costs 14 writes for nothing.
- **Per-file-system limits and usage by API:**
  - Usage and limits: yes, through `ti-drive9 admin tenant get` (undocumented).
  - Setting limits: the API exists (`set-quota`) but returns 402 without a card.
  - Request counts: none, per file system or per organization.
- **Hosted cohort (Q1263): no-go at $0, as §25.13 already rules.** Without a card, limits can't be set per file system, and the organization is limited to one file system per region, so the cohort can't be built at all. With a card, there is no spending cap, and there is no per-file-system write counter to enforce D1718's 2,000-write budget; Loams would have to count writes itself. BYOG workspaces: **go**, with the caveats above.
