# 36 — Loams Git: a WAL on the Bucket, Smart HTTP, Agent Scopes and a Build Cache

> **Revised 2026-10-10: monorepo mode (§19).** The owner directed on 2026-10-10: *"loam git should be similar to google monorepo, download only touched file, spawn million of worktrees on s3, it should wrap git command and take over"*. §19 designs it: server-side workspaces on the bucket (CitC-style overlays with a snapshot per save), a lazy FUSE client (`loams-vfs`), and a `git` shim that takes over porcelain and falls through to real git. The WAL core (§4–§5, D388–D392) and the build cache and mirror (§8–§9) stand. **Superseded or amended by §19:** §6.4 and D397 (amended by D826–D829), §13's phases and D411's schedule (D825), §2.2's LFS non-goal (D837). The owner added the same day **secrets as code** (§19.13): `env/` files committed like normal files whose values never become git objects. New decisions are **proposed D825–D847** and open questions **Q750–Q760**; they are not yet in the decision log (Q735–Q747 are reserved by the CL1 plan, so this document starts at Q750; numbers are confirmed when the log is updated). The plan is [GT1 (2026-10-10, rewritten for monorepo mode)](../plans/2026-10-10-gt1-loams-git.md).

> **Added 2026-10-16: §20 (proposed).** After comparing Loams Git with Enroute and Anthropic's Scion, the owner approved five proposals: a TiKV-backed `RefLog` for hot repositories (D1025), a server-side `Land` RPC (D1026), per-commit object locality in packs (D1027), a regional cache tier (D1028), and licensing and positioning (D1029); open questions Q906–Q909. They are Proposed in the decision log, with GT1 Tasks 88–92.

Status: **Approved** (owner defaults, 2026-10-02: "do suggested for all") · 2026-10-01; §19 **Proposed** (2026-10-10). Source: §14 "Loams Git (hosting, WAL, build cache)" and the related open questions in §15 of the owner's draft "Loams Serverless Runtime — Consolidated Plan" (2026-09-30, `chatdump.md` lines 868–946). The owner asked on 2026-10-01 to fold that draft into the design docs, the decision log and the plans. This document **extends §15 §3 (repos on the bucket)**; it does not fork that design. Where it changes an approved part of §15, the change is marked with its D-number and listed in §15 "Conflicts". The `Fs` trait is §17 below. Running Loams Git on Cloudflare (Workers, Durable Objects, Containers) is a commercial Cloudflare target (`loams-platform`): since the owner's ruling of 2026-10-02 ("move Cloudflare, OpenRTB etc. commercial to private repos"; §38 D440 on PR #182) the former §35 lives there (private). This repository's Git core stays portable to it but does not depend on it.

Decisions **D388–D399**; open questions **Q384–Q395** (the range D380–D399 / Q380–Q399 was shared with the former §35; D381 and D382 stay here, in §17, and D380, D383–D387 moved to `loams-platform`, private). Plans: [GT1](../plans/2026-10-01-gt1-wal-git-core.md) (the WAL git core and `git-remote-loams`), [GT2](../plans/2026-10-01-gt2-smart-http.md) (Smart HTTP for stock git) and [GT3](../plans/2026-10-01-gt3-build-cache-and-mirror.md) (the sccache backend and the crates mirror). GT4 and GT5 are not yet planned.

Markers: **(source)** means read in the upstream repository or documentation on 2026-10-01, at the URL in §18. **(verify)** means the plan that builds it checks it first. **(estimate)** means computed, not measured. **(draft)** means the figure comes from the owner's draft and was not re-checked.

---

## 1. Summary

| # | Decision | Status |
|---|---|---|
| D388 | **Loams Git extends §15 §3.** A repository's refs move from §15's single CAS'd `refs` document to a **per-repository write-ahead log of create-only segments plus periodic checkpoints**, under the same `ns/<ns>/repos/<repo_id>/` prefix. Packs stay immutable and content-addressed. Forks stay O(1). Repos stay a service with their own bucket WAL, not a sixth object kind and not a Loams stream (proposed answer to Q15). Track **GT** (GT1–GT5) delivers W1's repository scope and parts of W2 (§13) | Approved (owner defaults, 2026-10-02) |
| D389 | **The commit point is the create-only PUT of the next segment**, `wal/<seq:020>.lgw` with `If-None-Match: *`. There is no separately CAS'd head pointer: `head` is a hint, written at most once a second. Exactly one batch can own a sequence number on every supported store (S3, R2, GCS `ifGenerationMatch=0`, Azure `If-None-Match: *`, RustFS) | Approved (owner defaults, 2026-10-02) |
| D390 | **Formats.** A WAL record is a **CloudEvent 1.0** in the protobuf format (D270) whose `id` is the transaction's idempotency key and whose version is the `type` suffix plus `dataschema` (D364's profile; amended 2026-10-02 by D415), with the extensions `tenantid`, `traceparent` and `loamsseq`, whose data is a `loams.git.v1` message; a segment frames one `CloudEventBatch` with magic, version and CRC32C (§03 §6) and is at most 1 MiB; a checkpoint holds the full ref state, the live pack set, the fork parent and the idempotency window; a push is stored as **one object**, `packs/<checksum>.lpk` (pack ‖ index ‖ footer), named by the pack's trailer checksum | Approved (owner defaults, 2026-10-02) |
| D391 | **One sequencer per repository, with group commit.** The rendezvous owner of `(ns, repo)` (D75) under a lease (on a commercial Cloudflare target (`loams-platform`), a Durable Object per repository). One segment PUT in flight at a time; every transaction that arrives meanwhile joins the next segment. **Correctness never depends on there being one sequencer**: two writers for one repository fence each other on the segment name. Target **30 pushes/s per hot repository** (draft) | Approved (owner defaults, 2026-10-02) |
| D392 | **Four traits in `loams-git`, with exact semantics** (§5): `WalStore` (fenced, monotonic append, idempotent on the batch id; contiguous reads), `RefLog` (atomic multi-ref transactions, linearizable commits and reads, idempotent on the idempotency key within a 1 h window), `BlobStore` (create-only, content-named blobs; range reads) and `Materializer` (scoped, on-demand blob fetch for agents) | Approved (owner defaults, 2026-10-02) |
| D393 | **Usage is exposed through §27's hooks only.** Git, the build cache and the package mirror export metric families per namespace (§11) and append a CloudEvent per committed transaction to the repository's event stream. No meter events and no ledger in this repository. Amends the draft's "CloudEvents feeding the metering ledger" (D190, D200–D202) | Approved (owner defaults, 2026-10-02) |
| D394 | **Smart HTTP.** `git-upload-pack` speaks **protocol v2** (`ls-refs`, `fetch` with `filter`, `shallow` and `wait-for-done`, `object-info`); a v0/v1 upload-pack is added in GT2 only if a client in W1's matrix lacks v2. `git-receive-pack` speaks v0/v1, since protocol v2 has no push (`report-status`, `report-status-v2`, `atomic`, `delete-refs`, `side-band-64k`, `ofs-delta`, `push-options`, `quiet`). The server loop is Loams’, on gitoxide primitives. **Stock `git` runs only as an unmodified separate process**: the test oracle, the repack worker (D396) and, on the client, the pack steps of `git-remote-loams`'s pushes (D395). Answers §15 Q1 and the draft's last open question | Approved (owner defaults, 2026-10-02) |
| D395 | **`git-remote-loams`.** In GT1 it is a serverless helper that reads and writes the bucket directly through the core (capabilities `fetch`, `push`, `option`; `loams::<store-url>` addresses). In GT2 it adds `stateless-connect` to tunnel protocol v2 to an in-process upload-pack, which is what partial clone and lazy fetch need, and `loams://<host>/<ns>/<repo>` addresses that reach a Loams server | Approved (owner defaults, 2026-10-02) |
| D396 | **Compaction is never on the push path.** Checkpoints every 256 segments or 8 MiB of replay; geometric repack with bitmaps and a multi-pack index by **stock `git repack`** on a worker's local mirror (gitoxide cannot yet write deltas or bitmaps); pack-set changes are committed through the WAL like pushes; one compactor per repository, by lease; GC by fork-family reachability after the §03 §7 grace period | Approved (owner defaults, 2026-10-02) |
| D397 | *(Amended (proposed) by D826–D829, §19: `loams-vfs` becomes Loams Git's primary client and workspaces become server-side records; scopes and write admission stand.)* **`loams-vfs` is §15 §5.1's `/workspace` lower layer** (GT4). A scope is a list of cone-mode sparse-checkout paths carried in the agent's vended token; blobs are fetched on first read; **write admission** refuses a commit that changes paths outside the scope; a commit is one WAL record whatever the folders it touches. **No per-scope WAL partitions** until GT4 measures that the per-repository sequencer is the bottleneck (Q391) | Approved (owner defaults, 2026-10-02) |
| D398 | **Build cache: sccache** (Apache-2.0, v0.18.0) is the primary cache. Loams serves it two ways: **direct** (sccache's S3 backend against R2, RustFS or S3 with vended prefix-scoped credentials; no Loams code on the path) and **through the gateway** (sccache's WebDAV backend against a Loams endpoint that meters hits and misses, refreshes entries on hit for approximate LRU and enforces trust). **Trust model:** trusted branches write; forks, pull requests and agent sandboxes read only, with an optional private scratch prefix. BuildCache (zlib) only if a toolchain sccache cannot handle needs it | Approved (owner defaults, 2026-10-02) |
| D399 | **Package mirror: a crates.io sparse-index read-through** in the `gateway` role (§15 §6): index files cached with ETag revalidation, `.crate` files content-addressed by their SHA-256 `cksum` in the public-packages namespace, the §15 §6 policy (allowlists, quarantine, audit records). The index lives in the object store, **not** in a Durable Object or D1 | Approved (owner defaults, 2026-10-02) |

## 2. Goals and non-goals

### 2.1 Goals

1. **Object storage is the source of truth for Git** (D1). Node disks, Durable Object storage and NVMe mirrors are caches. Any node, or a client with bucket credentials, can rebuild a repository's state from the bucket.
2. **Pushes are durable when acknowledged.** A push is acknowledged only after its pack and its ref update are in the bucket (the rule Continuity and Spokes share, §3).
3. **Throughput for agent fleets.** Thousands of small repositories with a few pushes a minute each, and hot repositories (a monorepo with an agent fleet) at 30 pushes/s (draft), without a stateful replica fleet.
4. **Stock clients.** `git` over Smart HTTP, and gitoxide, libgit2 and JGit, pass §15 W1's client matrix.
5. **Agents read only what they touch** (§15 principle 4), per folder scope, and cannot write outside it.
6. **One build cache and one package mirror**, with tenant isolation and a trust model that stops cache poisoning.
7. **Portable.** The same core runs on Kubernetes (NVMe, RustFS or S3) and on a commercial Cloudflare target (`loams-platform`); nothing in the core assumes either.

### 2.2 Non-goals

- A GitHub replacement: no pull-request UI, issues or CI (§15 §14). Webhooks and mirroring only.
- A distributed POSIX filesystem or concurrent multi-writer workspaces (§15 §14).
- Gossip, any-node writes and NVMe replica fleets in GT1–GT3. They are GT5, after measurement (D391).
- SHA-256 repositories in GT1–GT2. The object format is a field from the first format version (Q393).
- Git LFS beyond §15 §3.2's plan (batch API on the namespace CAS, W1). *(Revised by D837, §19.12: lazy fetch makes LFS unnecessary for Loams workspaces; an LFS-compatible read path for stock clients comes after GA.)*
- *(§19 note: "a distributed POSIX filesystem or concurrent multi-writer workspaces" stays a non-goal. A §19 workspace is a single-writer virtual filesystem over one repository (D827), not a shared POSIX filesystem.)*

## 3. Reference systems

| System | What it does | What Loams takes |
|---|---|---|
| **Cursor Continuity** ("Git at any scale", Vicent Martí, cursor.com, 2026-08; InfoQ, Leela Kumili, 2026-09-30) | An S3-backed WAL is the source of truth; NVMe repositories are warm caches; "we never acknowledge a push until it has been fully persisted"; batching hides PUT latency; rendezvous hashing picks preferred nodes; compare-and-swap on S3 lets any server accept a push; UDP gossip is a hint and conditional reads verify state; only the primary compacts, replicas download packs. **Reported, synthetic, not independently verified** (InfoQ's words): about 120 pushes/s on S3 Standard and over 300 on S3 Express One Zone, with compaction the bottleneck at the top rate (source) | The WAL as truth, ack-after-persist, batching, rendezvous placement, primary-only compaction. Not gossip or any-node writes yet (GT5) |
| **GitHub Spokes** (DGit, 2016; "Stretching Spokes", 2017, updated 2025-06-03) | Three replicas on three servers; writes stream to all three and commit when a quorum confirms; updates use a three-phase commit that doubles as a distributed lock; four round trips to distant replicas (source) | The contrast: Loams keeps no stateful replicas; the bucket is the quorum |
| **Delta Lake's transaction log** | Commits are files `_delta_log/<version>.json` created with put-if-absent; the next version number is the fence | The commit protocol of D389: a sequential, create-only object name is the compare-and-swap |
| **§15 §3** (approved) | One CAS'd `refs` document per repository, packs create-only, forks by alternates, Smart HTTP v2 in the `gateway` role, repo-affinity group commit | Everything except the single document (D388) |
| `git-remote-object-store` (Apache-2.0), `awslabs/git-remote-s3` (Apache-2.0, last push 2026-08-20), Cloudflare Artifacts (closed) | Client-side helpers that use a bucket as a serverless Git remote; a Git server on Durable Objects and R2 | GT1's serverless helper (D395); the Durable Object sequencer of a commercial Cloudflare target (`loams-platform`) |
| **go-git** (Apache-2.0, v5.19.2 2026-07-29) | Server-side `upload-pack` and `receive-pack` with a pluggable `storage.Storer`; its v6 `main` has v2 code while its compatibility table still lists v2 as unsupported (source) | A reference for the server loop. Not a dependency (Go; Loams’ core is Rust and must run in Workers) |

## 4. The WAL on the bucket (D388–D391)

> **Durability substrates (2026-10-16, [§58](58-data-access-and-htap.md) §8.4–§8.5, D1059, D1060).** Loams Git uses no `loams-wal`, which is the Postgres WAL service. Objects stay on the bucket. The bucket WAL below stays the default `RefLog`, and hot repositories may opt into the TiKV `RefLog` (§20.4, D1025). The segment code of `wal/*.lgw` (create-only PUTs, unknown-outcome resolution, framing, checkpoints, GC) moves into the shared bucket-segment library `loams-seglog`, which `loams-wal`'s `.lwal` offload also uses. The formats here do not change.

### 4.1 Layout

Extends §15 §3.1 and §15 §11:

```
ns/<ns>/repos/<repo_id>/
  head                          # hint: {seq, checkpoint_seq, written_unix_ms}; best effort, ≤ 1 write/s
  wal/<seq:020>.lgw             # create-only segments; segment n's PUT is the commit point of its batch
  checkpoints/<seq:020>.lgc     # create-only; the full state after segment seq
  packs/<checksum:hex>.lpk      # create-only, content-named: pack ‖ idx ‖ footer (D390)
  midx/<seq:020>.midx           # multi-pack index over the pack set at seq (compaction output, D396)
  commit-graph/<seq:020>.graph  # written by compaction
```

§15's `refs` document is replaced by `head`, `wal/` and `checkpoints/` (D388). §15's `packs/<ulid>.pack` + `.idx` become one `.lpk` object named by the pack checksum (D390), so a retried upload writes the same name and a push costs one PUT for its data.

### 4.2 The commit protocol (D389)

A writer that holds the state after segment `n` commits a batch by:

```
PUT wal/<n+1>.lgw   If-None-Match: *     body = segment(n+1, batch_id, events)
  200 → committed at n+1
  412 → fenced: segment n+1 exists. GET it, apply it, re-validate the batch's transactions
        against the new state, try n+2
  timeout / 5xx after the request was sent, or 429 (R2's one-write-per-second limit when writers race one seq)
        → unknown: GET wal/<n+1>.lgw
        own batch_id → committed;  another batch_id → as 412;  404 → resend the same bytes
```

- **Exactly one batch per sequence number.** Every store Loams supports refuses a second create of the same name: S3 (`If-None-Match`, 2024-08-20), R2 (S3 API and the Workers binding's `onlyIf`), GCS (`ifGenerationMatch=0`), Azure (`If-None-Match: *`) and RustFS (412 under a write lock; source). D178's `ObjectStoreProvider` already refuses a provider without conditional writes.
- **Why no head pointer.** The draft advanced a head pointer by conditional write after each segment. That costs a second PUT per batch, adds a second linearization point, and contends on one key. **R2 limits writes to the same key to one per second** (R2 limits page, updated 2026-06-08), which would cap a CAS'd head at one commit a second. Sequential segment names are written once each by a successful writer, so the limit applies only when fenced writers race one name, and their `429` is resolved by the GET above; `head` is coalesced to at most one write a second and only speeds up readers.
- **Contiguity.** A writer appends `n+1` only after it has read or written segment `n`. Readers read forward from a checkpoint and stop at the first missing number, so a segment written above a gap (which a correct writer never does) is unreachable, and GC deletes it after the grace period.
- **Strong reads.** S3 and R2 give read-after-write and list consistency (R2 consistency page), so "GET `wal/<n+1>` returns 404" proves that `n` is the tail at that instant. A linearizable read costs one GET past the state a node holds.
- **RustFS atomicity.** RustFS's conditional PUT is atomic for bodies up to 1 MiB (its PR #6798, 2026-08-28; source). Segments are capped at 1 MiB on every store (D390). Packs are larger, but a pack's name is its content, so two racing creators write identical bytes and either outcome is correct.

### 4.3 Formats (D390)

**Segment** `wal/<seq:020>.lgw`, format version 1:

```
header   magic "LGITWAL\0" (8) | format_version u16 LE = 1 | flags u16 = 0 | seq u64 LE
         | batch_id [16] (random per batch; recognises one's own segment after an unknown outcome)
         | created_unix_ms i64 LE | body_len u32 LE
body     io.cloudevents.v1.CloudEventBatch (protobuf format, the encoding D270 uses on gRPC)
trailer  crc32c u32 LE over header ‖ body | magic "LGITWEND" (8)
```

Readers accept format versions N and N−1 (§03 §6). A body over 1 MiB is refused at encode.

**Events.** Each event in a batch is one transaction:

| Attribute | Value |
|---|---|
| `specversion` | `1.0` |
| `id` | the transaction's idempotency key (§4.5) in lowercase hex; a ULID the sequencer generates for events without a client key (compaction, config changes). Dedup is on (`source`, `id`), as D270's. The event's position is `loamsseq` plus its index in the batch (D364, D415) |
| `source` | `/ns/<ns>/repos/<repo_id>` |
| `type` | `io.loams.dev.git.reftxn.v1`, `io.loams.dev.git.packset.v1` (compaction, D396), `io.loams.dev.git.config.v1` (`HEAD`, protection rules), under the owner's prefix `io.loams.dev.<domain>.<name>.v1` (ruling of 2026-10-01; the draft's §6 had `io.loams.<domain>.<name>.v1`) |
| `time` | the sequencer's clock at commit, RFC 3339 |
| `datacontenttype` / `dataschema` | `application/protobuf` / `urn:loams:proto:loams.git.v1.RefTransaction` (or `PackSetChange`, `ConfigChange`); with the `type` suffix `.v1` this is the version, so there is no `schemaversion` extension (D364) |
| `tenantid` | `<org>/<ns>`, from the credential, never from the request (D182's rule) |
| `traceparent` | the receive request's W3C trace context, if any |
| `loamsseq` | the segment's sequence number, as a decimal string (CloudEvents integers are 32-bit) |

**Data messages** (`proto/loams/git/v1/wal.proto`):

```protobuf
syntax = "proto3";
package loams.git.v1;

enum ObjectFormat { OBJECT_FORMAT_UNSPECIFIED = 0; SHA1 = 1; SHA256 = 2; }

message RefUpdate {
  string name = 1;                       // "refs/heads/main"; validated as git check-ref-format
  oneof expect {
    bytes old_oid = 2;                   // must currently point here
    bool must_not_exist = 3;             // create
    bool any = 4;                        // only for force updates the policy allows
  }
  bytes new_oid = 5;                     // empty = delete
}

message PackRef {
  string checksum = 1;                   // hex of the pack trailer checksum; names packs/<checksum>.lpk
  uint64 object_count = 2;
  uint64 pack_len = 3;                   // bytes of the pack section
  uint64 idx_offset = 4;                 // the idx section's offset in the .lpk
  uint64 idx_len = 5;
  bool has_bitmap = 6;
}

message RefTransaction {
  repeated RefUpdate updates = 1;        // applied atomically, all or none
  repeated PackRef packs = 2;            // packs this transaction adds (usually 0 or 1)
  string principal = 3;                  // who pushed (credential subject)
  repeated string push_options = 4;
  ObjectFormat object_format = 5;
  bytes request_digest = 6;              // the idempotency key's request digest (§4.5; added by GT1 Task 8, T8.1)
}

message PackSetChange { repeated PackRef added = 1; repeated string removed = 2; string midx = 3; string commit_graph = 4; }
message SymRef { string name = 1; string target = 2; }
message Protection { string pattern = 1; bool deny_delete = 2; bool deny_force = 3; }
message ConfigChange { repeated SymRef symrefs = 1; repeated Protection protections = 2; bytes request_digest = 3; }   // 3: GT1 Task 13, T13.2

message ForkParent { string repo_id = 1; uint64 seq = 2; }
message IdemEntry { string key = 1; bytes request_digest = 2; uint64 seq = 3; uint32 index = 4; int64 expires_unix_ms = 5; }
message Checkpoint {
  uint64 seq = 1;
  ObjectFormat object_format = 2;
  map<string, bytes> refs = 3;
  repeated SymRef symrefs = 4;
  repeated Protection protections = 5;
  repeated PackRef packs = 6;            // the live pack set
  ForkParent parent = 7;                 // set on forks: packs fall through to the parent at its seq
  repeated IdemEntry idempotency = 8;    // entries still inside the window
}
```

**Checkpoint** `checkpoints/<seq:020>.lgc`: header magic `LGITCKPT`, version u16 = 1, seq u64; body `Checkpoint`; trailer CRC32C and `LGITCEND`. No size cap (a repository with a million refs has a large checkpoint); it is written with a multipart upload above 64 MiB.

**Pack object** `packs/<checksum>.lpk`: the pack bytes exactly as received or generated, then the version 2 `.idx` bytes, then a 32-byte footer `magic "LGITLPK\0" | pack_len u64 | idx_len u64 | crc32c u32 | reserved u32`. Range reads fetch the footer, the idx's fan-out table and object entries without downloading the pack. A local mirror (D396, GT5) splits it into `pack-<checksum>.pack` and `.idx`.

**As built (GT1a, Tasks 3–5; `crates/loams-git/src/format/`).** The layouts are as above, pinned by `crates/loams-git/tests/golden/{segment_v1.lgw,checkpoint_v1.lgc,lpk_v1_footer.bin}`, with these precise rules:
- `wal.proto` is crate-local, at `crates/loams-git/proto/loams/git/v1/wal.proto`, and is verbatim; buf does not lint it (GT1 R0.5). It is generated with prost, with every map in key order, so equal checkpoints have equal bytes.
- The segment body is `loams_cloudevents::protobuf::encode_batch` (GT1 Task 3). That function writes a typed attribute value with its protobuf type only when the type prints back the same string; otherwise it writes `ce_string`, so `time` keeps Loams' millisecond form (GT1 ruling T3.1).
- `encode_segment(seq, batch, created_unix_ms)` takes the header's time, because `WalBatch` has none. It refuses a body over `MAX_SEGMENT_BYTES` and any event whose `loamsseq` is not `seq`. `decode_segment` refuses non-zero flags, a length that disagrees with `body_len`, a body over 1 MiB, and an event whose `loamsseq` is not the header's seq. Readers accept versions 1..=1 (N−1 = N until version 2 exists).
- The checkpoint header has no body length: the body is everything between the 18-byte header and the 12-byte trailer. A header seq that differs from `Checkpoint.seq` is corrupt.
- `LpkFooter` also carries the CRC32C, for scrubs. A non-zero `reserved` word is corrupt.
- Transaction events are written with their attributes in the order the protobuf format reads them back: `specversion`, `id`, `source`, `type`, then `datacontenttype`, `dataschema`, `loamsseq`, `tenantid`, `time` and `traceparent`. A decoded segment therefore equals the batch that was written. `time` is RFC 3339 in UTC with milliseconds. The reader refuses any other attribute.
- The event `id` is the `IdemKey` string itself (a derived push key is already lowercase hex). `to_cloudevent(event, seq)` takes no index: an event's position is its place in the batch.

### 4.4 The sequencer and group commit (D391)

One sequencer task serves each active repository. It holds the state after the last segment it has seen (refs, symrefs, protections, the pack set, the idempotency index) and a queue of validated transactions.

```
loop:
  wait until the queue is non-empty (and no PUT is in flight)
  group = take up to max_group_txns (64) or max_segment_bytes (1 MiB) from the queue
  state' = state
  for txn in group (arrival order):
      if idempotency hit: answer the original receipt; drop from group
      check every update's expectation against state'   (earlier txns of this group included)
      check protections (deny_delete, deny_force) and the token's scope (D397)
      pass → apply to state', keep;   fail → answer Rejected{per ref reasons}; drop
  PUT wal/<state.seq+1> (§4.2)
      committed → state = state'; answer every kept txn {seq, index}; schedule hint and stream mirror
      fenced    → apply the foreign segments; re-validate the kept txns from scratch; retry
```

- **One PUT in flight.** Transactions that arrive during a PUT form the next group, so the group size grows with load and no fixed linger is needed (`group_linger`, default 0 ms, exists for benchmarks).
- **What the sequencer does not do.** Pack upload, pack indexing, fast-forward checks and connectivity checks happen in the receive path *before* a transaction is queued (§6.3). Fast-forward is a property of the old and new commits, not of the ref state: if `old_oid` still matches at commit, a new commit that descends from it is a fast-forward. The sequencer therefore checks only object ids, protections and scopes, in memory.
- **Throughput (estimate).** With a segment PUT latency `L`, the sequencer commits `1/L` groups a second. At `L` = 50–150 ms that is 7–20 groups/s, so 30 pushes/s needs an average group of 2–5 transactions, which forms on its own at that load. A push's acknowledgement waits for its pack PUT, up to one in-flight PUT, and its own segment PUT: 3 × `L` plus transfer time. GT1 measures `L` on R2, S3 Standard, S3 Express One Zone and RustFS (Q384).
- **Placement.** The sequencer runs on the rendezvous owner of the placement key `(ns, repo, repo_id)` (D75) under the metastore lease `task/git-seq/<ns>/<repo_id>`, and nodes that receive a push for a repository they do not own forward the transaction to the owner, as §15 §3.2's repo-affinity routing describes. On a commercial Cloudflare target (`loams-platform`) the sequencer is a Durable Object instead. A wrong owner, a zombie after a lease loss, or a `git-remote-loams` client writing the bucket directly is fenced by §4.2, never trusted.
- **Idle repositories** keep no sequencer. The first push loads the hint, the checkpoint and the segments after it.

**As built (GT1b, Task 14; `crates/loams-git/src/{owner.rs,forward.rs}`, `crates/loams/src/api/git_internal.rs`, plan rulings T14.1–T14.7).** `RepoSequencers` routes every commit (`commit`, `commit_config`): the owner is the first `gateway` node of the rendezvous ranking of `PlacementKey::repo(ns, h)`, where `h` is the first 8 bytes, big-endian, of `sha256(repo_id)` (kind byte 1 in the hash input), and only `gateway` nodes rank for a repository. The owner takes the lease `task/git-seq/<ns>/<repo_id>` straight through the metastore (TTL 10 s, renewed every 3 s, owner `v1;<node>;<incarnation>;<addr>`) before its first commit; a refused or expired renewal closes the sequencer, and calls still holding it answer `Unavailable`. Another node posts the commit to `POST /internal/v1/git/commit` (a postcard body carrying the whole transaction and its idempotency key); when no answer comes within `FORWARD_TIMEOUT` (2 s), it marks the owner unreachable and commits through its own sequencer without the lease (`fallbacks` counter), and the owner's late commit of the same key is then replayed, not applied twice. The owner serves a forwarded commit whatever its placement says, so nothing is forwarded twice. A lease held elsewhere never blocks a commit: the segment PUT fences it. Reads (`snapshot`, `watch`) run on any node. A sequencer unused for `SEQ_IDLE_EVICT` (10 min) is closed and its lease released. A single server (`loams dev`, standalone) runs every sequencer itself, with no lease and no forwarding.

**Measured (GT1a, Task 11; `bench/git-wal`, `bench/results/git-wal/20261012T0955Z-gt1-t11.jsonl`).** `git-wal-bench` drives `BucketRefLog` directly: each pusher uploads a 4 KiB pack, then commits an update of its own branch; `--put-delay-ms` adds that latency to every segment append and pack upload. Machine: Intel Core Ultra 5 225H (14 threads), Linux 7.2.4, debug build, 2026-10-12, other builds running. One repository, 32 pushers, 20 s per row unless noted.

| Store | Added PUT delay | Commits/s | Commit p50 / p99 | Push p50 / p99 | Mean group | PUTs / GETs per commit |
|---|---|---|---|---|---|---|
| in-memory | 20 ms | 521 | 38 / 60 ms | 60 / 84 ms | 15.6 | 1.07 / 1.0 |
| in-memory | 50 ms | 271 | 63 / 112 ms | 116 / 166 ms | 15.6 | 1.07 / 1.0 |
| in-memory | **100 ms (the gate)** | **145** | **109 / 212 ms** | 212 / 315 ms | 15.4 | 1.07 / 1.0 |
| in-memory | 150 ms | 98 | 158 / 312 ms | 311 / 465 ms | 15.3 | 1.08 / 1.0 |
| in-memory, 8 pushers | 100 ms | 33 | 105 / 207 ms | 207 / 311 ms | 3.5 | 1.32 / 1.0 |
| RustFS 1.0.0 (podman, loopback) | 0 | 619 | 27 / 60 ms | 39 / 169 ms | 13.9 | 1.07 / 1.0 |
| RustFS 1.0.0 | 100 ms | 93 | 231 / 510 ms | 346 / 627 ms | 11.4 | 1.10 / 1.0 |
| RustFS 1.0.0, 1 pusher | 0 | 33 | 16 / 25 ms | 29 / 78 ms | 1.0 | 2.03 / 1.0 |
| RustFS 1.0.0, 8 repositories, 64 pushers | 0 | 731 | 42 / 294 ms | 65 / 543 ms | 2.9 | 1.35 / 1.0 |

The **gate is met**: at 100 ms per PUT, one hot repository commits 145/s in memory and 93/s on RustFS (the delay is added to RustFS's own ~15 ms), with commit p99 at 212 and 510 ms, under 1 s. Throughput is about `group / L`: groups grow with load (15 at 32 pushers), as §4.4 estimated. The one GET per commit is the pack HEAD before queueing; a PUT per commit beyond the segment's share is the pack. R2, S3 Standard and S3 Express One Zone are not measured yet (GT-Q8: no credentials). **Q393 is decided: packs stay out of the WAL** (D390 stands). A pack PUT overlaps other groups' segment PUTs and costs one `L` of push latency, not throughput, and keeping packs out keeps every segment within RustFS's 1 MiB atomic conditional PUT.

### 4.5 Idempotency

- **Keys.** A client may send `Idempotency-Key` (API pushes). For a git push, the receive path derives `sha256(principal ‖ pack checksum ‖ sorted (name, expect, new) triples)`, so git's own retry of the same push over a flaky connection is answered with the first push's result.
- **Window.** Keys are kept for `idempotency_window` (1 h, the same default as D270's CloudEvents dedup window) in the sequencer's index and in each checkpoint (`IdemEntry`). A retry inside the window returns the original receipt (`replayed: true`). A key reused with a different request digest is refused with `IdempotencyMismatch`, as Live's idempotency records are bound to their arguments (R1 review of #76).
- **After the window** a retry is a new transaction, and its old-oid expectations decide it: a duplicate push of an already-applied fast-forward finds `old_oid` changed and is rejected, never applied twice.

**As built (GT1a, Task 8; `crates/loams-git/src/state.rs`, `reflog.rs`).** `RefTransaction` carries its `request_digest` (field 6, added by this task), so a sequencer that rebuilds its state from segments written after the last checkpoint restores each key with its digest, and a retry after a restart or a fence is still answered `Replay` or `IdempotencyMismatch`, not a fresh decision. An entry expires at the transaction event's `time` plus the window, both when it is applied live and when its segment is replayed. `RepoState::check` decides in this order: the idempotency index, a ref named twice in one transaction (`InvalidName`), each update's expectation (`Stale { current }`, `Exists`), then protections (`deny_delete` refuses a delete; `deny_force` refuses any update that expects `Any`). A protection `pattern` is a ref name in which each `*` matches any run of characters, `/` included. Symbolic refs are keyed by `HEAD` or a ref name. The writer's scope is not checked until GT1 Task 16 (branch prefixes) and GT4 (cones).

### 4.6 Reads

| Read | How | Cost |
|---|---|---|
| `Latest` on the sequencer's node | in-memory state, then one GET of `wal/<seq+1>` (404 → current) | 1 GET |
| `Latest` elsewhere | `head` → the checkpoint it names → segments after it → probe until 404 | 1 + 1 + k GETs; cached checkpoints make it k + 2 |
| `AtLeast(seq)` | as `Latest`, but a node whose state is at or past `seq` answers without the probe | 0–1 GET |
| `Exactly(seq)` | the latest checkpoint at or below `seq` plus segments up to `seq` (retained 24 h, as collections keep manifests, D38) | — |

A push returns its sequence number as a consistency token, `git:<repo_id>:<seq>`, so an agent that pushes and then reads (`ls-refs` with `server-option=loams-at-least=<seq>`, or a code-index search, §15 §4) sees its own push.

### 4.7 Forks

Forking repository A at sequence `s` writes repository B's `checkpoints/00000000000000000000.lgc` with `parent = {A, s}`, A's refs at `s` and an empty pack set: one PUT, no data copy (§15 §3.1's alternates rule, kept). Object lookup falls through to the parent's pack set at `s`, recursively. GC computes reachability across a fork family (§15 §3.3).

**As built (GT1b, Task 13; `crates/loams-git/src/catalog.rs`, plan rulings T13.1–T13.4).** The catalog changes only by create-only writes: `_names/<name>.json` is written after the repository's checkpoint 0 and is the commit point of a create or fork; a delete writes `_deleting/<repo_id>.json`, and a purge after `GC_GRACE` removes the objects, the name and the marker unless a fork still reads the repository. A configuration change (`HEAD`, protections) carries a request digest (`ConfigChange.request_digest = 3`) and is idempotent like a ref transaction.

### 4.8 The event-stream mirror

After a segment commits, the sequencer appends each transaction's CloudEvent to the namespace stream `_git` (partition key `repo_id`) through `ProduceCloudEvents` (D270), deduplicated by `source` + `id`. This is §15 §3.2's "append a record to the repo's event stream", at least once with a repair sweep that compares `head` with the stream's last `loamsseq`. Code-index links (§15 §4) and usage consumers (§11) read it. On a commercial Cloudflare target (`loams-platform`) the sequencer posts to a Queue whose consumer calls `ProduceCloudEvents`.

**As built (GT1b, Task 15; `crates/loams-git/src/mirror_events.rs`, `crates/loams/src/api/git_events.rs`, plan rulings T15.1–T15.5).** `_git` is a reserved system stream (8 partitions, keyed by `repo_id`), created only by the metastore command `EnsureSystemStream` (every backend and the conformance suite); public producers (`POST …/events`, record produce, `ProduceCloudEvents`) are refused, so its events cannot be forged. The mirror is a task beside each sequencer that commits: it follows the WAL with `watch`, publishes through the CloudEvents ingest (deduplicated by `source` + `id`), retries with backoff, and never delays an acknowledgement. A best-effort cursor (`ns/<ns>/repos/<repo_id>/mirror.json`) replaces the comparison with the stream's last `loamsseq`: the hourly repair publishes whatever each WAL holds past its cursor. In-process consumers use `RepoSequencers::subscribe(ns, tenant, repo, from)`, resumable by seq.

## 5. The traits (D392)

All four live in `crates/loams-git` and are object-safe (`Arc<dyn …>`). On `wasm32` targets they use `async_trait(?Send)`, because Workers futures are not `Send` (§17).

> **Cross-reference, 2026-10-02 ([§42](42-cloudflare-2026-betas.md) D561, D562).** Cloudflare Artifacts (open beta; 1 GB per repository, 32 MB per blob) is a Git-level service and cannot implement `WalStore`, `RefLog` or `BlobStore`. The bucket WAL stays the default. An open adapter, `loams-git-artifacts`, mirrors commits to Artifacts and implements `Materializer` for agent workspaces; hosted use is private.

### 5.1 `WalStore`

```rust
/// Sequence numbers start at 1; Seq(0) is the empty log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Seq(pub u64);

pub struct WalBatch { pub batch_id: [u8; 16], pub events: Vec<CloudEvent> }   // loams_cloudevents::CloudEvent
pub struct WalSegment { pub seq: Seq, pub batch: WalBatch, pub created_unix_ms: i64 }
pub struct Hint { pub seq: Seq, pub checkpoint: Seq, pub written_unix_ms: i64 }

pub enum Appended {
    /// The batch owns `seq`, now or from an earlier attempt with the same batch id.
    Committed { seq: Seq },
    /// Another batch owns `seq`; the caller applies it and re-validates.
    Fenced { existing: WalSegment },
}

#[async_trait]
pub trait WalStore: Send + Sync + fmt::Debug {
    /// Writes `batch` as segment `expect_seq` with a create-only PUT.
    /// Precondition (caller's): segment `expect_seq - 1` exists or `expect_seq == Seq(1)`.
    /// Fenced: at most one batch is ever stored under a seq, across every writer.
    /// Idempotent: a retry with the same `batch_id` after an unknown outcome returns `Committed`.
    /// Errors: `TooLarge` (encoded body > 1 MiB), `Store(StoreError)` (retryable per StoreError::is_retryable).
    async fn append(&self, expect_seq: Seq, batch: &WalBatch) -> Result<Appended, WalError>;

    /// Segments `from, from+1, …` up to `max` of them, stopping at the first missing number.
    /// A CRC or magic mismatch is `Corrupt { seq }`, never a silent stop.
    async fn read_from(&self, from: Seq, max: usize) -> Result<Vec<WalSegment>, WalError>;

    /// The hint, or `Hint::empty()` if none was written. Never ahead of the true tail.
    async fn hint(&self) -> Result<Hint, WalError>;
    /// Best effort and coalesced by the caller (≤ 1 write/s); failures are logged, not returned to pushers.
    async fn publish_hint(&self, hint: &Hint) -> Result<(), WalError>;

    /// Create-only; an existing checkpoint at the same seq is `Ok` if its bytes are equal.
    async fn put_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), WalError>;
    /// The checkpoint with the greatest seq ≤ `at_or_below`, using the hint, else a LIST of `checkpoints/`.
    async fn latest_checkpoint(&self, at_or_below: Seq) -> Result<Option<Checkpoint>, WalError>;
}
```

**As built (GT1a, Task 7; `crates/loams-git/src/wal.rs`).** `StoreWalStore::new(store, paths)` stamps segments with a `Clock` (`SystemClock`, or `with_clock`). `append` encodes the segment once and PUTs it create-only. `AlreadyExists` and every transient error are resolved by reading the segment back: this batch's `batch_id` is `Committed`, another's is `Fenced`, and an absent segment is resent with the same bytes, for at most 5 PUTs before the last error is returned. A batch with an event whose `loamsseq` is not `expect_seq` is `WalError::Invalid`. `read_from` keeps up to 8 GETs in flight and returns them in order. `head` is `{"seq":…,"checkpoint":…,"written_unix_ms":…}`, overwritten unconditionally; an unreadable hint, or one whose checkpoint is above its seq, reads as `Hint::empty()`. `latest_checkpoint` reads the hinted checkpoint when the hint is not empty and is at or below the bound, else LISTs `checkpoints/` page by page; a checkpoint deleted between the LIST and the GET makes it list again. `put_checkpoint` is create-only up to 64 MiB, and above it a HEAD, a multipart upload and a read-back (no create-only multipart completion is available, so the last of two racing writers wins; GT1 ruling T7.4); an existing checkpoint at the seq is accepted when its bytes are equal **or when it differs only in its idempotency entries** (two writers prune the window at different times), and is `Corrupt` otherwise (GT1 ruling T7.2).

The draft's `append(expect_seq, record)` "idempotent on `idempotencykey`" (the draft's extension; the key is the CloudEvents `id` here, D415) is split in two: `WalStore` is idempotent on the batch (a store-level retry), and `RefLog` is idempotent on each transaction's key (a client-level retry, §4.5). A batch holds many keys, so the store cannot be idempotent per key without reading every segment.

### 5.2 `RefLog`

```rust
pub enum Expect { Oid(ObjectId), Absent, Any }
pub struct RefUpdate { pub name: RefName, pub expect: Expect, pub new: Option<ObjectId> }   // None = delete
pub struct RefTxn {
    pub key: IdemKey,
    pub request_digest: [u8; 32],
    pub updates: Vec<RefUpdate>,          // 1..=max_refs_per_txn (4096)
    pub packs: Vec<PackRef>,              // must already exist in the BlobStore
    pub principal: Principal,
    pub scope: Option<Scope>,             // write admission (D397); None = whole repository
    pub trace: Option<TraceParent>,
    pub push_options: Vec<String>,
}
pub struct Receipt { pub seq: Seq, pub index: u32, pub replayed: bool }
pub enum RejectReason { Stale { current: Option<ObjectId> }, Exists, Protected, OutOfScope { path: RepoPath }, InvalidName }
pub enum RefError {
    Rejected(Vec<(RefName, RejectReason)>),   // nothing applied
    IdempotencyMismatch,
    TooLarge,                                 // the transaction alone would exceed a segment
    Unavailable,                              // the sequencer is shutting down or lost its lease; retry
    Wal(WalError),
}
pub enum ReadAt { Latest, AtLeast(Seq), Exactly(Seq) }

#[async_trait]
pub trait RefLog: Send + Sync + fmt::Debug {
    /// Applies every update or none, at one sequence number, through group commit.
    /// Linearizable: the transaction takes effect at its segment's PUT, between call and return.
    /// Acknowledged only after the segment is durable.
    async fn commit(&self, txn: RefTxn) -> Result<Receipt, RefError>;

    /// The draft's `cas_ref(name, old, new)`: one update, through `commit`.
    async fn cas_ref(&self, name: RefName, old: Expect, new: Option<ObjectId>, key: IdemKey)
        -> Result<Receipt, RefError>;

    /// `Latest` is linearizable; `AtLeast(s)` reflects every commit up to `s`; `Exactly(s)` is retained 24 h.
    async fn snapshot(&self, at: ReadAt) -> Result<Arc<RefSnapshot>, RefError>;

    /// Committed transactions from `from`, in order, without gaps; resumable by seq.
    fn watch(&self, from: Seq) -> BoxStream<'static, Result<CommittedTxn, RefError>>;
}
```

`RefSnapshot` holds `seq`, the refs (`BTreeMap<RefName, ObjectId>`), symrefs, protections, the live pack set and the fork parent. Implementations: `BucketRefLog` (the sequencer of §4.4 over a `WalStore`, used by the gateway, the Durable Object and the GT1 helper in single-writer mode) and `MemRefLog` (tests).

**As built (GT1a, Task 10; `crates/loams-git/src/{reflog.rs,sequencer.rs,testing.rs}`).** `BucketRefLog::open(wal, blobs, config, clock)` loads the latest checkpoint (by the hint, else a listing; a repository with none is `RefError::NotFound`) and every segment after it, then starts one sequencer task; clones share it, and `shutdown` lets the group in flight finish, answers queued transactions `Unavailable` and writes the hint once more. `commit` refuses a transaction with no updates or more than 4,096 (`RefError::Invalid`), one whose event alone is over a segment (`TooLarge`), and one whose packs are not all in the `BlobStore` (a HEAD each, before queueing). The loop takes a group by count and encoded size, reads the clock **once** per attempt and uses that time for every check, every `apply_txn` and every event's `time`, so replaying the segment rebuilds identical idempotency entries. Replays and mismatches are answered at once (a key in the state was committed whatever the tail), except that a second transaction under a key accepted earlier in the same attempt waits for that attempt's outcome and is then decided against committed state; **a rejection is answered only once the state it was decided on is known to be the tail**: after the group's segment commits, or, for a group of rejections only, after one GET finds no segment past the state. A sequencer whose state is behind another writer's otherwise refuses updates that apply at the tail, which the two-sequencer linearizability test found. A fence applies the foreign segment and every later one and decides the whole group again in arrival order, at most 16 fences per transaction; an append whose outcome the WAL could not learn answers `Unavailable`, and a retry with the same key replays if the batch landed. Reads never wait for the loop: they start from the state it last published and read forward until a segment is missing (`Exactly(s)` below that state replays from the latest checkpoint at or below `s`). `watch(from)` reads the WAL in order and takes the sequencer's own segments directly when they are the next, polling every second for other writers; a seq the sequencer had applied and the WAL no longer holds ends the stream with `Invalid`. `cas_ref` commits as principal `loams-cas-ref` with a digest over the update. `RefError` adds `Invalid`, `NotFound` and `State` to §5.2's variants. The sequencer reports `GitEvent::{Committed { seq, txns }, Fenced { at }, GroupSize(n)}` on an in-process broadcast.

### 5.3 `BlobStore`

```rust
/// Content-derived names: a pack checksum (`packs/<hex>.lpk`) or, from GT4, a scope cache chunk.
pub struct BlobId(String);
pub enum Put { Created, Existed }

#[async_trait]
pub trait BlobStore: Send + Sync + fmt::Debug {
    /// Create-only. If the name exists, returns `Existed` after checking that the stored length matches
    /// (`Mismatch` otherwise: two contents under one content-derived name is corruption).
    async fn put(&self, id: &BlobId, bytes: Bytes) -> Result<Put, BlobError>;
    /// Same, as a multipart upload for bodies over 64 MiB; the parts are buffered in an `Fs` (§17).
    async fn put_stream(&self, id: &BlobId, body: BoxStream<'static, io::Result<Bytes>>, len: u64)
        -> Result<Put, BlobError>;
    /// `range` must lie inside the blob; reads go through the H1 cache (§04) where one exists.
    async fn get_range(&self, id: &BlobId, range: Range<u64>) -> Result<Bytes, BlobError>;
    async fn head(&self, id: &BlobId) -> Result<Option<u64>, BlobError>;
}
```

**As built (GT1a, Task 6; `crates/loams-git/src/blob.rs`).** `BlobId` is the pack checksum in lowercase hex (40 or 64 digits) and names `packs/<id>.lpk`. A blob of at most 64 MiB is a `put_if_absent`; an existing name answers `Existed` when the stored length (a HEAD) equals the offered one, else `Mismatch`. A larger blob is one unconditional multipart upload after a HEAD finds no blob of that name; `put_stream` first spills the body to a `SpillDir` on the configured `Fs` (memory by default, at most 256 MiB per body), checks it is exactly `len` bytes (`LengthMismatch`), and uploads from the spill file, so a failed upload is retried without the caller's stream. Transient store errors (`StoreError::Backend`) are retried three times after jittered pauses of 50, 200 and 800 ms; a create-only PUT whose answer was lost finds its name on the retry and answers `Existed`. `get_range` is one ranged GET in the common case, and answers `OutOfRange` (with the blob's length) for any range that does not lie inside the blob, an empty one included. `LpkWriter::from_parts` checks that the idx is a version 2 idx of the pack (the object count and the pack checksum in the idx trailer) and builds `pack ‖ idx ‖ footer` and its `PackRef`; `read_idx` and `read_pack` are one range read each.

There is no `delete` on the trait: only GC deletes, through `Store`, after the grace period and a GC claim (§03 §7). Objects are located by an `Odb` built on `BlobStore`: pack idx sections (cached in H0/H1) or the compaction's multi-pack index map an object id to `(pack, offset)`, and gitoxide's `gix-pack` resolves deltas from range reads.

**As built (GT1a, Task 9; `crates/loams-git/src/odb/`).** `ObjectDb` has `contains`, `read` (kind and bytes, deltas resolved; `NotFound` when absent) and `missing_from_closure(tips, known, extra)`. `PackCacheOdb::open(repo, blobs, cache, snapshot, parent)` looks an object up in the snapshot's packs, newest first, then in `parent`, which the caller opens on the parent's blob store at the parent's seq; it refuses a parent the snapshot does not name, a missing one, another repository or one at another seq (`ParentMismatch`), and more than `MAX_FORK_DEPTH` (8) parents (`ForkTooDeep`). `PackCache::new(fs, max_bytes)` keeps whole sections as `<checksum>.idx` and `<checksum>.pack` files at the root of its `Fs`, least recently used out first; a file is checked against its `PackRef` (length and the pack checksum in its trailer) on every read, so a deleted or damaged cache costs only downloads, and files left by an earlier process are counted on first use. A lookup downloads only idx sections; a pack's data is downloaded only when it holds the object. `gix-pack` (0.75.0, the 0.88 train) decodes entries from memory and resolves both delta kinds inside the pack (stored packs are never thin). `missing_from_closure(tips, known, extra)` walks commits, trees and tags breadth first from `tips`, asks `extra` (the pushed pack) before the repository, checks a blob named by a tree for existence only, skips submodule commits, and treats the objects in `known` (the current ref tips) as already seen, so it never enters their history; trees and blobs a new commit shares with them are still visited, so a push costs the commits it adds and the trees they reach. A pushed pack read through the cache keeps its parsed sections for the whole walk, as the repository does. It does not stop merely because an object is stored: a stored pack may hold unreferenced objects whose closure was never checked.

### 5.4 `Materializer` (implemented in GT4)

```rust
/// Cone-mode sparse-checkout paths ("src/api", "docs"); empty = the whole tree.
pub struct Scope { pub repo: RepoId, pub commit: ObjectId, pub cones: Vec<RepoPath> }
pub struct AgentId(pub String);
pub struct Touched { pub tree: Arc<TreeManifest>, pub fetched_blobs: u64, pub fetched_bytes: u64, pub cached_blobs: u64 }

#[async_trait]
pub trait Materializer: Send + Sync + fmt::Debug {
    /// Fetches every tree of `scope.commit` (the full path listing) and the blobs inside `scope.cones`
    /// that `agent`'s cache lacks. Idempotent. Reads are grouped by pack and coalesced into ranges of
    /// at least 1 MiB: never one GET per blob (§15 §12).
    async fn touch(&self, scope: &Scope, agent: &AgentId) -> Result<Touched, MatError>;
    /// One blob, fetched on demand; `OutOfScope` for a path outside the cones.
    async fn read(&self, scope: &Scope, agent: &AgentId, path: &RepoPath) -> Result<Bytes, MatError>;
}
```

The agent cache is an `Fs` (§17): `NativeFs` on a sandbox host's NVMe (other backends belong to a commercial Cloudflare target (`loams-platform`)). It is shared by every agent on a host (§15 principle 4); the `agent` argument scopes metering and quotas, not storage.

## 6. Access paths

### 6.1 Smart HTTP (D394, GT2)

Routes in the `gateway` role, loopback-only until the unified auth plan (D111, Q30; Q389):

| Route | Protocol |
|---|---|
| `GET /git/<ns>/<repo>.git/info/refs?service=git-upload-pack` with `Git-Protocol: version=2` | v2 capability advertisement: `agent=loams/<version>`, `ls-refs=unborn`, `fetch=shallow wait-for-done filter`, `server-option`, `object-format=sha1`, `object-info` |
| `POST /git/<ns>/<repo>.git/git-upload-pack` | v2 commands `ls-refs`, `fetch`, `object-info` (gitprotocol-v2) |
| `GET …/info/refs?service=git-receive-pack` | v0 advertisement with `report-status report-status-v2 delete-refs side-band-64k quiet atomic ofs-delta push-options object-format=sha1 agent=loams/<version>` |
| `POST …/git-receive-pack` | commands, then the pack (gitprotocol-pack, gitprotocol-http) |

HTTP/2 is accepted where the client negotiates it (git over libcurl with `http.version=HTTP/2`). `packfile-uris` (pre-signed bucket URLs for whole-pack reuse on clone) is a GT2 option, worthwhile on R2 because R2 egress is free.

**Fetch.** Negotiation walks commits with a commit graph (the compaction's `commit-graph`, or one built on demand and cached in H2). The pack is assembled from stored entries with `gix-pack`'s output pipeline, reusing deltas whose base is also sent ("pack copy" mode), and whole stored packs are streamed when the wants cover them. `filter=blob:none` and `tree:0` drop blobs or trees. `shallow`/`deepen` follow gitprotocol-v2.

**Push.** §6.3.

**Why gitoxide primitives and not `git upload-pack` on every request.** The serving path must run on nodes with no local repository (and stay portable to Workers, for a commercial Cloudflare target (`loams-platform`)), with object reads that go through range GETs and the H1 cache. `gix-pack` builds for `wasm32-unknown-unknown` in gitoxide's CI; the `git` binary does not run in Workers. gitoxide's server-side upload-pack/receive-pack plumbing is still unchecked in its `crate-status.md` (read 2026-10-01), as §15 §3.4 found, so the loop is Loams’. Pack *encoding* without delta compression and *bitmap writing* are also unchecked there, so repacking uses stock `git` (D396), which is GPL-2.0 and runs as an unmodified separate process, never linked, as WeSQL (D148) and PgDog (D236) do.

### 6.2 `git-remote-loams` (D395)

| Phase | Address | Capabilities | What it does |
|---|---|---|---|
| GT1 | `loams::s3://bucket/prefix/ns/<ns>/repos/<repo_id>` (any `object_store` URL; git's `<transport>::<address>` form runs `git-remote-loams`) | `fetch`, `push`, `option` | **Serverless**: lists refs from `RefLog::snapshot(Latest)`; fetches the live packs the local repository lacks and writes each with its stored index into `.git/objects/pack/`; pushes by running `git pack-objects` for the refspecs, writing one `.lpk`, and committing a `RefTxn` through its own `BucketRefLog` (the client is its own sequencer, fenced by §4.2) |
| GT2 | adds `loams://<host>/<ns>/<repo>` | adds `stateless-connect` | Tunnels protocol v2 to the server (or to an in-process upload-pack over the bucket for `loams::` addresses), so `--filter=blob:none`, sparse checkout and lazy fetch of promisor objects work. Whether git drives lazy fetch through a helper's `stateless-connect` (documented as "experimental; for internal use only") is checked first (Q386) |

Credentials: the `object_store` environment (`AWS_*`, `GOOGLE_*`, `AZURE_*`), or a vended token from `loams` (§15 §8). The helper links only Apache-2.0/MIT code and runs `git` subcommands as processes, as every remote helper does.

**As built (GT1g, Task 51's first commit; `crates/loams-git-remote`, plan rulings T51.1–T51.8).** `git clone loams::file:///srv/loams/ns/1/repos/web` works with stock git: the address's last segment is a repository id, or a name the catalog (`_names/`) maps to one; a repository being deleted is not found. `list` and `fetch` read the tail with `latest_state` (the latest checkpoint and the segments after it) and write nothing, so read-only credentials can clone and fetch. `fetch` downloads every live pack the local repository lacks, the fork parents' at their fork points included, in 16 MiB range reads, checks each pack's trailer against its name, and installs `pack-<checksum>.pack` and then its `.idx`; the checksums go to `.git/loams/<remote>/packs`, as does the pusher's own pack. `push` checks fast-forwards locally (a forced fast-forward is still an ordinary update, so `deny_force` does not refuse it; a tag that exists never moves without force), runs `git pack-objects --revs --thin` into `git index-pack --fix-thin` once for all refspecs, uploads one `.lpk`, and commits one ref transaction (`--atomic`) or one per ref through a `BucketRefLog` it opens for the push and shuts down after it. A commit answered `Unavailable` is repeated with the same key (at most five times), so a commit whose answer was lost is replayed, never applied twice. The idempotency key is `git-push:` and SHA-256 over the principal and the request digest, which is SHA-256 over the pack checksum and the sorted (name, expect, new) triples. The principal is the S3 access key id or the Azure storage account of the credentials in use, `local:<user>` for a local directory, and `git config user.email` only under `LOAMS_GIT_UNAUTHENTICATED_TESTS=1`. A push to a missing repository fails with `fatal: repository '<address>' not found` unless `LOAMS_GIT_CREATE_ON_PUSH=1`, which writes checkpoint 0 with `HEAD` on the first branch pushed. Not yet: `stateless-connect` and `loams://` (Task 51's second commit, after Tasks 46–49), `--force-with-lease` (`option cas` is answered `unsupported`), and streaming a push larger than memory (the helper reads its pack whole; the receive path streams).

### 6.3 The receive path (GT2; the helper's push in GT1 uses the same steps)

1. Parse the commands (old, new, ref) and capabilities. Refuse more than `max_refs_per_push` (4096) or a pack over `max_pack_bytes` (2 GiB, configurable).
2. Stream the pack into a spill file in the node's `Fs`, indexing it with `gix-pack` as it arrives (memory bounded by the index, not the pack). Resolve thin-pack bases from the repository's pack set at the snapshot the advertisement came from.
3. Verify: every object hashes to its id; every new tip's closure is present in the new pack plus the pack set (connectivity); fast-forward unless the update is forced and allowed; ref names valid; protections; the token's scope (D397).
4. PUT `packs/<checksum>.lpk` (create-only, D390). An empty push (deletes only) has no pack.
5. Queue a `RefTxn`: one for an `atomic` push, else one per ref, all in the same group, so each ref gets its own status.
6. Answer `report-status`/`report-status-v2` after the segment commits, with the sequence number in a `loams-seq=<n>` progress line on side-band 2.

A pack older than the GC grace period (1 h, §03 §7) at commit is refused (`Stale`), so a long push cannot rely on a pack that compaction retired meanwhile.

### 6.4 `loams-vfs` and per-folder scopes (D397, GT4, not yet planned)

> **Superseded (proposed) by §19 (D826–D834).** `loams-vfs` is no longer a later phase over an agent's local overlay: it is the primary client, and the workspace (base, overlay, snapshots) is a server-side record that any machine can mount. Scopes (cones carried in the token, `EACCES` outside them, `OutOfScope` write admission on the server) and "one WAL per repository" carry over unchanged (§19.11). The text below is kept for the record.

`loams-vfs` is the lazy lower layer of §15 §5.1's `/workspace`, not a second filesystem design:

- **Mount.** A FUSE (or virtiofs, §15 Q3) filesystem over a `Materializer`: the tree of the agent's commit is listed at mount, blobs are fetched on first read or write, and writes go to the local upper layer (overlay).
- **Scopes.** The agent's vended token (§15 §8) names its cones. Paths outside the cones are listed but unreadable (`EACCES`), and the receive path and `RefLog` refuse a commit that changes a path outside them (`OutOfScope`). This is write admission for agents, enforced on the server, not trust in the client.
- **Commit.** A checkpoint (§15 §9) hashes the upper layer into blobs and trees, writes one `.lpk` with the new objects and commits one `RefTxn` on the agent's branch. A commit that touches several folders is still one WAL record and one sequence number: the draft's "cross-folder commits use one head-pointer swap" holds with no head pointer.
- **Contention.** Agents work on their own branches (§15 principle 2: one writer per workspace), so they contend only in the sequencer, which group-commits them. The draft's per-scope WAL partitions would split one repository's ordering, and with it atomic cross-folder commits, to remove a contention point that group commit already removes. They are not built until GT4 measures the sequencer as the bottleneck (Q391). A server-side merge of disjoint-path commits into a shared branch (a merge queue) is a separate question (Q392).

## 7. Compaction and GC (D396)

| Job | When | How |
|---|---|---|
| **Checkpoint** | every 256 segments, or when replay since the last checkpoint exceeds 8 MiB | The sequencer writes `checkpoints/<seq>.lgc` from its state; the hint names it |
| **Repack** | more than 16 packs above the geometric progression (git's `--geometric=2`), or on demand | A worker task under the lease `task/git-compact/<ns>/<repo_id>` (D30's pattern) hydrates a local mirror (the `.lpk` files split into pack and idx), runs `git repack --geometric=2 -d --write-midx --write-bitmap-index` and `git commit-graph write --reachable`, uploads the new packs as `.lpk`, the `midx/` and `commit-graph/` objects, then commits a `PackSetChange` through the sequencer. Readers see the old or the new pack set, never a mix |
| **Segment GC** | daily | Segments below the oldest checkpoint still needed for `Exactly` reads (24 h) are deleted; segments above a gap after the grace period |
| **Pack GC** | daily | A pack removed by a `PackSetChange` is deleted once no checkpoint within retention and no fork at a seq that still names it references it, after the grace period, with a GC claim (§03 §7, D59) |

Only one compactor runs per repository, and never on the push path. The sequencer's owner schedules it as a worker task (§09), because repacking is CPU-heavy (a commercial Cloudflare target (`loams-platform`) runs it in a container). Continuity reports compaction as its bottleneck at the top rate (§3), so GT2 measures repack time per pushed MiB.

**As built (GT1a, Task 11; `crates/loams-git/src/{checkpointer.rs,fork.rs,gc.rs}`).** `create_repo(wal, format)` writes checkpoint 0 (create-only; SHA-1 only). The sequencer counts segments and encoded event bytes since its last checkpoint (`CheckpointPolicy`, 256 / 8 MiB, in `SequencerConfig`) and, after the receipts of the commit that reaches the policy, starts one checkpoint write beside the loop (at most one in flight); the hint names it from the next commit, and `shutdown` waits for it. `state_at(wal, seq)` is the latest checkpoint at or below `seq` replayed to it. `fork_repo(src, dst_wal, src_repo, at)` is one create-only PUT of the fork's checkpoint 0 (the source's refs, symbolic refs and protections, no packs, `parent = {src, seq}`); `open_fork_odb(store, ns, repo, snapshot, cache)` reads each parent's state at the seq its child names and opens the chain, refusing more than 8 parents. **Segment GC** (`plan_segment_gc`, `run_segment_gc`): `keep_from` is the newest checkpoint at or below the state that was current at the `EXACTLY_RETENTION` cutoff (the state just before the first segment written after it; the newest checkpoint when no segment is that young), never above the checkpoint the hint names (GT1 ruling T7.3: readers trust it), and never above the newest checkpoint at or below the oldest live fork point, which the caller passes (a fork reads its parent at the fork point for as long as it lives; the fork index arrives with Task 54). `run_segment_gc` reads the hint again and refuses a plan the hint has moved below. Segments at or below `keep_from`, checkpoints below it, and segments above a gap are deleted once their last-modified time is older than `GC_GRACE`. Deleting old checkpoints makes a hint that names a deleted one harmless: the reader falls back to listing.

## 8. Build cache (D398, GT3)

### 8.1 Tools

| Tool | Licence, version | Verdict |
|---|---|---|
| **sccache** | Apache-2.0, v0.18.0 (2026-09-14) | **Primary.** Wraps C/C++, Rust and CUDA compilers. Backends: local disk, S3, R2, Redis, Memcached, GCS, Azure, GitHub Actions, WebDAV, Alibaba OSS, Tencent COS. A multi-level cache (`SCCACHE_MULTILEVEL_CHAIN`, docs/MultiLevel.md, added 2026-04-17, first released in v0.16.0 (verify)); read-only modes per backend (`SCCACHE_S3_RW_MODE`, `SCCACHE_WEBDAV_RW_MODE`, `SCCACHE_LOCAL_RW_MODE`, `rw_mode`); `SCCACHE_BASEDIRS` since v0.14.0 (source) |
| **BuildCache** | zlib, v0.33.1 (2026-09-23; moved to gitlab.com/bits-n-bites/buildcache) | Optional. C/C++ and rustc; local cache plus an `http`, `redis` or `s3` second level. Overlaps sccache; adopt only if a toolchain sccache cannot handle needs it |
| **mozilla-actions/sccache-action** | Apache-2.0, v0.0.11 | Wires sccache to GitHub's cache on GitHub-hosted runners only; Loams’ endpoint replaces it elsewhere |
| bazel-remote, REAPI | Apache-2.0 | §15 §7 stands: bazel-remote on the bucket now, Loams’ REAPI CAS in W3 |

Known sccache limits, from its README: crates that invoke the linker (`bin`, `dylib`, `cdylib`, `proc-macro`) and incrementally compiled crates are not cached; absolute paths must match unless `SCCACHE_BASEDIRS` is set.

### 8.2 Two ways in

| Path | sccache config | Loams code on the path | What it gives |
|---|---|---|---|
| **Direct** | S3 backend: `SCCACHE_BUCKET`, `SCCACHE_ENDPOINT` (R2: `https://<account>.r2.cloudflarestorage.com`, region `auto`), `SCCACHE_S3_KEY_PREFIX=ns/<ns>/cache/sccache/<repo>/trusted/`, vended credentials; `SCCACHE_S3_RW_MODE=READ_ONLY` for untrusted builds | none (§15 §7's "no Loams code" path) | Isolation by credential scope: R2 temporary credentials take `prefixes` and `object-read-only`, up to 7 days (Cloudflare API, read 2026-10-01); RustFS and S3 through `ObjectStoreProvider::issue_credentials` (§25 §5). Eviction by age since write only |
| **Gateway** | WebDAV backend: `SCCACHE_WEBDAV_ENDPOINT=https://<gateway>/cache/sccache/<ns>/<repo>/`, a token | `loams-buildcache` in the `gateway` role | Hit, miss and put counts (§11); quotas; trust enforced on the server; **approximate LRU**: a hit on an entry older than `refresh_after` (7 days) rewrites its timestamp with an in-place copy, at most one copy per entry per week. The WebDAV subset sccache needs is checked first (Q394) |

CI templates (GT3) set `RUSTC_WRAPPER=sccache`, `CARGO_INCREMENTAL=0`, `SCCACHE_BASEDIRS=<workspace root>` and either path's variables. With `SCCACHE_MULTILEVEL_CHAIN=disk,webdav` (or `disk,s3`), a local disk cache sits in front, and on Cloudflare R2 is the edge level; a regional S3 level is a third link only where a deployment has one.

### 8.3 Trust model

**Never share a writable cache between untrusted builds.** A poisoned entry is a compiled object that any later build links.

| Trust class | Who | Reads | Writes |
|---|---|---|---|
| `trusted` | CI on protected branches of the repository; release builds | `trusted/` | `trusted/` |
| `untrusted` | fork and pull-request builds, agent sandboxes, local developer machines | `trusted/` | nothing, or `scratch/<principal>/` when the repository enables it; scratch is never read by another principal |

Keys are `ns/<ns>/cache/sccache/<repo>/<class>/<sccache key>`: per tenant and per repository, so no cross-tenant reads exist to poison, and dedup across repositories is given up deliberately (§15 principle 5). The class comes from the credential: the vending flow (§15 §8) maps a CI job's identity (for example a GitHub OIDC token's `ref` and `repository` claims) to a class. Eviction: a TTL (default 30 days since write or refresh) and a size quota per repository (default 50 GiB), enforced by a sweeper that deletes the oldest entries first.

## 9. Package mirror (D399, GT3)

The crates.io part of §15 §6's registry proxy; PyPI, npm and Go stay in W2.

- **Routes** (gateway role): `GET /registry/crates/<ns>/index/config.json`, `GET /registry/crates/<ns>/index/<path>` (the sparse layout: `1/`, `2/`, `3/<c>/`, `<ab>/<cd>/<name>`), `GET /registry/crates/<ns>/dl/<crate>/<version>/<cksum>`.
- **`config.json`:** `{"dl": "https://<gateway>/registry/crates/<ns>/dl/{crate}/{version}/{sha256-checksum}", "auth-required": true}` and no `api` (no publishing). The `{sha256-checksum}` marker lets downloads go straight to content-addressed storage.
- **Index files** are fetched from `https://index.crates.io/<path>` with the cached ETag (`If-None-Match`), cached under `ns/_public/packages/crates/index/<path>` with a freshness TTL (60 s), and filtered per namespace at serve time.
- **`.crate` files** are fetched once from `static.crates.io`, checked against the index's `cksum` (SHA-256), and stored create-only at `ns/_public/packages/crates/sha256/<cksum>`. The designated public-packages namespace is §15 principle 5's exception to per-namespace dedup.
- **Policy** (§15 §6): allowlists and denylists, version pinning, a quarantine window that hides versions younger than N days (needs a publish time per version: the index's `pubtime` field where present (verify), else the crates.io API), and an audit record per download to the stream `_packages`.
- **Where state lives.** Index files and crates are objects; policy is namespace configuration. The draft's "index in a Durable Object or D1" is not needed: the object store plus H1 serve the index (D399).
- **Buy vs build.** kellnr (Apache-2.0, v6.9.0, 2026-09-23) is a private registry; panamax (Apache-2.0, last release 2024-06-06) and ktra (Apache-2.0) are mirrors or registries with their own storage. None runs in Workers or stores into a Loams namespace, and the proxy is three routes, so it is built; kellnr is the documented choice for a self-hosted private registry with publishing (GT3 Task 0 checks its crates.io proxy and S3 storage).

## 10. Security

- **Tenant from the credential.** The namespace comes from the token, never from the URL alone (D182's rule); a URL naming another namespace is `404`.
- **Scoped, short-lived tokens** (§15 §8): `(namespace, repo, branch prefix, scope cones, cache class, registry read)`.
- **Pack limits** (§6.3) and fsck-light verification bound what a push can store; malformed packs never reach the WAL.
- **No ambient bucket credentials** in sandboxes: agents reach Git, the cache and the mirror through the gateway, or hold vended prefix credentials.
- **Cache poisoning**: §8.3. **Supply chain**: §9's quarantine and audit.

**As built (GT1b, Task 16; `crates/loams-git/src/auth.rs`, `crates/loams/src/api/git_auth.rs`, plan rulings T16.0–T16.5).** A `GitGrant` names one namespace, the repositories, an access level (`read` < `write` < `admin`), branch prefixes, cones, workspaces, environments (none unless named), a cache class and registry access. Stock git presents a git token on `--git-listen` (loopback only until MT1) as the HTTP Basic password or a Bearer token; the token file (`--git-tokens`) holds SHA-256 hashes and their grants. The namespace comes from the token: a URL naming another namespace, an uncovered repository and a missing one answer the same `404`. On the main port, `RepoService` callers are authorized by their org role: reads need `member`, administration `admin`, and the operator always passes. Pushes are checked with `GitGrant::check_push` (write access and branch prefixes) on the receive paths (Tasks 27, 50).

## 11. Usage hooks (D393)

Exported per §27 (Prometheus, or OTLP delta metrics for per-repository cardinality, Q-UH-1), labels `org` and `namespace`:

| Family | Meaning |
|---|---|
| `loams_git_pushes_total{result}` | committed, rejected, replayed |
| `loams_git_received_bytes_total`, `loams_git_sent_bytes_total` | pack bytes in and out |
| `loams_git_stored_bytes` | live pack set, per namespace (gauge, from checkpoints) |
| `loams_git_wal_segments_total`, `loams_git_group_txns` (histogram) | sequencer activity; group size |
| `loams_git_cpu_seconds_total{op}` | `receive`, `upload`, `compact`, from the thread CPU clock around each request (as §27 §3.3 measures T1) |
| `loams_buildcache_requests_total{result}` | `hit`, `miss`, `put`, `denied` (gateway path only) |
| `loams_buildcache_bytes_total{direction}`, `loams_buildcache_stored_bytes` | |
| `loams_packages_requests_total{ecosystem,source}` | `cache`, `upstream`, `denied` |
| `loams_packages_bytes_total{ecosystem}` | |

Storage bytes for the direct cache path come from the namespace's logical-bytes accounting (D103). Turning these into invoices is `loams-platform`'s job (D190, D202).

## 12. Cost sketch (estimate)

R2 prices read 2026-10-01: storage $0.015/GB-month, Class A $4.50 per million, Class B $0.36 per million, no egress fee. One push with a pack costs one `.lpk` PUT plus its share of a segment PUT (1/k at group size k) and a hint write at most once a second: **about $5 per million pushes** on R2, before storage. A clone of a compacted repository is a handful of GETs plus egress, which R2 does not charge. On S3 Standard the same push costs about $5 per million (PUT $0.005 per 1,000, from a search summary; verify) plus egress.

## 13. Phases

> **Replaced (proposed) by §19.12.** The 2026-10-01 GT1–GT3 plans were superseded by the 2026-10-10 GT1 production plan, which is rewritten for monorepo mode; GT4 is folded into it, and track GT starts now (D825, superseding D411). GT5 is unchanged. The table below is kept for the record.

| Phase | Scope | Exit gate | Plan |
|---|---|---|---|
| **GT1** | `loams-git`: formats, `BlobStore`, `WalStore`, `Odb`, `BucketRefLog` with group commit and idempotency, checkpoints, forks; `git-remote-loams` (serverless `fetch`/`push`) | Stock git clones, fetches and pushes through `loams::` on RustFS and in-memory; 8 concurrent pushers never lose an acknowledged push; the linearizability checker passes RefLog histories under store faults; pushes/s and push latency measured on RustFS, R2 and S3 | [GT1](../plans/2026-10-01-gt1-wal-git-core.md) |
| **GT2** | Smart HTTP v2 upload-pack, v0/v1 receive-pack, the receive path, negotiation and pack assembly, compaction with stock git, `stateless-connect` in the helper (partial clone, lazy fetch) | §15 W1's client matrix (git, gitoxide, libgit2, JGit): clone, fetch, push, partial clone, shallow; differential tests against `git upload-pack` on the same objects | [GT2](../plans/2026-10-01-gt2-smart-http.md) |
| **GT3** | `loams-buildcache` (WebDAV subset, trust classes, refresh-on-hit, sweeper), the direct-path credential recipe, CI templates; `loams-registry` crates mirror | A cold and a warm `cargo build` of a fixture workspace through each path (the Loams workspace nightly), with the warm hit rate reported; an untrusted build cannot write `trusted/`; `cargo fetch` of the workspace through the mirror with egress limited to Loams | [GT3](../plans/2026-10-01-gt3-build-cache-and-mirror.md) |
| **GT4** | `loams-vfs`, `Materializer`, scopes and write admission (§6.4) | Not yet planned | — |
| **GT5** | Continuity-style NVMe replica caches, gossip as a hint, any-node writes, compaction at scale | Not yet planned | — |

GT1–GT2 are §15 W1's repository scope; GT3 is W2's sccache wiring and the crates part of the registry proxy; GT4 is W2's lazy workspace mount. §15 §13 places W1 after M3. Starting GT1 earlier, as its own track beside M and R like tracks R, D and J, was an owner decision (Q395); on 2026-10-02 the owner kept §15's slot, so GT1 starts with W1 after M3 (D411).

## 14. Risks

| # | Risk | Mitigation |
|---|---|---|
| 1 | **The server-side Smart HTTP effort** is larger than planned (negotiation, shallow, filters, thin packs, every client's quirks) | v2-only upload-pack first; stock `git upload-pack` as the differential oracle; go-git's server as a reference; the client matrix as the gate |
| 2 | **Small-object request costs** on S3 (Class A $5 per million) | One PUT per push for data (`.lpk`), one PUT per group for the WAL, hints coalesced; request counts per operation tracked in CI (§12 §2 item 11: cost regressions are bugs) |
| 3 | **Pack GC and repacking** bugs lose objects | Removal only through `PackSetChange` after the replacement pack is durable; deletion only after grace with GC claims; a nightly `git fsck` of mirrors of sampled repositories |
| 4 | **gitoxide memory in Workers** (128 MB per isolate) | Relevant only to a commercial Cloudflare target (`loams-platform`), which measures it |
| 5 | **Cache poisoning** | §8.3 trust classes, enforced by credential scope or the gateway, never by client configuration |
| 6 | **Unverified third-party throughput** (Continuity's 120 and 300 pushes/s are synthetic) | Loams’ own target (30/s) is measured in GT1 on each store |
| 7 | **Segment PUT latency** on R2 or S3 Standard is too high for 30 pushes/s at small group sizes | Group commit grows with load; S3 Express One Zone as an `express`-like class for hot repositories (§02 §2), if its conditional create is confirmed (Q384) |
| 8 | **A direct-writing `git-remote-loams` client** and the server race on one repository | Both are fenced by §4.2; the server re-reads and re-validates; tested in GT1 Task 9 |
| 9 | **Stock git as a process dependency** (GPL-2.0) | Never linked or vendored; invoked as a binary only in the repack worker and tests; recorded in the licence check like PgDog (D236) |
| 10 | **gitoxide API churn** (0.x releases every few weeks) | Pin `gix-*` versions; the core touches gix only behind `Odb` and the protocol module |

## 15. Conflicts with existing decisions, and how they are resolved

| Earlier | The draft or this document | Resolution |
|---|---|---|
| §15 §3.1 (approved): one CAS'd `refs` document per repository | D388, D389: WAL segments plus checkpoints | **Amends §15 §3.1–§3.2** (marked there). The single document is the special case "a checkpoint after every segment"; forks and alternates are unchanged. Needs the owner's approval because §15 is approved |
| §15 §3.1: `packs/<ulid>.pack` + `.idx` | D390: `packs/<checksum>.lpk` | Amends §15 §3.1: one PUT per push and idempotent retries |
| The draft §14.3: "head pointer advances via conditional write" | D389: the segment create is the commit point | R2's one-write-per-second-per-key limit and one fewer PUT per batch |
| The draft §14.4: `append` idempotent on `idempotencykey` | D392: batch-level idempotency in `WalStore`, key-level in `RefLog` | Same guarantee, at the layer that can check it |
| The draft §14.5: per-scope WAL partitions | D397: one WAL per repository; scopes are read and write admission | §15 principle 2 (one writer per workspace) and atomic cross-folder commits; Q391 reopens it on measurement |
| The draft §14.7: metering CloudEvents feed the ledger | D393: §27 hooks only | D190, D200–D202: billing is in `loams-platform` |
| The draft §13.3: package index "in a Durable Object or D1" | D399: in the object store | D1 (object storage is the source of truth); fewer services |
| §15 §7: sccache with "no Loams code" | D398 adds an optional gateway path | The direct path stays code-free; the gateway path is for metering, LRU and server-enforced trust |
| D11: no copyleft dependencies | D394, D396: stock `git` (GPL-2.0) | An unmodified separate process, never linked, as D148 (WeSQL) and D236 (PgDog) |
| §15 §13: W1 after M3 | GT1–GT3 could start earlier | §15's slot kept (Q395, D411) |
| Q15: repos as a sixth object kind | D388: a service with its own bucket WAL | Proposed answer to Q15; a repo's WAL is not a Loams stream, and its events are mirrored into one (§4.8) |
| §15 §16 Q1: protocol v2 only? | D394 | Answered: v2 upload-pack, v0/v1 receive-pack, v0 upload-pack only if a matrix client needs it |
| The draft §2/§3/§4.3: Resonate on TiDB | Not used by Git | D260/D261: Resonate on TiKV; Git needs no durable-execution store |
| The draft §6: event types `io.loams.<domain>.<name>.v1` | `io.loams.dev.git.*.v1` | The owner's ruling of 2026-10-01: the prefix is `io.loams.dev.<domain>.<name>.v1` |

## 16. Open questions

> **2026-10-10.** Q384–Q395 stay resolved, except that Q395's answer (keep §15's slot, D411) is reversed by the owner's direction of 2026-10-10 (D825). Monorepo mode's open questions are §19.15 (Q750–Q756).

| # | Question | Owner | Needed by |
|---|---|---|---|
| Q384 | ~~Segment PUT latency (p50, p99) on R2, S3 Standard, S3 Express One Zone and RustFS, and whether S3 Express directory buckets honour `If-None-Match: *` on PutObject (AWS documents conditional writes without excluding them; not confirmed)~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision: GT1 Task 10 measures it; S3 Express is used only if its conditional create is confirmed | Eng | Resolved |
| Q385 | ~~RustFS conditional PUT above 1 MiB: does a create-only PUT of a large `.lpk` stay atomic, or can a reader see a partial object (D390 relies only on equal content, but a torn read must be impossible)~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision: GT1 Task 3 checks it; until then RustFS refuses content-named blobs over 64 MiB (GT1 Ruling 1). **Checked 2026-10-11 (GT1 Task 6, ruling T6.2):** on `rustfs/rustfs:1.0.0`, a reader polling 10 multipart uploads of 80 MiB saw no partial object in 4,004 polls, so RustFS is treated as atomic and the refusal stays only as `StoreBlobStore::with_atomic_multipart(false)` for stores not known to complete multipart uploads atomically | Eng | Resolved |
| Q386 | ~~Does current git (2.5x) drive partial clone and lazy fetch of promisor objects through a remote helper's `stateless-connect`, or only through native transports~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision: GT2 Task 0 checks it; `stateless-connect` is the design, and the result goes into GT2 | Eng | Resolved |
| Q387 | ~~Do libgit2 and JGit speak protocol v2 for fetch, or does W1's client matrix require a v0/v1 upload-pack~~ Answered 2026-10-02 by the owner: the recommended default — upload-pack v2 only, unless GT2 Task 0 finds a matrix client without v2, in which case GT2 Task 10 adds v0/v1 (D394) | Eng | Resolved |
| Q388 | ~~SHA-256 repositories: when, given gitoxide's open SHA-256 parity item and that GitHub mirrors are SHA-1~~ Answered 2026-10-02 by the owner: the recommended default — SHA-1 only until gitoxide's SHA-256 parity lands; the object format is already a field in every record (GT1 Ruling 8) | Eng | Resolved |
| Q389 | ~~Authentication before the unified auth plan (D111): loopback only in the open-source gateway until MT1 (§38 D451, PR #182); the Cloudflare variant is a commercial Cloudflare target (`loams-platform`)~~ Answered 2026-10-02 by the owner: the recommended default — loopback only in the open-source gateway until MT1 (§38 D451) | Founder | Resolved |
| Q390 | ~~Is age-since-write eviction good enough for the direct cache path, or must quotas require the gateway path~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision: GT3 Task 4 measures it; quotas apply on the gateway path | Eng | Resolved |
| Q391 | ~~Per-scope WAL partitions: needed once GT4 measures sequencer contention on a hot monorepo~~ Answered 2026-10-02 by the owner: the recommended default — not now: one WAL per repository (D397), reopened only if GT4 measures sequencer contention | Eng | Resolved |
| Q392 | ~~A server-side merge queue that rebases disjoint-path agent commits onto a shared branch: in GT4, a later phase, or never~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — a later phase after GT4, on demand; why: it keeps GT4's scope, and agents work on their own branches (§15 principle 2) | Founder | Resolved |
| Q393 | ~~Whether the WAL should carry pack bytes for very small pushes (one PUT per push instead of two)~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision: GT1 Task 10's numbers decide; the default is a separate `.lpk` PUT | Eng | Resolved |
| Q394 | ~~The WebDAV subset sccache's backend (OpenDAL `webdav`) needs: `PROPFIND`, `MKCOL`, `HEAD`, `GET`, `PUT`~~ Answered 2026-10-02 by the owner: the plan's default — not an owner decision: GT3 Task 0 records the subset; the default is `PROPFIND`, `MKCOL`, `HEAD`, `GET` and `PUT` | Eng | Resolved |
| Q395 | ~~Timing: start GT1–GT3 now as track GT, beside M, R, D and J, or keep §15's W1 after M3~~ Answered 2026-10-02 by the owner: the chosen default (the doc gives no recommendation) — keep §15's slot: GT1 starts with W1, after M3 (D411); why: the doc gives no reason to start early, and the one-build machine already carries tracks M, R, D, J, CLI, FL, AP and RT | Founder | Resolved |

## 17. The `Fs` trait and object-store providers (D381, D382; moved from the former §35)

When the Cloudflare target moved to `loams-platform` (2026-10-02, §38 D440), its two vendor-neutral decisions stayed here, because Loams Git needs them.

**D381: no Loams-built S3 service.** RustFS is the store where Loams runs its own storage (D61, D178); any S3-compatible service (S3, R2, Cellar, MinIO) is a provider behind `ObjectStoreProvider` (§25 §5), and `loams-store` stays the client. Tenants are isolated by **vended, prefix-scoped credentials** (`ObjectStoreProvider::issue_credentials`); on R2 these are temporary credentials with `prefixes` and `object-read-only` or `object-read-write`, up to 7 days, which GT3's `r2` vendor requests. D178's provider list gains `r2`.

**D382: an `Fs` trait for warm, rebuildable state and scratch.** Loams code that keeps spill buffers (a pack arriving on a push), local caches (pack indexes, scope caches, §5.4) or small mutable metadata (a sequencer's idempotency index) uses `Fs`, so the same code can run where there is no `std::fs`. **Durable truth stays in the object store behind conditional writes (D1)**; an `Fs` is a cache and scratch, never the source of truth.

### 17.1 Where it lives

`crates/loams-fs` (Apache-2.0, no cloud dependency) holds the trait, `NativeFs` (`tokio::fs` under a root directory; `rename(2)`; fsync of file and directory when `survives_restart`; Kubernetes, Lambda `/tmp` (not surviving), laptops) and `MemFs` (tests, and the conformance suite's reference). Cloudflare backends (Durable Object SQLite, R2 blobs) are part of the commercial Cloudflare target in `loams-platform`. One conformance suite, `loams-fs::conformance`, runs against every backend, with capability flags selecting the append and rename cases: read-after-write, `CreateNew` races, range reads at boundaries, list pagination, rename atomicity under a crash, and the limits.

### 17.2 The trait

`crates/loams-fs` (Apache-2.0, no Cloudflare dependency):

```rust
/// Relative, '/'-separated, no empty, '.' or '..' segments, at most 1,024 bytes (R2's key limit).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FsPath(String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode { Overwrite, CreateNew }

#[derive(Clone, Debug)]
pub struct FsMeta { pub len: u64, pub modified_unix_ms: i64 }

#[derive(Clone, Copy, Debug)]
pub struct FsCaps {
    pub append: bool,          // NativeFs, MemFs (and the commercial Durable Object backend)
    pub atomic_rename: bool,   // NativeFs (rename(2)), MemFs
    pub max_file: u64,         // NativeFs: disk; MemFs: configured
    pub survives_restart: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum FsError {
    #[error("not found: {0}")] NotFound(FsPath),
    #[error("already exists: {0}")] AlreadyExists(FsPath),
    #[error("unsupported on this backend: {0}")] Unsupported(&'static str),
    #[error("too large: {0} bytes")] TooLarge(u64),
    #[error("backend: {0}")] Backend(String),
}

#[cfg_attr(not(target_family = "wasm"), async_trait::async_trait)]
#[cfg_attr(target_family = "wasm", async_trait::async_trait(?Send))]   // Workers futures are !Send
pub trait Fs: fmt::Debug {
    fn caps(&self) -> FsCaps;
    async fn read(&self, path: &FsPath) -> Result<Bytes, FsError>;
    async fn read_range(&self, path: &FsPath, range: Range<u64>) -> Result<Bytes, FsError>;
    /// `CreateNew` fails with `AlreadyExists`; a returned write is visible to every later read.
    async fn write(&self, path: &FsPath, data: Bytes, mode: WriteMode) -> Result<(), FsError>;
    /// Returns the new length. `Unsupported` on blob-only backends.
    async fn append(&self, path: &FsPath, data: Bytes) -> Result<u64, FsError>;
    /// Atomic replace of `to`. `Unsupported` on blob-only backends.
    async fn rename(&self, from: &FsPath, to: &FsPath) -> Result<(), FsError>;
    async fn remove(&self, path: &FsPath) -> Result<(), FsError>;   // NotFound is Ok
    async fn stat(&self, path: &FsPath) -> Result<Option<FsMeta>, FsError>;
    /// Paths under `dir`, sorted, strictly after `after`, at most `limit`.
    async fn list(&self, dir: &FsPath, after: Option<&FsPath>, limit: usize)
        -> Result<Vec<(FsPath, FsMeta)>, FsError>;
}
```

**As built (GT1a, Task 2; `crates/loams-fs`).** The trait is as above, with these additions and precise rules:
- `Fs: fmt::Debug + MaybeSendSync`. `MaybeSendSync` is `Send + Sync` on native targets and empty on `wasm32`, so an `Arc<dyn Fs>` can cross tokio tasks (GT1 Task 0, T0-32).
- `FsPath::new` refuses NUL bytes as well, with an `FsPathError`.
- `FsPath::root()` (the empty path) names the whole backend. It is a directory for `list`, never a file.
- `read_range` needs the range inside the file. An empty range up to the length reads nothing; a range past the end, or an inverted one, is an error, never a short read.
- `append` creates a missing file.
- `list` is recursive, returns files only, and takes `dir` as a prefix of whole segments.
- `FsMeta` and `FsCaps` also derive `PartialEq` and `Eq`.
- `NativeFs::new(root, survives_restart)` keeps files under `root/files/`. Each write is staged whole in `root/staging/`, then renamed (`Overwrite`) or hard-linked (`CreateNew`) into place. Opening a root clears its staging directory, and removing a file prunes the directories it empties.
- `SpillDir::new(fs, parent)` makes `<parent>/spill-<ulid>`. `close` removes its files, and so does a task spawned on drop.
- `fs_conformance!` expands the suite into one test per case.


## 18. Sources

Read on 2026-10-01 unless noted.

- The owner's draft "Loams Serverless Runtime — Consolidated Plan" (2026-09-30), §13–§15 (`chatdump.md` lines 814–946).
- Repository: §01 §5, §02 §2–§3 and §7.4 (D270), §03 §6–§7, §04, §09, §15 (all), §18 §5.3 (D75), §24, §25 §5 (`ObjectStoreProvider`), §27; `crates/loams-store/src/store.rs` (`put_if_absent`, `put_if_match`, `get_range`), `crates/loams-cloudevents`, `crates/loams-meta-conformance/src/linearizability.rs`, `crates/loams-log/src/wal.rs` (format conventions).
- Cursor: "Git at any scale", Vicent Martí, https://cursor.com/blog/git-at-any-scale (2026-08). InfoQ: "Cursor Uses S3 WAL to Scale Git Storage to More than 300 Pushes per Second", Leela Kumili, https://www.infoq.com/news/2026/09/cursor-continuity-git-storage/ (2026-09-30).
- GitHub: "Introducing DGit", https://github.blog/news-insights/the-library/introducing-dgit/ (2016-04-05); "Stretching Spokes", https://github.blog/engineering/infrastructure/stretching-spokes/ (2017-10-13, updated 2025-06-03).
- Git: https://git-scm.com/docs/protocol-v2 (`ls-refs`, `fetch` with `filter`, `packfile-uris`, `wait-for-done`; `object-info`; no push command), https://git-scm.com/docs/gitprotocol-http, https://git-scm.com/docs/gitremote-helpers (`connect`, `stateless-connect` "experimental; for internal use only", `fetch`, `push`, `option from-promisor`).
- gitoxide: https://github.com/GitoxideLabs/gitoxide `crate-status.md` (server-side upload-pack/receive-pack plumbing, delta compression in pack encoding, bitmap writing, commit-graph writing and partial clone unchecked; MIDX read/write done), `.github/workflows/ci.yml` (`wasm` job: `gix-pack`, `gix-commitgraph`, `gix-hash` on `wasm32-unknown-unknown`); `gix` 0.88.0 (2026-09-25), MIT OR Apache-2.0.
- go-git: https://github.com/go-git/go-git (Apache-2.0; `plumbing/transport/{upload_pack.go,receive_pack.go}`; v5.19.2, 2026-07-29).
- Object stores: AWS "Amazon S3 now supports conditional writes" (2024-08-20) and "… functionality for conditional writes" (2024-11-25); https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html; https://developers.cloudflare.com/r2/api/s3/api/ (`If-Match`, `If-None-Match` on PutObject; updated 2026-07-31), https://developers.cloudflare.com/r2/platform/limits/ ("Maximum concurrent writes to the same object name (key): 1 per second"; updated 2026-06-08), https://developers.cloudflare.com/r2/reference/consistency (strong read-after-write and listing), https://developers.cloudflare.com/r2/pricing/ (updated 2026-10-01), Cloudflare API `POST /accounts/{account_id}/r2/temp-access-credentials` (`permission`, `prefixes`, `objects`, `ttlSeconds` ≤ 604800); https://docs.cloud.google.com/storage/docs/request-preconditions (updated 2026-09-30); Microsoft "Specifying conditional headers for Blob service operations" (updated 2026-01-20); RustFS https://github.com/rustfs/rustfs (Apache-2.0; 1.0.1-preview.14, 2026-09-30; PRs #6421, #6798, #6801).
- sccache: https://github.com/mozilla/sccache (Apache-2.0; v0.18.0, 2026-09-14; README limits; docs/MultiLevel.md; docs/Configuration.md, docs/S3.md, docs/Webdav.md). mozilla-actions/sccache-action (Apache-2.0, v0.0.11). BuildCache: https://gitlab.com/bits-n-bites/buildcache (zlib; v0.33.1, 2026-09-23).
- Cargo: https://doc.rust-lang.org/cargo/reference/registry-index.html (sparse layout, `config.json` `dl` markers including `{sha256-checksum}`, `auth-required`, `cksum`). kellnr (Apache-2.0, v6.9.0, 2026-09-23), panamax (Apache-2.0, v1.0.14, 2024-06-06), ktra (Apache-2.0).

---

## 19. Monorepo mode: virtual workspaces on the bucket (revision of 2026-10-10)

Status: **Proposed** (2026-10-10). Decisions **D825–D847** and open questions **Q750–Q760** are proposed numbers above the decision log's highest (D824; Q735–Q747 are reserved by CL1). They go into the log when the owner confirms them. Plan: [GT1, rewritten 2026-10-10](../plans/2026-10-10-gt1-loams-git.md).

### 19.1 The direction and what it changes

The owner, 2026-10-10: *"loam git should be similar to google monorepo, download only touched file, spawn million of worktrees on s3, it should wrap git command and take over"*. Later the same day: *secrets as code* in an `env/` folder (§19.13).

Read as a design (Piper and CitC on object storage):

1. **One server-side repository per monorepo, on the bucket.** §4's WAL core is that store and does not change: create-only segments, one-object packs, a per-repository sequencer, O(1) forks.
2. **Workspaces are server-side records**, not clones. A workspace is a base commit plus an overlay of changed paths, saved as a create-only chain of snapshots under the repository's prefix. Creating one costs one PUT, so millions are cheap, and any machine or agent can mount any workspace.
3. **The client is a virtual filesystem.** It shows the whole tree, fetches a file's content on first read, caches it on the host, and uploads only what changed. It never clones.
4. **`git` is taken over.** A shim installed as `git` runs the common porcelain natively against the workspace and the server. Anything it does not support falls through to real git with a one-line notice. Outside a Loams mount it runs real git untouched.
5. **Stock git still works.** Smart HTTP, partial clone and sparse checkout (§6) remain the interop path. Workspaces can be fetched as hidden refs.
6. **It is built for agent fleets.** Each agent gets its own workspace, cheaply. Workspaces tie into `loams-agentd` (§50, DD1) and the software factory (§39 D-SF-18).

| # | Decision (proposed) | § |
|---|---|---|
| D825 | **Monorepo mode is Loams Git's product direction.** §4–§5 (D388–D392) stay as the repository store. §6.4 and phase GT4 are replaced by §19 and folded into the GT1 plan. **Track GT starts now**, superseding D411: the owner's direction reverses Q395's answer | §19.1, §19.14 |
| D826 | **Workspaces are server-side records.** Each workspace is a create-only chain `ws/<ws_id>/snap/<n:020>.lws` under the repository. A snapshot is complete: base, HEAD, local refs, index tree, worktree tree, snapshot commit, workspace packs and a changed-path hint. Creating a workspace is one PUT (two with a name). A save commits at the create-only PUT of snapshot `n+1`, which is D389's pattern | §19.4 |
| D827 | **One writer per workspace, by fencing, with no lease service.** The writer is the mount that wrote the latest snapshot. Another mount becomes the writer only by writing a takeover snapshot: explicitly (`--take`), or automatically once the last snapshot is older than `WRITER_IDLE` (10 min). A fenced former writer's unsaved state goes to a salvage record and is never merged silently. Read-only mounts are unlimited | §19.4.3 |
| D828 | **`loams-vfs`, the lazy client.** One per-user daemon, `loams-vfsd`, serves FUSE on Linux (`fuser`, MIT; FUSE passthrough for opened local files). It has one mount root with workspaces as subdirectories (CitC's layout), and also accepts per-workspace mounts. It fetches trees per directory and blobs per file, lazily. The content-addressed cache is shared by every workspace on a host. A local overlay and change journal make status O(changed). Ignored build directories are redirected to local disk. macOS (NFSv3 loopback first, then FSKit) and Windows (ProjFS) come after GA | §19.6 |
| D829 | **Saves upload only touched files.** The save path debounces, hashes, checks which objects the server already has, writes one workspace pack (`.lpk`) per save with the new blobs, trees and commits, then PUTs the snapshot. Ignored paths stay local. `env/` values are sealed first (D839) | §19.5.3 |
| D830 | **`ObjectService` and derived data.** It serves batched object reads (range reads, coalesced per pack), tree reads with aux data (entry sizes), so `stat` and `ls -l` never fetch blobs, immutable per-object GETs, and has-checks. Derived data (tree aux, commit graph with changed-path Bloom filters, blame) are immutable bucket objects keyed by object id under `derived/` | §19.5 |
| D831 | **A push from a workspace moves no data from the client.** `PushWorkspace` assembles, on the server, the closure the repository lacks from the workspace packs into one `.lpk`. It then commits a `RefTxn` through the repository's sequencer (§4.4). Commits are real git commits (SHA-1), so object ids match on every interop path | §19.7.5 |
| D832 | **The `git` shim.** The binary `loams-git` is installed as `git` ahead of the system git. Outside a Loams workspace it `exec`s real git. Inside one, it runs natively only when the subcommand **and every flag** are on its allowlist. Otherwise it falls through to real git over a projected `.git` (partial clone with a `loams` promisor remote, sparse index, fsmonitor answered from the vfsd journal), with a one-line notice. `git clone` of a Loams URL and `git worktree add` create workspaces | §19.7 |
| D833 | **History runs on the server.** The `Log`, `Blame`, `Grep` and `DiffStat` RPCs run over derived data. The shim formats their results exactly as git does and merges in the workspace's local changes | §19.8 |
| D834 | **The workspace API.** `WorkspaceService` and `ObjectService` join `loams.repos.v1`. They cover create, get, list, fork, delete, snapshots, `ReadFiles`, `ApplyEdits` (agents that do not mount), `Diff`, `Rebase`, `Push` and `Watch` | §19.10 |
| D835 | **Snapshot commits make in-progress work visible to stock git.** Each snapshot records a deterministic commit (tree = worktree tree, parent = HEAD, a fixed author `Loams Snapshot <snapshot@loams.invalid>`, time = the snapshot time). It is advertised as the hidden ref `refs/loams/ws/<ws_id>`: shown only to an `ls-refs` with that prefix, and allowed as a want | §19.9 |
| D836 | **Retention and GC.** Every snapshot is kept for 7 days, then hourly snapshots to 30 days, then one a day while the workspace lives. Named snapshots are kept. Deleted workspaces are purged after `GC_GRACE`. A workspace pack is collected once no retained snapshot names it. Workspace packs are compacted every 64 | §19.4.5 |
| D837 | **GA scope.** Linux FUSE. Local-first (a direct bucket backend) and loopback serving before MT1, behind `GitAuthorizer`; networked serving after MT1. Webhooks after GA. LFS is unnecessary, because blobs are already lazy, so an LFS-compatible read path for stock clients comes after GA. macOS, Windows and Kubernetes packaging after GA | §19.12 |
| D838 | **Licences.** No code is copied from or linked to Sapling, EdenFS or Mononoke (GPL-2.0), or git (GPL-2.0; it runs as an unmodified process, D394). Only ideas are taken, from public documentation and papers. VFS for Git (MIT, C#) is a reference, not a port. The new dependencies are permissive: `fuser` (MIT), gitoxide (MIT OR Apache-2.0), `grep-*` and `ignore` (MIT OR Unlicense), and rules derived from gitleaks (MIT, with attribution) | §19.2 |
| D839 | **Secrets as code.** Values in `env/` files are never stored as git objects. The repository stores the **sealed env format**: per key a reference and a keyed fingerprint, never plaintext and never ciphertext. Sealing happens at the first point plaintext would leave the user's machine (vfsd save, shim, the git clean filter, `ApplyEdits`), and the server refuses an unsealed `env/` file | §19.13.2 |
| D840 | **The vault.** `loams-vault` encrypts each version with its own data key, wrapped by the namespace's KMS key (D96's `KeyProvider`), and stores versions as create-only bucket objects. A `SecretBackend` trait has adapters for OpenBao and HashiCorp Vault (KV v2), AWS Secrets Manager, Azure Key Vault and GCP Secret Manager. D189's Dapr path stays the read path for applications | §19.13.3 |
| D841 | **Versions, drafts and environments.** A sealed value is a version of a secret, identified by (repository, file, key). A value sealed on a branch that is not mapped to an environment is a draft. An environment is a create-only chain of revisions, each a map from key to version. `env/policy.toml` maps branches to environments (default `dev` → `refs/heads/dev`, `main` → `refs/heads/main`) | §19.13.4 |
| D842 | **Promotion on merge.** A committed `RefTxn` that moves a mapped branch triggers `EnvPromoter`. It is durable and idempotent on (repository, environment, seq), writes the environment's next revision and emits `io.loams.dev.env.promoted.v1`. Deployments receive the change through Loams runtimes (read live), `loams env run --watch`, and the Dapr secret-store component. A Kubernetes sync comes after GA | §19.13.5 |
| D843 | **Rotation without commits.** Each key can have a rotation policy, served by a provider: random, Loams Postgres role, Loams SQL role, the external vault's own rotation, or a webhook. A rotation runs as a durable workflow with an overlap grace period. It writes a new version and an environment revision, never a commit. Views note when the live version differs from the pinned one | §19.13.6 |
| D844 | **Access and audit.** Plaintext goes only to a principal with `env:read` on that environment. Agents have no `env:read` unless a grant gives it. Everyone else sees redacted lines. Every plaintext read, seal, promotion and rotation is audited (CloudEvents on `_audit`, never values). vfsd never persists rendered plaintext, except files the user edited on that host | §19.13.7 |
| D845 | **Review without values.** Diffs, `show`, `log -p` and the server's `Diff` show env changes per key (added, removed, changed by fingerprint), never values. A key-level three-way merge driver resolves concurrent edits of different keys | §19.13.8 |
| D846 | **Leak prevention.** Secret scanning uses rules derived from gitleaks plus an exact match against the repository's known fingerprints. It runs at save (the file is held local-only), at native commit, `ApplyEdits`, `PushWorkspace` and `receive-pack` (the push is refused, naming path, line and rule). An allowlist entry needs a reason and is audited | §19.13.9 |
| D847 | **Plain-git interop for env.** Clients without the shim see the sealed format. `loams env filter` is a git clean, smudge, diff and merge driver for people who use plain git. The receive path refuses plaintext `env/` files with a remedy | §19.13.10 |

### 19.2 Prior art, and what Loams takes

| System | What it does | What Loams takes | Licence note |
|---|---|---|---|
| **Google Piper + CitC** (Potvin and Levenberg, "Why Google Stores Billions of Lines of Code in a Single Repository", CACM 2016) | A central monorepo on Spanner/Bigtable. CitC workspaces store only modified files in the cloud, as an overlay on a Piper snapshot, through FUSE. Every save is a snapshot. Workspaces are reachable from any machine and readable by review and search tools. Development is trunk-based | Workspaces as server-side overlays, snapshots per save, the `/<root>/<user>/<ws>` layout, review of in-progress work, trunk-based by default | Paper only |
| **Meta Sapling + EdenFS + Mononoke** | EdenFS is a virtual filesystem (FUSE on Linux, NFSv3 on macOS, ProjFS on Windows) with lazy fetch, an on-disk overlay, a journal for fast status, "redirections" (bind mounts) for build output, and prefetch profiles. Mononoke stores **derived data** (fsnodes with sizes, blame, unodes) in a blobstore. Sapling's CLI talks to the daemon. Commit Cloud syncs workspaces | Journal-based status, redirections for ignored build directories, sizes as derived data so `stat` needs no blob, the daemon-plus-thin-CLI split, an NFS fallback on macOS | **GPL-2.0: no code is read for copying, ported or linked** (D838). Ideas only, from public documentation and talks |
| **Microsoft VFS for Git (GVFS) and Scalar** | ProjFS placeholders, the GVFS protocol (`/gvfs/objects`, a prefetch of commits and trees), a read-object hook, fsmonitor. VFS for Git was retired; Scalar moved to partial clone, sparse checkout, the sparse index, fsmonitor and background maintenance, all now in git | The batched objects endpoint and tree prefetch. The lesson: virtualization is hard to keep correct, so Loams keeps a non-virtual path that always works (partial clone with sparse checkout) and uses Scalar's recipe for the projected `.git` | MIT (C#). Referenced, not ported |
| **git: partial clone, promisor remotes, sparse index, fsmonitor, `bundle-uri`** | Clients that lack objects fetch them lazily, and status scales with the changed set | The projected `.git` of the fallthrough path (§19.7.4), and stock-client interop | git is GPL-2.0 and runs as a process (D394) |
| **Jujutsu** (Apache-2.0) | "The working copy is a commit", snapshotted at every command; an operation log | The snapshot commit (D835) and snapshot-on-command in the shim | Apache-2.0. Ideas only; no dependency |
| **SOPS, git-crypt, External Secrets Operator** | SOPS: encrypted values in files, decrypted with KMS. git-crypt: transparent encryption with clean and smudge filters. ESO: syncs external vaults into Kubernetes Secrets | The clean/smudge filter for plain-git users; key-level diffs; ESO as the Kubernetes delivery after GA. Not ciphertext in git: see D839 and Q757 | MPL-2.0, GPL-3.0 and Apache-2.0 respectively. Ideas only |

### 19.3 Architecture

```
 host (laptop, CI runner, agent sandbox host)                         server (gateway role)            bucket
 ┌──────────────────────────────────────────────────────┐            ┌──────────────────────┐    ┌─────────────────────────┐
 │ git (shim: loams-git) ──unix socket──► loams-vfsd     │  Connect   │ RepoService          │    │ ns/<ns>/repos/<repo_id>/ │
 │   │ outside mount: exec real git       │ FUSE ~/loams │◄──────────►│ WorkspaceService     │───►│   wal/ checkpoints/ packs│
 │   │ unsupported: real git + projection │ overlay      │            │ ObjectService        │    │   ws/<ws_id>/snap/ packs/│
 │   ▼                                    │ CAS cache    │            │ EnvService           │    │   derived/               │
 │ real git (process)                     │ journal      │            │ Smart HTTP (interop) │    │ ns/<ns>/vault/           │
 │                                        └──────┬───────┘            │ sequencers (§4.4)    │    └─────────────────────────┘
 │ local-first: vfsd's DirectBackend writes the bucket itself (fenced) ┘ └──────────────────────┘
 └──────────────────────────────────────────────────────┘
```

- **The shim is a thin client.** Native commands read working files through the mount, as real git would. They read and change workspace state (HEAD, index, refs) through vfsd. Its startup budget is 2 ms when it passes through to real git and 20 ms for a native command, before any I/O (§19.7.6).
- **vfsd is the host's authority** for the workspaces it mounts as writer. It owns the overlay, the journal, the cache and the save pipeline.
- **The server is stateless** for workspaces. The snapshot chain fences writers (D827), so no workspace sequencer exists. Pushes still go through the repository's sequencer.
- **Two backends, one client.** `ApiBackend` uses `loams.repos.v1`. `DirectBackend` uses the `loams-git` core against the bucket with vended credentials, for single-user and local-first use, as `git-remote-loams` does (D395). Both are fenced the same way.

### 19.4 Workspaces as server-side records (D826, D827, D836)

#### 19.4.1 Layout

Added to §4.1:

```
ns/<ns>/repos/<repo_id>/
  ws/<ws_id>/snap/<n:020>.lws        create-only; snapshot 0 is written by CreateWorkspace; ≤ 1 MiB
  ws/<ws_id>/head                    hint {n, written_unix_ms}; ≤ 1 write/s; never ahead of the true tail
  ws/<ws_id>/packs/<checksum>.lpk    objects created in the workspace (blobs, trees, commits), D390's bundle
  ws/<ws_id>/salvage/<mount_id>/<ulid>.lws   a fenced writer's unsaved state (D827)
  _ws_names/<owner>/<name>.json      {ws_id, created_unix_ms}; put_if_absent; unnamed (agent) workspaces have none
  derived/tree-aux/<tree_oid>.lta    entry sizes per tree (D830)
  derived/blame/<commit>/<sha256(path)>.lbl
  derived/graph/<seq:020>.graph      a commit-graph layer for commits since the last compaction (D833)
```

`ws_id` is `w` followed by a 26-character lowercase ULID. `owner` is the principal's stable id.

#### 19.4.2 The snapshot (`proto/loams/git/v1/workspace.proto`, an on-disk format like D390)

A `.lws` object is framed like a segment: magic `LGITWSNP`, `format_version` u16 = 1, `n` u64, the protobuf body, CRC32C, then `LGITWSEN`. Its body is at most 1 MiB, so it stays atomic on RustFS (§4.2).

```protobuf
message ChangedPath {
  string path = 1;
  enum Kind { MODIFIED = 0; ADDED = 1; DELETED = 2; UNTRACKED = 3; TYPE_CHANGED = 4; }
  Kind kind = 2;
  bool staged = 3;                    // the index differs from HEAD at this path
}
message LocalRef { string name = 1; bytes oid = 2; string upstream = 3; }
message Conflict { string path = 1; bytes base = 2; bytes ours = 3; bytes theirs = 4; uint32 mode = 5; }
message Takeover { string from_mount = 1; uint64 from_n = 2; string reason = 3; }
message ForkOrigin { string repo_id = 1; string ws_id = 2; uint64 n = 3; }

message WorkspaceSnapshot {
  string ws_id = 1;  uint64 n = 2;  ObjectFormat object_format = 3;
  string repo_id = 4;  string owner = 5;  string name = 6;          // "" for unnamed workspaces
  string track = 7;                    // "refs/heads/main": what `pull` rebases onto
  bytes base = 8;  uint64 base_seq = 9;    // the commit last rebased onto, and the repository seq it was read at
  bytes head = 10;  string head_ref = 11;  // HEAD; "" when detached
  repeated LocalRef refs = 12;             // workspace-local branches and lightweight tags
  bytes index_tree = 13;                   // the staged tree; conflicts live in `conflicts`
  bytes worktree_tree = 14;                // tracked + untracked, without ignored paths
  bytes snapshot_commit = 15;              // D835
  repeated ChangedPath changed = 16;  bool changed_overflow = 17;   // capped at MAX_CHANGED_HINT
  repeated PackRef packs = 18;             // the live workspace pack set
  string writer = 19;  string principal = 20;  int64 created_unix_ms = 21;
  string label = 22;                       // a named snapshot (`loams ws snapshot --name`)
  Takeover takeover = 23;  ForkOrigin fork = 24;
  repeated Conflict conflicts = 25;
}
```

- **Complete, not a delta.** Mounting reads one snapshot, plus the hint and a probe, as WAL readers do (§4.6). The tree objects carry the size of the overlay, so the record stays small. A change to `k` files writes about `k × depth` new tree objects into the save's pack.
- **Status is O(changed).** The `changed` hint lists every path that differs among HEAD, the index and the worktree. Past `MAX_CHANGED_HINT` (10,000 paths) it is omitted and readers diff the trees, which skips equal subtrees by object id.

**As built (GT1d, Task 22; `crates/loams-git/src/format/snapshot.rs`).** The framing above is exact: the header's `format_version` is u16 LE and `n` u64 LE, the CRC32C covers header and body, and both encoder and decoder refuse a body over 1 MiB. The snapshot commit (D835) has tree `worktree_tree`, parent `head` (none for an unborn workspace), author and committer `Loams Snapshot <snapshot@loams.invalid>` at `created_unix_ms` truncated to seconds in `+0000`, and message `loams snapshot <ws_id>@<n>` with a final newline. Plan ruling T22.1.

#### 19.4.3 Writers, readers and takeover (D827)

```
save: PUT ws/<ws_id>/snap/<n+1>.lws  If-None-Match: *
  200 → saved at n+1
  412 → GET n+1. Its writer is this mount (a retry after an unknown outcome) → saved.
        Its writer is another mount → this mount is fenced: it writes its unsaved state to
        salvage/<mount_id>/<ulid>.lws, becomes read-only, and tells the user and the shim
        ("workspace taken over by <mount> at <time>; your unsaved changes are in salvage <ulid>")
```

- **Mounting as writer.** If the latest snapshot's writer is this mount, the mount continues. If the writer is another mount and the snapshot is younger than `WRITER_IDLE`, the mount is read-only unless `--take` is given. Otherwise vfsd writes a takeover snapshot `n+1`, with the same trees and `takeover` set, before it accepts writes.
- **Read-only mounts** pin a snapshot (`@n`) or follow the latest (polling the hint, or `WatchWorkspace`).
- **Salvage** is listed and restored with `loams ws salvage list|restore`, which replays it as edits on the current snapshot. Paths that conflict are written as conflicts.

#### 19.4.4 Creating, forking and naming

- `CreateWorkspace(repo, track, at?, name?)` writes the name record (when named) with `put_if_absent`, then snapshot 0. Snapshot 0 has `base = head = at` (default: `track`'s tip at `snapshot(Latest)`), `index_tree = worktree_tree = base^{tree}`, and no packs. **It writes no data and copies nothing.**
- **Fork** (`ForkWorkspace(ws@n)`) writes the new workspace's snapshot 0 as a copy of snapshot `n`, with `fork` set and the source's packs referenced by the source's paths. GC counts those references (§19.4.5).
- **Listing** pages through `_ws_names/<owner>/` (named workspaces) or `ws/` (all of them, for administrators and GC).

**As built (GT1d, Task 23; `crates/loams-git/src/ws/`).** `WorkspaceStore` is implemented by `BucketWorkspaceStore` and `MemWorkspaceStore`. The name record is `{ws_id, created_unix_ms}` and the hint `{n, written_unix_ms}`. Create and fork are idempotent on a caller-chosen `ws_id`. An unnamed create or fork is one create-only PUT, and a named one is two. A fork copies snapshot `n` whole, with `fork` set and no label. Its `packs` name the source's packs by checksum, and readers find those packs under the source's prefix by following `fork`. `latest` reads the hint and probes forward, or lists `snap/` when there is no usable hint. Listing all workspaces skips past each workspace's objects (`ws/<ws_id>/~`). Plan rulings T23.1–T23.6.

#### 19.4.5 Retention and GC (D836)

| What | Rule |
|---|---|
| Snapshots | Every snapshot for 7 days; then the last one of each hour to 30 days; then the last one of each day. Named snapshots, the latest snapshot and any snapshot a fork names are kept while the workspace lives |
| Workspace packs | Collected after `GC_GRACE` once no retained snapshot, fork or in-flight push names them, with a GC claim (§03 §7) |
| Workspace pack compaction | When a snapshot names more than `WS_PACKS_COMPACT_AT` (64) packs, a worker merges the reachable objects into one pack. The next save names it, fenced on the snapshot `n` it read |
| Deleted workspaces | `DeleteWorkspace` writes a final snapshot with the label `deleted`; everything under `ws/<ws_id>/` is purged after `GC_GRACE`, unless a fork still names its packs |
| Salvage | 30 days |

### 19.5 Objects: lazy fetch and upload (D829, D830)

#### 19.5.1 Reads

- `GetObjects(ws, oids[])`: up to `FETCH_BATCH_OIDS` (1,024) per call, streamed back. The server resolves through `RangeOdb` (§5.3) over the repository's pack set plus the workspace's packs, groups the reads by pack and coalesces them into ranges of at least 64 KiB.
- `GetTrees(ws, root, depth)`: a tree and its subtrees to `depth` (default `TREE_PREFETCH_DEPTH` = 2), each with **tree aux data**: for every entry, its mode, object id and size. Sizes come from pack entry headers; a delta's header carries the result size, so no blob is inflated. The aux data is cached as `derived/tree-aux/<tree_oid>.lta`, which is immutable.
- `GET /git/<ns>/<repo>.git/loams/objects/<oid>` returns one object, zlib-compressed in the loose format, with `Cache-Control: private, max-age=31536000, immutable`. An organisation's caching proxy can serve it; R2 charges no egress for it.
- **Prefetch.** `.loams/profiles/<name>.txt` holds gitignore-style patterns, committed to the repository; `loams vfs prefetch --profile <name>` uses one. vfsd's fetch log records the paths a command (`cargo build`, `pytest`) read first and suggests a profile (`loams vfs profile suggest`). On `readdir`, vfsd prefetches that directory's trees to depth 2.

**As built (GT1c, Tasks 17 and 19; `crates/loams-git/src/odb/range.rs`, `src/derived/tree_aux.rs`; plan rulings T17.1–T17.5, T19.1–T19.2).** `RangeOdb` reads idx fan-outs once per pack, the 4 KiB oid pages of one bucket per lookup, and entries in reads of at least 64 KiB, through the blob store's H1 cache; `header` answers a delta's result size from at most 20 inflated bytes of its delta data. `WsOdb` searches a workspace's packs after the repository's, so they never shadow an object. Tree aux data is `derived/tree-aux/<tree_oid>.lta`: magic `LGITTAUX`, version 1, the tree id, a `loams.git.v1.TreeAux` body (name, mode, object id, and a size for blobs and symlinks), CRC32C and the end magic `LGITTEND`.

#### 19.5.2 Upload

- `HasObjects(ws, oids[])` returns a bitmap over the repository and the workspace packs.
- `PutWorkspacePack(ws, lpk)` checks that every object hashes to its id, applies the hardening limits of §6.3, runs secret scanning (§19.13.9), and stores `ws/<ws_id>/packs/<checksum>.lpk` (create-only, content-named). The snapshot that names the pack is the commit point. An unnamed pack is garbage after `GC_GRACE`.

**As built (GT1d, Task 24; `crates/loams-git/src/ws/{tree_build.rs,pack.rs}`).** A save's trees come from `TreeBuilder::apply(base_tree, edits, odb)`, which reads and writes only the trees along the changed paths and gives what `git update-index` plus `git write-tree` give. `PutWorkspacePack` is a client stream: the first message names the repository and the workspace, and each message carries the next bytes of the bundle. Before anything is stored, the server checks the footer, the pack and idx trailers, and the idx's order, fan-out and offsets. It reads every entry from that pack alone, with deltas resolved, so each object must hash to the id its idx gives and parse as its kind. A workspace pack must be self-contained: a thin pack is refused. Each object then goes to the scanner hook (a no-op until GT1 Task 67). A workspace's owner, an org admin or the operator may upload. A forked workspace reads packs from its own prefix and then from each fork source's (`WsPackBlobs`, `fork_sources`). Plan rulings T24.1–T24.6.

**As built (GT1d, Task 25; `crates/loams/src/api/workspaces.rs`, `crates/loams-git/src/ws/{lookup.rs,save.rs,salvage.rs,pack_write.rs}`).** `WorkspaceService` serves the lifecycle, snapshots and salvage over the workspace store, and the server's `WsLookup` reads workspaces from it, so object reads and uploads that name a workspace work end to end, through fork chains (at most 8 deep: `fork_too_deep`). `SaveSnapshot` is fenced on `n`; it first checks that the snapshot commit is the snapshot's own, that the packs it names are held, and that its trees, `head` and snapshot commit exist with every new tree connected. A fenced save is kept as salvage under its writer. `RestoreSalvage` replays salvage path by path: paths both writers changed keep the latest entry and become conflicts, and the new trees go into one pack the server writes. Repository readers read workspaces unless the server limits that to owners (`--git-workspace-reads-owner`). Writing needs the owner, an org admin or the operator. `DeleteWorkspace` writes the final `deleted` snapshot. Plan rulings T25.1–T25.9.

**As built (GT1d, Task 26; `crates/loams-git/src/diff/`, `crates/loams-git/src/ws/{edits.rs,merge3.rs}`).** `ReadFiles`, `ApplyEdits`, `Diff` and `Rebase` are served for mount-less agents. `ApplyEdits` builds the trees, writes one server pack and appends `expect_n + 1`; `Diff` prints what `git diff --no-renames --no-indent-heuristic` prints, from Loams' own Myers diff; `Rebase` replays uncommitted changes onto the track's tip with a clean-room diff3 (`merge3`, diff3-style markers), reading only the paths both sides changed. Until env sealing (GT1 Task 62) env files are read redacted and cannot be written. Plan rulings T26.1–T26.6.

**As built (GT1d, Task 27; `crates/loams-git/src/ws/{commit.rs,push.rs}`).** `CommitWorkspace` commits a snapshot's index on the server (author: the caller; committer: Loams), so agents that do not mount can make commits, and `PushWorkspace` pushes them: the closure of the new tips that the repository lacks is walked through the workspace's packs, copied into one repository pack (stored entries byte for byte, deltas resolved), verified and scanned as an upload is, and committed with the ref updates as one `RefTxn` through the repository's sequencer. Updates must fast-forward unless forced or leased (`--force-with-lease`); a push that loses a ref to another push re-checks it against the new tip and retries. Each ref's outcome is in the answer. A push that moves `track` to the workspace's HEAD moves the workspace's base too. `Land` (D1026, Task 90) builds on both. Plan rulings T27.1–T27.8.

#### 19.5.3 The save pipeline (vfsd)

1. **Trigger.** A file is closed after writing, or a rename, unlink or chmod happens. Saves wait for `SAVE_DEBOUNCE` (2 s idle) and are at least `SAVE_MIN_INTERVAL` (1 s) apart. Every native shim command and `loams ws sync` force a save, as jj snapshots on each command.
2. Take the journal entries since the last save, and drop ignored paths (§19.6.4).
3. **Seal** `env/` files (§19.13.2) and **scan** the other files (§19.13.9). A file that fails the scan is held local-only and shown in `git status` as `blocked: possible secret (<rule>)`.
4. Hash the changed files, build the new trees along the changed paths, and build the snapshot commit.
5. Run `HasObjects`, write one `.lpk` with the missing objects, then `PutWorkspacePack`.
6. PUT snapshot `n+1` (§19.4.3), then publish the hint (at most once a second).

Data is **locally durable** when the overlay file is fsynced, and **remotely durable** once the snapshot commits. `git commit` and `loams ws sync` wait for the latter. After a crash, vfsd replays the journal against the last committed snapshot.

### 19.6 The client: `loams-vfs` (D828)

#### 19.6.1 Mounts

- **One root per user**, `$LOAMS_VFS_ROOT` (default `~/loams`), with `~/loams/<namespace>/<repo>/<owner>/<workspace>/` beneath it: CitC's layout, one FUSE session for every workspace. `git worktree add <path>` and `git clone loams://…` create a symlink at `<path>` that points into the root. `--mount` gives a workspace its own FUSE session at `<path>`, for tools that resolve symlinks badly.
- `$XDG_RUNTIME_DIR/loams/mounts` lists the mounted roots and workspaces (written atomically). The shim reads it without contacting vfsd (§19.7.1).

#### 19.6.2 Local state (`$LOAMS_HOME/vfs/`)

| Store | What | Survives restart |
|---|---|---|
| `meta.sqlite` (`rusqlite`, the workspace pin) | the inode table (stable inode numbers per path and workspace), per-workspace state (the last committed snapshot, the writer flag), the journal (sequence, path, kind) | yes |
| `cache/` (an `Fs`, §17) | a content-addressed cache of objects (blobs, trees, tree aux) shared by all workspaces on the host; LRU, `CACHE_DEFAULT_BYTES` 20 GiB | yes, and it can be deleted at any time |
| `overlay/<ws_id>/` | real files for every path written in the workspace; plaintext `env/` files only when edited on this host (§19.13.7) | yes |
| `redirect/<ws_id>/` | the local directories behind redirected build paths (§19.6.4) | yes |

#### 19.6.3 Filesystem semantics

- **Reads.** A file not yet in the overlay is served from the cache, or fetched (`GetObjects`). Once fetched, an opened file is backed by a cache file, and FUSE passthrough (Linux ≥ 6.9) serves its reads in the kernel. Otherwise the kernel page cache is kept (`FOPEN_KEEP_CACHE`).
- **Metadata.** `stat`, `ls -l` and `readdir` are answered from trees and tree aux data, never from blobs. Unchanged files report the base commit's committer time as their mtime. When a rebase or switch changes a file, its mtime becomes the time of the change. Build tools' mtime fingerprints therefore stay valid.
- **Writes** go to the overlay (copy-up on the first write; truncate-on-open needs no fetch). Renames, unlinks, symlinks, chmod (only the executable bit, as git tracks), hard links (refused with `EPERM`, as git cannot represent them) and `fsync` (fsync of the overlay file and the journal) are supported.
- **Scopes** (D397, kept): paths outside the grant's cones are listed but answer `EACCES`, and the server refuses writes outside them (`OutOfScope`).
- **Offline.** Cached content stays readable. An uncached read fails with `EIO` after `FETCH_TIMEOUT` (30 s), with the path logged, and `loams vfs status` reports the mount offline. Saves queue and resume.
- **Switching HEAD costs nothing on disk.** `switch`, `checkout` and `pull` swap the base tree. vfsd keeps overlay paths, and refuses to switch, as git does, when an overlay path conflicts with the target.

#### 19.6.4 Ignored paths and redirections

Paths matched by `.gitignore` (and `.git/info/exclude`) are **local only**. They are never uploaded and never visible on another host. The top-level ignored build directories named in `.loams/redirections.toml` (default: `target/`, `node_modules/`, `.venv/`, `build/`, `dist/`, `__pycache__/`) are **redirected**: vfsd serves them as passthrough directories backed by `redirect/<ws_id>/<path>`, so builds write at native disk speed (EdenFS's redirections, re-implemented). Build outputs that must cross hosts go through the build cache (§8).

#### 19.6.5 Platforms

Linux FUSE is the GA platform (`/dev/fuse`, `fusermount3`; unprivileged in a user namespace since Linux 4.18). **Sandboxes** either bind-mount a workspace directory from the host's vfsd into the sandbox, export it over virtiofs to microVMs (§15 Q3), or run their own vfsd. After GA, macOS gets an NFSv3 loopback server first (no kernel extension), then FSKit (macOS 15.4+); Windows gets ProjFS (Q753).

### 19.7 The `git` shim (D832)

#### 19.7.1 Dispatch

1. The binary `loams-git` is installed as `git` in `$LOAMS_HOME/bin`, which the installer puts ahead of the system git on `PATH`. The desktop app and the CLI installer offer it, on by default with a clear prompt (Q752); `loams git shim uninstall` removes it.
2. The shim finds **real git** from `LOAMS_REAL_GIT`, else as the next `git` on `PATH` whose canonical path is not the shim, cached in `$LOAMS_HOME/real-git`.
3. It resolves the effective directory from `-C`, `--git-dir`, `--work-tree`, `GIT_DIR`, `GIT_WORK_TREE` and the cwd. When that directory is **not inside a mounted workspace** (from the mounts file, with no socket call) or `LOAMS_GIT_IN_FALLBACK=1` is set, the shim `exec`s real git with argv and environment untouched.
4. Inside a workspace, it parses the arguments against the subcommand's allowlist. Any flag, form or configuration it does not implement (for example `commit.gpgsign=true`) means **fallthrough** (§19.7.4). Native handling is never a best-effort approximation.
5. Outside a workspace, `git clone <loams URL> [dir]` (`loams://…`, or an `https://` URL whose `info/refs` advertises `loams-workspaces`) creates a workspace tracking the default branch and links it at `dir`. It prints `loams: cloned as workspace <owner>/<name> (virtual; files download on first read). Use 'git clone --no-loams' for a full clone.` `--no-loams` or `LOAMS_GIT_CLONE=real` runs real git.

#### 19.7.2 Native commands at GA

| Command | Native scope (anything else falls through) |
|---|---|
| `status` | long, `-s`, `--porcelain[=v1\|v2]`, `-b`, `-u[no\|normal\|all]`, `--ignored=no`, pathspecs |
| `diff` | worktree, `--cached`/`--staged`, `<commit>`, `<a>..<b>`, `<a>...<b>`; `--stat`, `--name-only`, `--name-status`, `--numstat`, `-U<n>`, `--no-color`/`--color`, `--exit-code`, `--quiet`, pathspecs. Output is byte-identical to git's default algorithm (Myers, as git's xdiff). `--patience`, `--histogram`, word diff and external diff tools fall through |
| `add`, `rm`, `mv`, `restore`, `reset` | `add <paths>\|-A\|-u\|.`; `rm [--cached] [-r]`; `mv`; `restore [--staged] [--source]`; `reset [--soft\|--mixed\|--hard] [<commit>]`, `reset <paths>`. `-p` and `-i` fall through |
| `commit` | `-m` (repeatable), `-F`, `-a`, `--amend`, `--no-edit`, `--allow-empty`, `--author`, `--date`, `-q`, `--no-verify`. The editor is honoured (`GIT_EDITOR`, `core.editor`). Hooks `pre-commit`, `prepare-commit-msg`, `commit-msg` and `post-commit` run from `core.hooksPath` or the projected hooks directory. Signing falls through |
| `log`, `show` | `--oneline`, `-n`, `--format`/`--pretty` (the placeholders in the porcelain crate's table), `--stat`, `--name-only`, `--name-status`, `-p`, `--graph` (falls through above 1,000 commits), `--all`, `--author`, `--since`/`--until`, `--grep`, ranges, `-- <paths>`; through `Log` (§19.8) |
| `branch`, `switch`, `checkout`, `tag` | list, create, delete, rename, `-u`/`--set-upstream-to`, `-vv`; `switch [-c] <b>`, `checkout [-b] <b>\|<commit>\|-- <paths>`; lightweight tags (annotated and signed tags fall through) |
| `fetch`, `pull`, `push` | `fetch [origin]` refreshes the repository snapshot only (no objects); `pull [--rebase]` rebases the overlay and local commits onto `track` (§19.7.5); `push [-u] [origin] [<src>:<dst>]`, `--force-with-lease`, `--delete`, `-o` (push options) through `PushWorkspace`. A remote that is not the workspace's repository falls through |
| `worktree` | `add <path> [-b <b>] [<commit>]` creates a workspace (one PUT) and links it; `list` lists the user's workspaces of this repository; `remove` deletes the workspace; `prune` |
| `blame`, `grep` | `blame [-L] [-w] [--porcelain] <rev> -- <path>` through `Blame`; `grep [-n] [-i] [-l] [-e] [-w] [-F\|-E] <pattern> [<rev>] [-- <paths>]` through `Grep` plus a local search of overlay paths |
| plumbing | `rev-parse` (`--show-toplevel`, `--git-dir`, `--is-inside-work-tree`, `--abbrev-ref`, `HEAD`, `<rev>`), `ls-files` (`-m`, `-o`, `--exclude-standard`, `-s`), `cat-file` (`-p`, `-t`, `-s`, `-e`, `--batch`, `--batch-check`), `config --get`/`--list` (read only; writes fall through), `symbolic-ref`, `merge-base`, `rev-list --count` |

The command list is the **agent corpus**: the commands Claude Code, Codex, opencode and IDEs actually issue, recorded from their transcripts and fixtures. Merge, rebase, cherry-pick, stash, bisect, submodule and every interactive mode fall through at GA (Q751).

#### 19.7.3 The index and local branches

git's index is `index_tree` plus `conflicts`. Stat data is not needed, because vfsd's journal knows exactly which paths changed. Local branches and tags live in the snapshot (`refs`), so they move with the workspace to any machine. `refs/remotes/origin/*` is the repository's `snapshot(Latest)` at the last `fetch`, held by vfsd per workspace.

#### 19.7.4 Fallthrough on a projected `.git`

When a command falls through inside a workspace:

1. The shim prints one line to stderr: `loams: 'git <cmd> …' is not native yet; running real git <version> on this workspace's projected .git (slower on large trees)`. `loams.quietFallback=true` or `LOAMS_GIT_QUIET_FALLBACK=1` silences it; it is always logged.
2. vfsd **projects** `.git` into the overlay, or refreshes it if it is stale (stamped with the snapshot `n` in `.git/loams-projection`). The projection holds `HEAD`, `refs/` and `packed-refs` from the snapshot, and a `config` with `extensions.partialClone=loams`, `remote.loams.url=loams://…`, `remote.loams.promisor=true`, `core.fsmonitor=loams-git fsmonitor` (answered from the journal), `index.sparse=true`, `core.untrackedCache=true`, `filter.loams-env.*` (§19.13.10) and `core.hooksPath`. Its `index` is a sparse index: the changed directories are expanded and the rest are sparse directory entries. The object directory starts empty. Real git fetches missing objects through `git-remote-loams` (`stateless-connect`, §6.2) into a per-workspace promisor pack store under the cache.
3. The shim runs real git with `LOAMS_GIT_IN_FALLBACK=1`, so nested `git` calls go straight to real git.
4. When real git exits, vfsd **absorbs** the projection: new `HEAD` and `refs/heads/*`, the index (sparse entries back to trees, conflicts at stages 1–3), and new objects from real git's object directory into the next save's pack. Then it saves. A projection changed outside the shim, for example by an IDE through libgit2, is absorbed at the next save the same way.

The projection is **always present** as `.git` at the workspace root, so tools that look for `.git` (IDEs, `cargo`, `pre-commit`) find a repository. It is the same projection that Scalar-style partial clone would produce, so real git's behaviour is real git's.

#### 19.7.5 Pull, rebase and push

- **Pull** (`pull --rebase`, the default in a workspace): read `track`'s new tip `B'`. For each path in the overlay or the local commits that also changed between `base` and `B'` (a tree diff, which skips equal subtrees), run a three-way merge with the vendored algorithm of the porcelain crate (a clean-room diff3, not git's code). Conflicts become `conflicts` and conflict markers in the overlay. Local commits are replayed natively when every one of them merges cleanly. Otherwise the shim falls through to real `git rebase` on the projection. `base := B'` and `base_seq` is recorded. **No unchanged file is downloaded.**
- **Push** (D831): `PushWorkspace(ws@n, updates[], options)` runs on the server. It computes the closure of the new tips that the repository lacks, against the repository's pack set at `snapshot(Latest)`. It reads those objects from the workspace packs by range, writes one `.lpk`, and commits a `RefTxn` through the repository's sequencer with §4.5's derived idempotency key. Fast-forward, protection, scope and secret checks are §6.3's. The client uploads nothing, because the objects arrived at save time. The answer carries the seq as a consistency token.

#### 19.7.6 Performance budgets (gates in GT1, Task 80)

| Operation | Budget (p99, a 10-million-file synthetic tree, 1,000 changed files) |
|---|---|
| Shim passthrough overhead outside a workspace | 2 ms |
| `git status` (native) | 50 ms |
| `git diff --stat` (native, changed blobs cached) | 150 ms |
| `git worktree add` / `CreateWorkspace` | 300 ms (one or two PUTs) |
| Mounting an existing workspace (first `ls` at the root) | 1 s |
| First read of an uncached file in the same region | 150 ms |
| Save of 100 changed files to a durable snapshot | 1 s |
| `git push` of a workspace commit (server-side assembly) | 2 s plus 3 × segment PUT latency |

### 19.8 History on the server (D833)

- **`Log(rev, ranges, pathspec, limit, fields)`** walks the commit graph. The graph comes from compaction's `commit-graph`, written with `--changed-paths` (Bloom filters), plus `derived/graph/<seq>.graph` layers built on demand for later commits. Pathspecs are tested with the Bloom filters, then tree diffs. Results are streamed. The shim formats them.
- **`Blame(rev, path, range)`** uses `gix-blame` (MIT OR Apache-2.0; Task 0 confirms it is mature enough; the fallback is a Loams implementation over `Log` and diffs). It is cached in `derived/blame/`.
- **`Grep(rev, pattern, pathspec, flags)`** runs the `grep-regex` and `grep-searcher` crates (MIT OR Unlicense) in parallel over the tree's blobs, with a deadline and a match limit. It uses §15 §4's code index when one exists. The shim adds a local search of overlay paths, and lets an overlay path's result replace the server's.
- **`DiffStat(a, b, pathspec)`** returns tree-diff statistics. Line counts need blob reads, which the server does and caches per (a, b) pair.
- **`env/` files** in all four return keys and fingerprints only (§19.13.8).

**As built (GT1c, Task 20; `crates/loams-git/src/derived/{graph.rs,bloom.rs}`, `src/history/`).** A `derived/graph/<seq:020>.graph` layer is git's commit-graph format with version 2 changed-path Bloom filters, a `BASE` chain and one Loams chunk, `LOAM`, naming each base's seq; `gix-commitgraph` reads it, and it matches `git commit-graph write --changed-paths` chunk for chunk. A layer holds the commits its seq's refs reach that the newest stored chain lacks, and layers merge geometrically (at most 16 in a chain). Two details differ from git's format page and follow git 2.55 instead: a filter is `ceil(10n/8)` bytes, and the 512-path limit counts directories. `Log` emulates git's default walk (date order, ties in the order reached, the excluded side walked in the same queue with git's slop, default history simplification in which only excluded tips count among excluded parents) and checks a commit's filter before its trees. Pathspecs are literal. `DiffStat` counts lines with Myers' shortest edit script, without rename detection, and is not cached yet. Compaction's `commit-graph/` joins as a base kind with Task 53. Plan rulings T20.1–T20.6.

**As built (GT1c, Task 21; `crates/loams-git/src/derived/blame.rs`, `src/history/grep.rs`).** `Blame` runs `gix-blame` (Myers, whole-file renames followed, every blob diffed as text) and stores each file's blame at a commit in `derived/blame/<commit>/<sha256(path)>.lbl` (`LGITBLAM` v1, CRC32C, `LGITBEND`); a file `gix-blame` cannot walk (a path that was a directory in an ancestor) answers `not_implemented` rather than a partial blame. `Grep` translates POSIX basic and extended patterns to the regex crate, refuses back-references and patterns that match an empty line, searches regular files only, treats a NUL in the first 8,000 bytes as binary, and stops at `GREP_DEADLINE` (30 s) or `GREP_MAX_MATCHES` (10,000), flagging which. The code index is not used yet. Plan rulings T21.1–T21.3.

### 19.9 Interop (D835)

- §6 (Smart HTTP and `git-remote-loams`) is unchanged and is the path for stock clients and CI: `git clone --filter=blob:none --sparse https://…/<repo>.git` works against the same repository.
- `ls-refs` hides `refs/loams/` unless the client asks for that prefix. `fetch` allows any snapshot commit reachable from a workspace the caller may read as a want. CI and reviewers can therefore fetch in-progress work: `git fetch origin refs/loams/ws/<ws_id>`.
- The advertisement adds the capability `loams-workspaces` (an unknown capability, which stock git ignores), so the shim recognises a Loams `https://` URL.

### 19.10 The API (D834)

`loams.repos.v1` gains two services. §44's rules apply: an idempotency key on every mutation, `NO_SIDE_EFFECTS` reads, `loams.errors.v1` reasons and pagination.

| Service | RPCs |
|---|---|
| `WorkspaceService` | `CreateWorkspace`, `GetWorkspace`, `ListWorkspaces`, `ForkWorkspace`, `DeleteWorkspace`, `ListSnapshots`, `GetSnapshot`, `SaveSnapshot` (append a snapshot whose objects were uploaded; fenced on `n`), `ReadFiles` (paths at `ws@n`, rendering `env/` per §19.13.7), `ApplyEdits` (writes, deletes and renames as one new snapshot, for agents that do not mount; seals `env/` and scans), `Diff`, `Rebase` (the server-side pull for mount-less agents), `CommitWorkspace` (commits the index on the server; GT1 Task 27), `PushWorkspace`, `WatchWorkspace`, `ListSalvage`, `RestoreSalvage` |
| `ObjectService` | `GetObjects` (server stream), `GetTrees`, `HasObjects`, `PutWorkspacePack` (client stream), `Log`, `Blame`, `Grep`, `DiffStat` |

New reasons: `workspace_not_found`, `workspace_fenced` (aborted), `workspace_read_only` (failed_precondition), `workspace_name_taken` (already_exists), `object_missing` (failed_precondition), `secret_detected` (failed_precondition), `env_unsealed` (invalid_argument), `env_access_denied` (permission_denied), `env_foreign_reference` (permission_denied).

**As built (GT1b, Task 12; `proto/loams/repos/v1/`).** `loams.repos.v1` holds four files: `repos.proto` (`RepoService`: `CreateRepo`, `GetRepo`, `ListRepos`, `ForkRepo`, `DeleteRepo`, `UpdateRepoConfig`, `ListRefs`, `CompactRepo`, `WatchRepo`) and empty stubs of `workspaces.proto`, `objects.proto` and `env.proto`, which Tasks 25, 18 and 61 fill in. Every reason above and `RepoService`'s own (`repo_not_found`, `repo_already_exists`, `repo_deleting`, `ref_stale`, `ref_protected`, `out_of_scope`, `object_format_unsupported`, `fork_too_deep`, `quota_exceeded`, `sequencer_unavailable`, `idempotency_key_reused`) is registered in `docs/api/reasons.md`. The package is in the catalogue with `available` following the `loams` cargo feature `git`, and is unstable until GA.

### 19.11 Agent scale

- **Cost (estimate, R2 prices of §12).** A workspace costs 1–2 PUTs to create ($4.50–9 per million workspaces) and about 1 KB to keep (one million idle workspaces ≈ 1 GB ≈ $0.015 a month). A save is two PUTs plus a hint at most once a second: an agent saving every 10 s for 8 hours makes about 2,900 saves, about 5,800 PUTs, **about $0.03 per agent-day**. Reads are Class B ($0.36 per million) and are shared across a host's agents by the host cache.
- **A thousand agents on one host** share one vfsd, one FUSE session and one cache. A new agent's workspace on the same base costs no download.
- **`loams-agentd`** (§50, DD1): its `CreateWorktree` and `DeleteWorktree` RPCs create and delete Loams workspaces when the repository is a Loams repository. agentd supervises vfsd on the desktop as it supervises the engine (D790). Each harness run gets a scoped token whose grant covers only its workspace and cones.
- **Mount-less agents** (remote, serverless, or §39's `forgejo.propose_patch`) use `ReadFiles`, `ApplyEdits`, `Rebase`, `CommitWorkspace` and `PushWorkspace` without FUSE.
- **Contention** stays in the repository sequencer at push time (D391, D397). Saves never touch it.

### 19.12 Phases, schedule and GA (D825, D837)

The single GT1 plan (2026-10-10, rewritten) carries everything to GA, in milestones GT1a–GT1l, **starting now**. GT5 (Continuity-style NVMe replicas, gossip, any-node writes) is unchanged and is not planned.

| In GA | After GA (planned as tasks after the GA gate) |
|---|---|
| WAL core, repositories as a service, `ObjectService`, workspaces, `loams-vfs` on Linux, the shim, Smart HTTP and the helper, compaction and GC, env secrets as code, the build cache and crates mirror, operations | The LFS-compatible read path (an LFS batch API serving pointer files' objects from the namespace CAS, for stock clients), push webhooks (through links and notifications, from `_git`), macOS (NFSv3, then FSKit), Windows (ProjFS), Kubernetes packaging (with an External Secrets Operator provider for env) |

GA serves loopback and local-first (the direct backend) before MT1, behind the `GitAuthorizer` and env-grant seams. Networked serving follows MT1 (D111) with no change to the GT1 code paths.

### 19.13 Secrets as code: `env/` (D839–D847)

#### 19.13.1 The requirement

The owner, 2026-10-10 (from the coordinator's brief): a repository can have an `env/` folder (`env/dev.env`, `env/main.env`, or one file per environment) that people commit like normal files. Loams Git never stores the values as plaintext git objects. Values go to a key vault (Loams' own, backed by KMS, or an external one), and the repository keeps only references. Secrets rotate automatically, per key, without a commit. A merge into `dev` or `main` promotes the env changes to that environment, and running deployments get the new values. Checkouts show env files decrypted only to authorised principals, and every read is audited. Reviews show key names and whether each key was added, changed or removed, never values. Secrets pasted outside `env/` are caught at push. Plain git clients see the sealed format, never plaintext.

#### 19.13.2 The sealed env format (D839)

Files matching `env/**/*.env` (configurable in `env/policy.toml`, Q758) are env files. In a workspace an authorised user edits ordinary dotenv text. The **git object** is always the sealed form:

```
# loams-env v1 — sealed by Loams Git; values are in the vault, not in git. Edit in a Loams workspace or with `loams env`.
DATABASE_URL=loams-env:v1:s01j9x…q2@7:fp=3f9a1c0d5e7b2a4c8d1e6f0a9b3c5d7e
STRIPE_KEY=loams-env:v1:s01j9y…k8@2:fp=…
# loams:plain
LOG_LEVEL=info
```

- A sealed line is `KEY=loams-env:v1:<secret_id>@<version>:fp=<fingerprint>`. `secret_id` is `s` plus a ULID, allocated on the first seal of (repository, file path, key) and reused afterwards. `fingerprint` is the first 128 bits of `HMAC-SHA256(repo_fp_key, key ‖ 0x00 ‖ value)`, where `repo_fp_key` is a per-repository key held in the vault and never in git.
- Comments, blank lines and order are kept verbatim. A value preceded by `# loams:plain` stays in git as plaintext (non-secret configuration). Values can use the dotenv quoting rules of the `dotenvy` crate (MIT); multi-line values are sealed whole.
- **Sealing is deterministic and idempotent.** If a key's fingerprint equals the one in the base or HEAD version of the file, the existing reference is reused with no vault call. Otherwise `Seal(repo, path, key, value)` is idempotent on (secret, fingerprint): sealing the same new value twice (a `git status` through the clean filter, a retried save) returns the same draft version. Re-saving an unchanged file therefore never creates versions or changes object ids.
- **References, not ciphertext** (Q757). Ciphertext in git can never be revoked: history keeps it, and anyone who later gains the key can read it. References can be revoked, rotated and purged in the vault. The cost is that reading needs the vault, which lazy fetch needs anyway.
- **Where sealing happens:** vfsd's save (§19.5.3 step 3), native `add` and `commit`, the clean filter (§19.13.10), and `ApplyEdits`. The overlay may hold plaintext on the editing host. Nothing leaves the host unsealed.

#### 19.13.3 The vault (D840)

- **`loams-vault`** (Apache-2.0) stores each version as `ns/<ns>/vault/secrets/<secret_id>/v/<version:010>.lsv`, create-only. The body is AES-256-GCM under a fresh data key, and the data key is wrapped by the namespace's key through D96's `KeyProvider` (AWS KMS, GCP Cloud KMS, Azure Key Vault keys; a file provider for tests and local-first use). The version record carries the fingerprint, the creator, the time and the state (`draft`, `live`, `superseded`, `destroyed`). `destroyed` deletes the body and keeps the record.
- **`SecretBackend`** abstracts where the bodies live. Adapters: `LoamsVault` (the default), `OpenBao` (HashiCorp Vault and OpenBao, KV v2 HTTP API), `AwsSecretsManager`, `AzureKeyVault` and `GcpSecretManager`, each mapping (secret, version) onto the store's own versioning and labels. One conformance suite covers them all. The repository chooses a backend in `env/policy.toml` (`[vault] backend = "loams" | "openbao:<mount>" | …`). The backend's credentials are namespace configuration, never repository content. With an external backend, Loams keeps only the version records and fingerprints, not the bodies.
- **D189** (Dapr's secrets building block) stays the path by which applications read external stores. Loams' vault is exposed to Dapr as a secret-store component (`loams`), so applications read an environment's live set the same way (§19.13.5).
- The vault can serve other Loams credentials later (§46's role passwords, Q30's credential store). That is out of GT1's scope.

#### 19.13.4 Versions, drafts and environments (D841)

- **Draft.** A version sealed in a workspace, or on a branch that is not mapped to an environment. It is readable only by principals who could read the environment its file maps to, plus the author.
- **Environment.** `env/policy.toml` maps environments to branches and files:

  ```toml
  [environments.dev]
  branch = "refs/heads/dev"
  files  = ["env/_shared.env", "env/dev.env"]   # later files override earlier keys
  [environments.main]
  branch = "refs/heads/main"
  files  = ["env/_shared.env", "env/main.env"]
  [keys.DATABASE_URL]
  rotate = { every = "30d", provider = "loams-postgres", role = "app", grace = "1h" }
  ```

  With no policy file, the defaults are `dev` and `main` as above, each reading `env/<environment>.env`.
- **An environment is a create-only chain of revisions**, `ns/<ns>/vault/env/<repo_id>/<environment>/rev/<n:020>.ler`, each a map from key to (secret, version). The latest revision is the **live set**. A revision records its cause: `promotion {commit, seq}` or `rotation {key, from, to}`.

#### 19.13.5 Promotion on merge, and delivery (D842)

1. `EnvPromoter` consumes the sequencer's committed transactions (GT1 Task 15's `GitEvent::Committed`). For each update that moves a mapped branch to a new tip, it reads the environment's files at that tip (sealed lines only).
2. It computes the new live set. Keys whose referenced version is a draft are marked `live`; removed keys leave the set. **A key whose live version came from a rotation and whose referenced version is unchanged in the merge keeps the rotated version**, so a merge never rolls back a rotation.
3. It writes revision `n+1` (create-only, idempotent on (repository, environment, seq)) and emits `io.loams.dev.env.promoted.v1` on the namespace stream `_env`, with the environment, revision, commit and key names (added, changed, removed), never values.
4. **Delivery.** Loams runtimes (Live, the serverless runtime) read the live set at invocation. `loams env run --env main [--watch] -- <cmd>` injects variables and restarts the child on a new revision. The Dapr `loams` secret store reads the live set. A Kubernetes sync controller (an External Secrets Operator provider plus a rollout annotation) comes after GA. Deployments can match revisions to code through the `commit` field.

The whole promotion is a durable execution (`loams-durable`), so a crash resumes it, and its idempotency key makes it exactly-once per (environment, seq).

#### 19.13.6 Rotation (D843)

- A `[keys.<KEY>] rotate` entry sets `every`, `provider` and `grace`. Providers: `random` (`bytes`, `alphabet`), `loams-postgres` (a role password through pg-control's `ResetRolePassword`, with an A/B role pair so the old credential works during `grace`), `loams-sql` (`RotateRolePassword`), `external` (the external vault rotates; Loams follows its latest version) and `webhook` (POST to a URL that returns the new value; HTTPS only; signed).
- The rotation workflow (durable) runs: create the new credential at the provider; seal it as version `v+1` (`live`); write an environment revision `{rotation}`; emit `io.loams.dev.env.rotated.v1`; wait for `grace`; revoke the old credential at the provider; mark the old version `superseded`.
- **No commit is made.** The sealed file in git still pins `@v`. Renders for authorised principals show the pinned value with a trailing comment `# loams: rotated, live is v<v+1> (<date>)`, and `loams env show --live` shows the live value. A later edit of that key in a branch seals a new value. Promoting it replaces the rotated version and resets the rotation clock.

#### 19.13.7 Reads, redaction and audit (D844)

- **Grants.** `GitGrant` gains `env: Vec<EnvGrant { environment, access: Read | Write | Promote }>`. MT1 maps these to OpenFGA relations. **Agents' vended tokens carry no env grant unless one is given explicitly**, so an agent sees redacted env files and cannot leak values into its transcript.
- **Rendering on the lazy fetch path.** vfsd renders an env file on open. It fetches the sealed blob and calls `EnvService.Open(repo, environment, refs[])`, which checks the grant, audits the read and returns the values. The rendered text is kept in memory only, for at most `ENV_RENDER_TTL` (5 min). Lines the principal may not read render as `KEY=<redacted fp=3f9a1c0d>`. The file's size is the rendered size, computed at `lookup`. Env files are small and few, so `stat` may call the vault.
- **Editing a redacted file.** A redacted line left as it was keeps its reference. A redacted line that is changed is refused with `EACCES` at save, and the shim names the key.
- **Plaintext on disk** exists only in the overlay of a file the user edited on that host (mode 0600 within the 0700 `$LOAMS_HOME`). `loams vfs scrub-env` removes it after its save.
- **Audit.** `io.loams.dev.env.read.v1`, `.sealed.v1`, `.promoted.v1`, `.rotated.v1` and `.destroyed.v1` go to `_audit`, with the principal, the workspace or deployment, the environment, keys and versions. Never values.

#### 19.13.8 Review without values (D845)

- The shim's `diff`, `show` and `log -p`, and the server's `Diff`, `DiffStat` and `Grep`, recognise env files. They print a **key-level diff** instead of a line diff:

  ```
  env/main.env (sealed; values hidden)
    + NEW_FEATURE_FLAG          added
    ~ DATABASE_URL              changed (fp 3f9a1c0d → 9b20e4aa)
    - LEGACY_TOKEN              removed
      LOG_LEVEL: "debug" → "info"   (loams:plain)
  ```

  Real git on the projection uses the same text through the `diff=loams-env` textconv driver (§19.13.10).
- **Merge.** A key-level three-way merge (`merge=loams-env`, native in the shim's pull and in `PushWorkspace`'s fast-forward checks): different keys merge cleanly; the same key changed to different fingerprints is a conflict, shown with fingerprints, never values.

#### 19.13.9 Leak prevention (D846)

- **Detectors.** Rules derived from gitleaks' default rule set (MIT; the attribution is in `NOTICE`), compiled with the `regex` crate, plus entropy checks. Then **known-value matching**: every token of a scanned file (split on non-token characters, 16–512 bytes long) is fingerprinted with the repository's `repo_fp_key`, and a match against any known fingerprint of the repository's secrets is a certain leak with no regex needed.
- **Where.** In vfsd's save the file is held local-only, and `git status` and `loams ws status` report it. Native `commit` refuses with `loams: possible secret in <path>:<line> (<rule>); move it to env/ or allow it with 'loams env allow <path>:<line> --reason …'`. `ApplyEdits`, `PutWorkspacePack`, `PushWorkspace` and `receive-pack` refuse, the last with report-status `secret detected in <path>:<line> (<rule>)`. **The server is the authority.** Client checks are early warnings.
- **Allowlist.** `.loams/secret-allowlist.toml` entries carry a path, a line fingerprint, a rule and a reason. Changes to it are audited. An entry never allows known-value matches.
- `.env` files outside `env/` are scanned like any file, and the scanner's message suggests `env/`.

#### 19.13.10 Plain-git interop (D847)

- Clients without the shim (Smart HTTP, `git-remote-loams`, mirrors) see the sealed format, which is valid dotenv text with opaque values.
- `loams env init` writes `.gitattributes` (`env/** filter=loams-env diff=loams-env merge=loams-env`) and `env/policy.toml`. `loams env filter` implements git's long-running filter protocol (`filter.<driver>.process`) for clean (seal) and smudge (render or redact), a textconv for diff, and the merge driver. A plain-git user with the `loams` CLI and a credential therefore gets the workspace experience on a normal clone. Without it, the user sees sealed files.
- `receive-pack`, `PushWorkspace` and `ApplyEdits` refuse an env file that is not fully sealed (`env_unsealed`; report-status `env file <path> is not sealed: run 'loams env init' or push through Loams Git`). They also refuse a reference to a secret that does not belong to this repository (`env_foreign_reference`), so a fork or another repository cannot point at someone else's secrets. **Forks do not inherit env grants.**

### 19.14 Conflicts with earlier decisions

| Earlier | §19 | Resolution |
|---|---|---|
| D411 (GT keeps §15's W1 slot, after M3; Q395) | D825: start now | The owner's direction of 2026-10-10 reverses Q395's answer. D411 is superseded when D825 is confirmed |
| D397 (`loams-vfs` as the lower layer of an agent's local `/workspace`, GT4) | D826–D829 | Amended: the workspace is a server-side record and `loams-vfs` is the primary client. Cones and write admission are kept |
| §6.4 "a checkpoint commits the upper layer on the agent's branch" | Snapshots per save, commits by `git commit` | Snapshots are not commits on a branch. The snapshot commit (D835) is a hidden ref, and branches move only on push |
| §15 §5.1 (overlayfs upper on local NVMe over a lazy lower) | vfsd's overlay | Same layering, inside one FUSE daemon. Sandboxes bind-mount or use virtiofs (§19.6.5) |
| §2.2 (LFS through §15 §3.2's batch API in W1) | D837 | LFS is unnecessary for workspaces; an LFS-compatible read path comes after GA |
| §2.2 (no concurrent multi-writer workspaces) | D827 | Kept: one writer, any number of readers, explicit takeover |
| D394 (stock git only as a separate process) | D832 | Kept: the shim `exec`s or spawns real git and links none of it |
| §15 principle 2 (one writer per workspace) | D827 | Kept, and enforced by fencing |
| D189 (tenant secrets through Dapr) | D840 | Complementary: Dapr stays the application read path; env needs writes and versions, so `SecretBackend` has native adapters, and Loams' vault is a Dapr component |
| D96 (customer-managed keys by envelope encryption) | D840 | Reused: the vault's data keys are wrapped by the namespace's `KeyProvider` key |
| D393 (usage through §27's hooks only) | §19 metrics | Kept: new families `loams_git_ws_*`, `loams_vfs_*` and `loams_env_*` follow §11's labels |

### 19.15 Open questions

Resolved with the plan's defaults on 2026-10-10 (coordinator's brief): **start now** (D825); **a loopback and local-first GA before MT1, with auth seams** (D837; was GT-Q6); **LFS unnecessary, with an LFS-compatible read path after GA** (D837; was GT-Q4); **webhooks after GA** (was GT-Q5); **Kubernetes packaging later** (was GT-Q7). The plan's other earlier questions take their stated defaults (the plan, "Open questions").

| # | Question | Default | Owner | Needed by |
|---|---|---|---|---|
| Q750 | **Workspace visibility.** Readable by every principal who can read the repository (CitC-like; enables review and agents reading peers' in-progress work), or by the owner and explicit grants only? | Repository readers can read; only the owner (and grants) can write or take over. `env/` stays per environment grant either way | Founder | GT1 Task 25 |
| Q751 | **Native versus fallthrough at GA.** Is §19.7.2's list right, with merge, rebase, cherry-pick and stash falling through to real git on the projection? | Yes | Founder | GT1 Task 36 |
| Q752 | **Taking over `git` by default.** Install the shim ahead of the system git by default (with a prompt and an uninstall), or opt-in only? | Installed by default with a prompt in the desktop app and the CLI installer; opt-out | Founder | GT1 Task 36 |
| Q753 | **macOS order.** NFSv3 loopback first (works on any macOS, as EdenFS does) or FSKit first (macOS 15.4+, needs an app extension and signing)? | NFSv3 first | Eng | After GA |
| Q754 | **Save cadence and retention** (cost against recovery): 2 s debounce and 1 s minimum interval; all snapshots for 7 d, hourly to 30 d, then daily | As stated | Eng | GT1 Task 32 |
| Q755 | **Performance-gate fixtures.** Which monorepo shapes gate GA (a synthetic 10-million-file tree plus the Linux kernel's history; a Chromium-sized tree nightly)? | As stated | Eng + Founder | GT1 Task 80 |
| Q756 | **Ignored outputs never cross hosts.** Confirm that `target/` and similar stay local, with the build cache (§8) as the cross-host path | Confirm | Founder | GT1 Task 31 |
| Q757 | **References or encrypted envelopes in git.** References only (revocable; reading needs the vault), or SOPS-style ciphertext in git (offline decryption with KMS access; never revocable)? | References only | Founder | GT1 Task 61 |
| Q758 | **Which files are env files.** `env/**/*.env` only, or also `.env*` anywhere in the tree? | `env/**/*.env`; `.env*` elsewhere is scanned as a possible leak | Founder | GT1 Task 61 |
| Q759 | **Rotation providers at GA.** random, Loams Postgres, Loams SQL, external and webhook; are cloud-database IAM providers (RDS, Cloud SQL) needed at GA? | Not at GA | Founder | GT1 Task 66 |
| Q760 | **Agents and env.** Agents never see plaintext unless explicitly granted (D844). Should an agent with `Write` be able to *set* values it cannot read (sealing new values blind), for example when provisioning a database? | Yes: write without read is allowed and audited | Founder | GT1 Task 62 |

Still open from before: **GT-Q8** (credentials for R2, S3 Standard and S3 Express One Zone measurements; RustFS only until they are provided).

### 19.16 Sources for §19

From the author's knowledge, not re-read on 2026-10-10 (Task 0 of the plan checks the version-specific facts):

- R. Potvin, J. Levenberg, "Why Google Stores Billions of Lines of Code in a Single Repository", *CACM* 59(7), 2016 (Piper, CitC).
- Sapling SCM, EdenFS and Mononoke: https://sapling-scm.com, https://github.com/facebook/sapling (GPL-2.0); EdenFS documentation on overlays, journals, redirections and NFS on macOS.
- Microsoft VFS for Git (https://github.com/microsoft/VFSForGit, MIT, archived) and Scalar (in git since 2.38); the GVFS protocol documentation.
- git documentation: partial clone, `gitremote-helpers` (`stateless-connect`), sparse index (`index.sparse`), `core.fsmonitor`, `gitattributes` (`filter.<driver>.process`, textconv, merge drivers), `commit-graph --changed-paths`.
- Jujutsu (https://github.com/jj-vcs/jj, Apache-2.0): working-copy commits and the operation log.
- `fuser` (https://github.com/cberner/fuser, MIT); Linux FUSE passthrough (6.9).
- SOPS (MPL-2.0), git-crypt (GPL-3.0), External Secrets Operator (Apache-2.0), gitleaks (MIT), OpenBao (MPL-2.0; KV v2 API), HashiCorp Vault KV v2 API, AWS Secrets Manager, Azure Key Vault and GCP Secret Manager APIs; `dotenvy` (MIT).

---

## 20. Landscape: Enroute and Scion, and what Loams Git takes from them (2026-10-16)

Status: **Approved** (owner, 2026-10-16: D1025–D1029 with the defaults of Q906–Q909). The owner compared Loams Git with Enroute and with Anthropic's Scion and on 2026-10-16 approved writing five proposals into this document. They are decisions **D1025–D1029** and open questions **Q906–Q909**, entered in the decision log as Proposed with their defaults filled. The plan tasks are [GT1](../plans/2026-10-10-gt1-loams-git.md) Tasks 88–92. Nothing here changes an approved decision until the owner confirms it. §20.9 lists what each proposal would amend.

Markers as in the rest of this document: **(source)** means read on 2026-10-16 at the URL in §20.11, as the vendor or author states it, not checked by Loams. **(target)** means a number Loams sets out to reach and has not measured. **(verify)** means the task that builds it checks it first.

### 20.1 The two systems

**Enroute** (https://enroute.sh, `github.com/enroute-sh/enroute`; Apache-2.0, Rust; v0.2.0 on 2026-09-25 and v0.3.0 on 2026-10-14 per the GitHub releases API, whose dates carry the year that the release page omits). Enroute is a git-compatible server on object storage, aimed at CI, agent artifacts and embedded version control. As described in its README, its v0.3.0 release notes and its article "Everyone is doing Git on S3 wrong" (2026-10-13) (source):
- **A ledger.** Each repository's refs and its list of layers live in a per-repository Raft log on the ledger nodes' disks, "CockroachDB-style MultiRaft". A push is acknowledged once a majority of ledger nodes hold its entry. Ref updates are compare-and-swap in that log, so pushes to different branches do not conflict.
- **Segments and layers.** Each commit becomes a *segment*: the objects that make up its change from its parents. Consecutive segments are grouped into immutable *layers*, which are pack-like files with an index and a table of contents at the end. A delta only points to the same path in a parent commit, so a client that has the parent can use it directly. A fetch sends the segments of the missing commits, without a tree walk.
- **Small layers inline.** Small pushes are stored in the log itself. Only layers over 1 MiB go straight to the bucket. About once a second, a compute node merges the recent inline layers into one file, uploads it and swaps it into the log, like an LSM memtable flush. Compaction merges about eight similar-sized layers at a time, copying bytes without recompressing them. There is no `git repack`.
- **Stateless compute.** Compute nodes build layers, serve clones and fetches, and compact. They keep only caches: a buffer pool over layers in memory and on disk, filled by parallel ranged GETs.
- **Numbers.** With its own benchmark, forgemark, on EC2 and S3: a peak of 7,619 pushes/s into one repository with 512 concurrent pushers, and a median push latency of 4.4 ms for one pusher. Against stock git 2.56.0 (`git http-backend`, a repacked linux.git): shallow clones more than 2x faster, partial clones on par, full clones about 17% slower, incremental fetches about 13% slower (attributed to the client decoding Enroute's deltas). A cold node cloned linux.git in 128.6 s against 126.0 s warm (source).
- **Not yet built** (the article): SSH and the less common parts of the protocol (smart HTTP only), garbage collection of unreachable objects, and a gRPC write API for server-side commits and merges. The v0.3.0 release notes, a day later, list "commits made through the API without a Git client" (source). 0.x releases are not backward compatible: 0.3.0 cannot read repositories that 0.2.0 wrote.
- **Its critique of WAL-on-S3 designs** (Cursor's Continuity, §3, and open-source followers such as walgit): every push contends on one WAL index object and one machine applies pushes in order, so one repository's write throughput is capped by that machine. Continuity's own figures (120 pushes/s on S3 Standard, over 300 on S3 Express One Zone) are limited by git compaction on disk. Read replicas need a full copy of the repository on disk (source).

**Scion** (Anthropic Engineering, "Redesigning source control at Anthropic", 2026-10-13, updated 2026-10-15). Scion is Anthropic's internal version control system for its monorepo. It is **not git-compatible**: its client departs from git's fetch protocol, history was ingested from git, and the article calls it "by no means a suitable replacement for git for general use". It is not open source. As described (source):
- **Changesets in a WAL.** The unit is a changeset: the files changed relative to its first parent, with content-addressed pointers to compressed contents in blob storage. Spanner holds refs and metadata. Moving a ref is a compare-and-swap on a single Spanner row, separate from creating changesets, and refs other than main and release branches are discouraged.
- **The server does the work.** Commits, rebases and merges happen on the server, so there is no push. There is no global merge-queue lock: Anthropic relies on post-merge and pre-deploy testing, accepts that concurrent changes can conflict, and has Claude fix the conflicts.
- **Derived data** (blame, path history, commit-graph skip lists) is computed by one generic mechanism, recursively from parent values, eagerly or deferred, and written back to Spanner.
- **Strict tiers.** A `scion-daemon` per node stores objects for the whole node and starts from a periodically refreshed snapshot. A regional `scion-cache` streams objects from memory. `scion-server` is authoritative. The tiers are strictly enforced to isolate failure domains, and each tier's hit rate is over 99%.
- **Checkouts** are optional, sparse or virtualised; many CI and write workloads use the API with no checkout. A fetch starts from a root tree and requests only missing trees and blobs, so it is resumable. There is no packfile and no repository maintenance.
- **Numbers.** Merges grew from about 2,500 a day (May 2026) to over 30,000 a day (September 2026). About 25 million CI checkouts a day. Infrastructure CPU and memory are about 200x smaller than the previous several hundred git mirrors. The review tool, Antique, can express semantic constraints on approvals (for example, comment-only changes against sensitive logic).

**Unofficial projects** after the article: `CarterT27/scion` (Apache-2.0, Rust; it calls itself an independent clean-room implementation; CockroachDB for metadata and S3-compatible storage for objects; early; git protocol compatibility and two-way sync are out of scope) and `tinyhumansai/openscion` (**GPL-3.0-only**, Rust; it follows Scion's design with BLAKE3-named objects, a per-node daemon, a regional cache and a server). §20.8 sets how Loams may use each.

### 20.2 Comparison

| | **Loams Git** (§4–§19) | **Enroute** (v0.3.0) | **Scion** (internal) |
|---|---|---|---|
| Git compatibility | Full: Smart HTTP, `git-remote-loams`, stock clients; SHA-1 commits with matching ids (D831) | Full over smart HTTP; no SSH yet | None; own client and protocol |
| Where refs are ordered | The bucket: a create-only segment PUT is the commit point (D389), one sequencer per repository with group commit (D391) | A per-repository Raft log on ledger nodes | A single-row CAS in Spanner |
| Acknowledged when | Pack and segment are in the bucket | A ledger majority holds the entry | The Spanner commit |
| Data layout | One `.lpk` per push or save (pack, idx, footer); geometric repack by stock git off the push path (D396) | Per-commit segments in immutable layers with a table of contents; deltas only to the same path in a parent; LSM-style merges | Changesets plus content-addressed blobs; no packfiles |
| Server-side commit, rebase, merge | Workspaces: saves, `ApplyEdits`, `Rebase`, `PushWorkspace` (D831, D834); no land with retry yet | Listed in v0.3.0 notes; called unbuilt a day earlier | Yes; no push at all |
| Merge queue | None; Q392 left a queue to a later phase | None stated | None; post-merge testing, agents fix conflicts |
| History on the server | `Log`, `Blame`, `Grep`, `DiffStat` over derived data (D833) | Not stated | Blame, path history, skip lists as derived data |
| Client and caches | `loams-vfsd` per host with a shared content-addressed cache (D828); no regional tier | Buffer pool on compute nodes | Per-node daemon, regional cache, server; strict tiers |
| Workspaces | Server-side, any machine can mount, one PUT to create (D826) | None | Sparse and virtualised checkouts |
| Licence | Apache-2.0 | Apache-2.0 (CLA to OpenCanopy GmbH for patches) | Closed |
| **Single-repository push throughput** | **Bounded by group size over segment PUT latency.** At most 64 transactions per segment (`MAX_GROUP_TXNS`) per segment PUT, so at most `64 / L` per second. Measured on one machine (§4.4): 145 commits/s in memory and 93 on RustFS with 100 ms added per PUT and 32 pushers. **Unmeasured on R2, S3 Standard and S3 Express One Zone** (GT-Q8) | 7,619 pushes/s claimed with 512 pushers (source) | About 30,000 merges a day reported, about 0.35 a second on average (source) |

**The honest reading.** Loams' weak point is one hot repository's push rate. Every push to the repository, whatever its branch, waits for the next segment PUT, so the rate is the group size divided by the store's PUT latency. Group commit makes it grow with load (§4.4), and the D391 target of 30 pushes/s is met on RustFS, but Enroute's claim is fifty times Loams' measured figure, and Loams has not measured a cloud store at all. Loams' strengths are full git compatibility together with Scion-style server-side workspaces and history, a bucket as the only durable state, and O(1) forks and workspaces. D1025 addresses the weak point without giving up the strengths.

### 20.3 Decisions (proposed)

| # | Decision (proposed) | § |
|---|---|---|
| D1025 | **A TiKV-backed `RefLog` for hot repositories.** `TikvRefLog` implements D392's `RefLog` with ref transactions ordered by TiKV's Raft, so a push waits for a TiKV commit (milliseconds) rather than a bucket segment PUT. The bucket WAL (D388–D391) stays the default, and the bucket stays the source of truth for the layout: `TikvRefLog` mirrors every log entry into `wal/` with the same sequence numbers and bytes, and checkpoints, packs and workspaces stay on the bucket. The choice is per repository (`RepoConfig.ref_log`: `bucket` by default, or `tikv` where the deployment runs TiKV, which the Loams stack already ships for Live and the metastore). A benchmark task runs a forgemark-style push workload against both adapters | §20.4 |
| D1026 | **`Land` in `WorkspaceService`.** A server-side rebase of a workspace's stack of local commits onto a target ref, then a `RefTxn` compare-and-swap, retried with backoff when another land wins. There is no global merge queue. Conflicts come back as a structured result that an agent can act on. An optional pre-land checks hook runs before the CAS. `Land` is idempotent on a caller-chosen land id (the request's `idempotency_key`) | §20.5 |
| D1027 | **Per-commit object locality.** Packs written at push, save or land time group each commit's new objects (Enroute's segment idea) and carry a table of contents (`.lpk` format version 2), so fetch and `loams-vfs` read a commit's closure with a few ranged GETs and no object-by-object tree walk on the bucket. Objects are also clustered by kind (commits, then trees, then blobs), so `--filter=blob:none` reads one prefix. Compaction stays off the push path with stock `git repack` (D396) | §20.6 |
| D1028 | **A regional cache tier, `loams-git-cache`,** between `loams-vfsd` or CI nodes and the gateway's `ObjectService` and the bucket. It speaks the same RPCs, caches immutable data only, and tiers are strict: a client talks only to its configured cache, and a cache only to the server. On CI nodes, one shared `loams-vfsd` in system mode serves several jobs from one cache | §20.7 |
| D1029 | **Licensing and positioning.** Extends D838. Ideas may be taken from Enroute and `CarterT27/scion` (Apache-2.0; their code may be read), but no code is copied unless a later decision allows it, with attribution. Nothing is taken from `openscion` (GPL-3.0), whose source is not read; Scion's ideas come only from Anthropic's public article. Positioning: **Loams Git keeps full git compatibility while offering Scion-style server-side workspaces, history and landing** | §20.8 |

### 20.4 A TiKV-backed `RefLog` (D1025)

**Why TiKV and not a new consensus layer.** Enroute's ledger is a per-repository Raft log. Loams already runs TiKV (Raft per region, PD's timestamp oracle, Percolator transactions) through `crates/loams-tikv` (R1, D124) for the metastore (D179) and Live (D130). A TiKV transaction commits in milliseconds and is replicated to three zones, so it gives the same property without a second consensus system.

**Shape.** `TikvRefLog` lives in a new crate, `crates/loams-git-tikv`, so `loams-git` keeps no TiKV dependency and stays portable (§5, §17). It reuses the §4.4 sequencer, the state machine (§4.5) and the segment format (D390) unchanged. Only the commit point moves. Concretely, a `TikvWalStore` implements `WalStore` (§5.1): `append` is the TiKV transaction below (it derives the `ref` keys from the batch's `RefTransaction` events), `read_from` reads `log/`, the hint is `tail`, and checkpoints go to the bucket through the repository's `StoreWalStore`. `TikvRefLog` is the existing sequencer over `TikvWalStore`, plus the bucket mirror and point reads for `snapshot`:

| Key (under the Loams keyspace's `git/` root, `loams_tikv::codec::tuple`) | Value |
|---|---|
| `(ns, repo_id, "tail")` | the last committed seq and the checkpoint seq |
| `(ns, repo_id, "log", seq)` | the encoded segment `wal/<seq>.lgw` would hold (D390 bytes, at most 1 MiB) |
| `(ns, repo_id, "ref", name)` | the ref's object id, for point reads without replay |
| `(ns, repo_id, "mirrored")` | the highest seq copied to the bucket |

- **Commit.** The sequencer forms a group as in §4.4 and commits it as one TiKV transaction through `Tikv::run`. The transaction locks `tail` (`get_for_update`), checks that it still equals the sequencer's state, writes `log/<seq+1>`, the changed `ref` keys and `tail`, and commits. The TiKV commit is the commit point, as the segment PUT is in §4.2.
- **Fencing.** A second writer (a zombie owner, a fallback after a forward timeout, Task 14) finds `tail` moved, reads the log entries past its state, applies them and re-validates, exactly as on a 412. An unknown outcome is resolved by reading `log/<seq+1>` and comparing `batch_id`. Correctness still never depends on there being one sequencer (D391).
- **Reads.** `snapshot(Latest)` on any node reads `tail` and the `ref` keys at one TSO timestamp. `watch(from)` reads `log/` in order. Receipts, sequence numbers, consistency tokens and idempotency (§4.5) are unchanged, so callers cannot tell the adapters apart.
- **Throughput.** Pushes to different branches still share one sequence, but a group now waits about one TiKV commit (single-digit milliseconds on a local cluster (verify)) instead of one segment PUT. Per-ref transactions with no shared `tail`, ordered by commit timestamp, would remove the shared key too. They are not built unless Task 89 shows `tail` is the limit (Q906 records the default).
- **The bucket stays complete.** A mirror task beside the sequencer copies each `log/<seq>` into `wal/<seq:020>.lgw` with a create-only PUT and the same bytes, then advances `mirrored`. The checkpointer keeps writing `checkpoints/` from the sequencer's state. TiKV log entries are deleted only once they are both mirrored and below the newest checkpoint. Any bucket reader (`git-remote-loams` fetch, `state_at`, the restore drill, Task 77) therefore sees the repository, at most `TIKV_MIRROR_LAG` (1 s (target)) behind. Packs, workspaces, derived data and the `_git` event mirror (§4.8) do not change.
- **Acknowledgement.** A push is acknowledged after its pack PUT and the TiKV commit, not after the mirror. For a repository that opts in, this amends goal 2 of §2.1 and D1 the way D130 does for Live: the refs' source of truth is TiKV until mirrored. Q906 asks the owner to confirm it.
- **Switching adapters.** `bucket` to `tikv`: the owner sequencer commits a handover event through the bucket WAL, of a new type `io.loams.dev.git.handover.v1` whose data is `RefLogHandover { RefLogKind to = 1; }` (`BUCKET = 0`, `TIKV = 1`). It then writes `tail` at that seq into TiKV and commits from there. A current bucket writer that reads the handover refuses to append, and `git-remote-loams` answers `fatal: repository '<address>' is ordered by a Loams server; push through loams://<host>/…`. A reader built before the handover type existed refuses the segment, because the event reader refuses an unknown `type` (`format/event.rs`), so an older helper stops rather than writes past it. `tikv` to `bucket`: stop accepting, let the mirror catch up, commit the reverse handover through TiKV (mirrored like any entry), and continue on the bucket WAL from the next seq.
- **Selection.** `RepoConfig.ref_log` is set at `CreateRepo` or by `UpdateRepoConfig` (admin access). `tikv` is refused with the reason `ref_log_unavailable` (`failed_precondition`) where the deployment has no TiKV, which includes `loams dev`, the desktop and every local-first `DirectBackend`. The default stays `bucket` everywhere.
- **The benchmark** (Task 89, `bench/git-forge`) is a forgemark-style workload written from Enroute's published description, not its code: N concurrent pushers, each pushing a small commit to its own branch of one repository over Smart HTTP, with N from 1 to 512. It runs against both adapters on the same machine (RustFS with and without 100 ms added per PUT; a three-node TiKV in compose) and records pushes/s, p50 and p99. Targets for the TiKV adapter on the reference machine: **at least 1,000 pushes/s at 512 pushers with p99 under 250 ms, and a p50 under 20 ms for one pusher (target)**. The bucket adapter's figures are recorded, not gated. Enroute's 7,619 pushes/s ran on EC2 and S3 across machines, so it is cited, not used as a target.

### 20.5 `Land` (D1026)

`Land` turns §19.7.5's pull-then-push into one server-side call with retries, which is what Scion's server-side merges give agents. It extends D834:

```proto
rpc Land(LandRequest) returns (LandResponse);
message LandRequest {
  string repo = 1; string ws_id = 2; uint64 expect_n = 3;   // the snapshot whose stack lands; 0 = latest
  string target_ref = 4;                                    // "refs/heads/main"
  LandStrategy strategy = 6;                                // REBASE (default) | FAST_FORWARD_ONLY
  bool run_checks = 7;                                      // the repository's pre-land hook, if configured
  google.protobuf.Duration deadline = 8;                    // default LAND_DEADLINE (60 s)
  string idempotency_key = 15;                              // AP0; this is the land id
}
message LandResponse {
  oneof outcome { Landed landed = 1; LandConflicts conflicts = 2; ChecksFailed checks_failed = 3; }
}
message Landed { uint64 seq = 1; bytes new_tip = 2; string consistency_token = 3; uint32 attempts = 4; bool workspace_updated = 5; }
message LandConflicts { bytes target_tip = 1; bytes at_commit = 2; repeated LandConflict conflicts = 3; }
message LandConflict { string path = 1; ConflictKind kind = 2; bytes base = 3; bytes ours = 4; bytes theirs = 5; uint32 mode = 6; repeated string env_keys = 7; }
// ConflictKind: CONTENT, MODIFY_DELETE, ADD_ADD, MODE, ENV_KEY (D845: keys and fingerprints, never values)
```

1. Read the workspace at `expect_n`. Its **stack** is the commits from `base` to `head`.
2. Read `target_ref`'s tip `T` at `snapshot(Latest)`. If `T` equals `base`, the stack fast-forwards. Otherwise replay each commit of the stack onto `T` with the clean-room `merge3` (Task 26), touching only the paths that changed on either side. The author is kept; the committer is the landing principal, at the land's start time.
3. A conflict stops the land and returns `LandConflicts`, naming the stack commit that failed. Nothing is written. The agent fixes the conflict in its workspace (`Rebase` writes the conflicts there, §19.10) and lands again.
4. If `run_checks` is set and the repository configures a pre-land hook, call it with the rebased tip and its changed paths, within `LAND_CHECK_TIMEOUT` (5 min). A failure returns `ChecksFailed` with the hook's report. The hook is an HTTPS webhook signed like D843's, or an in-process `LandCheck` for embedders. It is **off by default**: testing is post-merge, as Scion's is.
5. Write one `.lpk` with the new commits and the closure the repository lacks (D831's assembly, D1027's layout), then commit a `RefTxn` with `Expect::Oid(T)` through the repository's sequencer. Protection, scope, secret scanning and env sealing are checked as in `PushWorkspace`. A land is always a fast-forward of the tip it read, so `deny_force` does not refuse it.
6. On `Stale` (another land or push won), wait with jittered exponential backoff (10 ms doubling to 1 s), read the new tip and repeat from step 2, re-merging only commits whose paths the new tip changed. Give up after `LAND_MAX_ATTEMPTS` (16) or the deadline with `aborted` (reason `land_contended`). Checks re-run after a retry only when the repository sets `land.rerun_checks_on_retry` (default off).
7. On success, if the workspace is still at `expect_n`, append a snapshot with `base = head = new_tip` and the local branch moved, fenced like any save (`workspace_updated: true`). If it has moved on, leave it: the writer's next pull rebases onto the landed commits.

- **Idempotency.** The `RefTxn`'s key is `land:` plus SHA-256 over the principal and the land id, and its request digest covers `(repo, ws_id, expect_n, target_ref, strategy)`, never the rebased tip, which changes between attempts. A retry within the 1 h window (§4.5) is answered with the original receipt, and `Land` reads the ref at that seq to return the landed tip. A create-only land record, `ws/<ws_id>/land/<sha256(land id)>.json`, keeps the outcome for `LAND_RECORD_RETENTION` (7 days) past the window. The same land id with another request is `idempotency_key_reused`.
- **No merge queue.** Lands contend only at the CAS. The cost is a re-merge per lost race, which touches only the changed paths. Q392's answer (a queue in a later phase, on demand) stands for a queue; `Land` is not one. Q907 asks whether a branch should be able to require it.
- **Mount-less agents** (§19.11) land with `ApplyEdits` then `Land`, and never fetch the tree. The shim maps nothing to `Land` at GA: `git push` keeps its git semantics, and `loams ws land` (CL1's vocabulary) is the command.

### 20.6 Per-commit object locality (D1027)

**The problem.** A fetch of one new commit, or `loams-vfs` reading a fresh commit's trees, resolves object by object through the idx (§5.3, Task 17), which costs a ranged GET per scattered object on a cold cache. Enroute's segments put a commit's objects together and index them, so the read is a few ranges.

**`.lpk` format version 2** adds a table of contents between the idx and the footer:

```
pack ‖ idx ‖ toc ‖ footer v2 (40 bytes: magic "LGITLPK2" | pack_len u64 | idx_len u64 | toc_len u64 | crc32c u32 | reserved u32)
```

```protobuf
message ByteRange { uint64 offset = 1; uint64 len = 2; }      // within the pack section
message CommitExtent {
  bytes commit = 1;
  repeated ByteRange trees = 2;            // the trees this commit introduces, coalesced
  repeated ByteRange blobs = 3;            // the blobs this commit introduces, coalesced
  repeated ByteRange external_bases = 4;   // delta bases in this pack outside the commit's own ranges
}
message PackToc {
  ByteRange commits = 1;  ByteRange trees = 2;  ByteRange blobs = 3;   // kind clusters, when contiguous
  repeated CommitExtent extents = 4;       // in topological order, oldest first
}
```

- **Packs Loams writes** (workspace saves, `PushWorkspace`, `Land`, compaction's own merges of workspace packs) are written in the order *all commits, then trees grouped by the commit that introduces them, then blobs grouped the same way*. Each commit's trees and blobs are then one range each, and the kind clusters are three ranges. An object belongs to the oldest commit in the pack that reaches it. Delta bases precede their deltas, as ofs-deltas require, and a base outside the commit's ranges is listed in `external_bases`, so a reader knows every range it needs without parsing entries.
- **Packs Loams receives** (stock `git push`, the helper) are stored exactly as received (D390, Q908's default). git's `pack-objects` already writes commits, then trees, then blobs (its write order (verify)), so the kind clusters usually hold. The receive path already walks each new commit's closure to check connectivity (§6.3 step 3, `missing_from_closure`), so it builds the per-commit extents in that walk, with no second pass, and writes the TOC in the same create-only PUT. Ranges may be fragmented. That is correct, only less compact.
- **Readers.** `ObjectService.GetTrees` for a commit that has an extent reads its tree ranges in one coalesced GET. `upload-pack` serving wants whose haves include their parents streams the extents' ranges in pack-copy mode (§6.1); a delta whose base is not sent is converted to a ref-delta for a client that has the base, or resolved, as the assembler already does. `filter=blob:none` reads the commit and tree clusters only. `loams-vfs` prefetch (§19.5.1) reads a new commit's trees as one range. A pack without a TOC (format version 1, or a damaged TOC) falls back to idx lookups, so the TOC is an optimisation and never needed for correctness.
- **Compatible with D396.** The pack section stays a valid git pack and the idx stays version 2, so compaction's local mirror splits `.lpk` files into `pack-<checksum>.pack` and `.idx` as before and ignores the TOC. Stock `git repack --geometric=2` stays the only repacker, never on the push path. Geometric repacking leaves the youngest small packs alone until they roll up, and the youngest commits are the ones CI and agents read, so locality is kept where reads concentrate. When the compactor uploads a pack that git wrote, it computes that pack's TOC from the new pack's commits and idx (on the worker, beside the commit-graph step) and writes it in the `.lpk`, so older history keeps its kind clusters and gets extents, possibly fragmented. Readers accept versions 1 and 2 (§03 §6's N and N−1 rule), and existing version 1 packs gain a TOC only when compaction rewrites them.
- **Not taken from Enroute:** "deltas only against the same path in a parent". Loams cannot write deltas (gitoxide has no delta compression, §6.1), so Loams-written packs have none beyond those copied from stored packs, and stock git chooses the deltas of the packs it writes.

### 20.7 A regional cache tier (D1028)

`loams-git-cache` is Scion's `scion-cache` for Loams: a stateless read-through cache in a region or site, between hosts and the server.

```
loams-vfsd (per host; per CI node in system mode) ──► loams-git-cache (per region or site) ──► gateway: ObjectService ──► bucket
```

- **Same RPCs.** It serves `ObjectService` (`GetObjects`, `GetTrees`, `HasObjects`, `Log`, `Blame`, `Grep`, `DiffStat`) and the immutable object route (`GET …/loams/objects/<oid>`, §19.5.1). A client points its `ApiBackend` at the cache instead of the gateway, with no other change.
- **Immutable data only.** It caches responses keyed by content: objects by id, tree aux by tree id, blame by (commit, path), derived graph layers by seq, pack ranges by pack checksum. It forwards everything mutable or ref-relative (`HasObjects` misses, history by ref name, workspace and snapshot calls, saves, pushes, `Land`) to the server without caching. A positive `HasObjects` answer is cached, because objects are never removed while reachable; a negative one is not.
- **Strict tiering.** vfsd talks only to its configured upstream, and the cache talks only to the server. Neither skips a tier when the next one fails: a client fails over to another cache in the same tier, or waits. A cache outage therefore cannot shift a region's whole read load onto the server, which is the reason Scion gives. The server limits each cache's request rate.
- **Access.** The cache decides nothing by itself. It asks the server once per (token, repository, workspace) and keeps the answer for `CACHE_AUTHZ_TTL` (60 s). Entries are partitioned by namespace and fork family (§4.7), and objects from a workspace pack by workspace, so a hit is served only to a caller the server has authorised for that partition, and nothing is shared across tenants (§8.3's rule).
- **Shared node daemon for CI.** On a CI node, one `loams-vfsd --system` serves every job: each job mounts its own workspace with its own scoped token through the control socket (its group `loams-ci`, D788's peer checks), and all jobs share one content-addressed cache (§19.6.2). The node daemon warms its cache at boot from a prefetch profile (`loams vfs prefetch --profile ci`, §19.5.1), as Scion's daemon starts from a snapshot.
- **Storage.** Memory first, then a local disk (an `Fs`, §17), LRU, both rebuildable (D1). Target: a hit rate of at least 99% at each tier for a CI fleet checking out recent tips (target).
- **Not GT5.** GT5's NVMe replicas hold whole repositories and could accept writes. This cache holds immutable objects only and never orders anything.

### 20.8 Licensing and positioning (D1029)

| Project | Licence | What Loams may do |
|---|---|---|
| Enroute | Apache-2.0 | Read its documentation and source and take ideas. Copy no code unless a later decision allows it, with attribution in `NOTICE` |
| `CarterT27/scion` | Apache-2.0 | The same as Enroute |
| `tinyhumansai/openscion` | GPL-3.0-only | Nothing. Its source is not read for Loams work. Scion's ideas come only from Anthropic's article |
| Scion | Closed | Ideas from the public article only |
| walgit (`tobi/walgit`) | MIT (source) | Reference only; a Continuity-style design that §3 already covers |

This extends D838's rule to these projects. Every PR implementing a §20 idea cites the public document it worked from. **Positioning:** *Loams Git keeps full git compatibility while offering Scion-style server-side workspaces, history and landing.*

### 20.9 Conflicts with earlier decisions

| Earlier | §20 | Resolution |
|---|---|---|
| D1 and §2.1 goal 2 (a push is acknowledged when pack and ref update are in the bucket) | D1025: acknowledged at the TiKV commit, mirrored within about 1 s | Amended only for repositories that opt in, as D130 amends D1 for Live. The bucket default is unchanged. Q906 |
| D389, D391 (the segment PUT is the commit point; fencing on the segment name) | D1025 | For a `tikv` repository, the TiKV transaction on `tail` is the commit point and the fence. The bucket WAL is a mirror with identical segments. D390's event types gain `io.loams.dev.git.handover.v1` |
| D395 (the serverless helper writes the bucket directly) | D1025 | The helper refuses to push to a repository ordered by TiKV; fetch is unchanged |
| D397 and Q391 (no per-scope WAL partitions until contention is measured) | D1025 | Unchanged: one log per repository. A faster log, not more logs, is the answer for hot repositories |
| Q392 (a server-side merge queue in a later phase, on demand) | D1026 | `Land` is server-side rebase and CAS without a queue. A queue stays unplanned |
| D390 (`.lpk` = pack ‖ idx ‖ footer; packs stored as received) | D1027 | Format version 2 adds a TOC; received bytes stay verbatim; Loams-written packs are ordered per commit |
| D396 (compaction by stock `git repack`, off the push path) | D1027 | Kept. The compactor adds a TOC to the packs it uploads |
| D838 (licences) | D1029 | Extended to Enroute, `CarterT27/scion`, `openscion` and walgit |
| §3 (reference systems) | §20.1 | Enroute and Scion are compared here rather than in §3's table |

### 20.10 Open questions

| # | Question | Default | Owner | Needed by |
|---|---|---|---|---|
| Q906 | **TiKV adapter acknowledgement.** Acknowledge a push to a `tikv` repository at the TiKV commit (three-zone Raft, mirrored to the bucket within about 1 s), or wait for the mirror too? Waiting for the mirror would bring back the segment PUT latency. | At the TiKV commit, amending D1 for opted-in repositories as D130 does for Live. Per-ref commits without the shared `tail` key are built only if Task 89 shows `tail` is the limit | Founder | GT1 Task 88 |
| Q907 | **Requiring `Land`.** Should a protection rule be able to refuse direct pushes to a branch and accept only `Land` (so every change is rebased on the server)? | Yes, as a `land_only` flag on `Protection`, off by default | Founder | GT1 Task 90 |
| Q908 | **Received packs.** Rewrite pushed packs into per-commit order (CPU on the push path, and no longer the bytes received), or store them verbatim with a TOC? | Verbatim with a TOC (D390 unchanged); only Loams-written packs are reordered | Eng | GT1 Task 91 |
| Q909 | **Where the cache tier runs.** As its own optional binary and image, one per region or self-hosted site, or as a role of the `loams` binary? | Its own binary and image (`loams-git-cache`), optional in the compose stack and off in `loams dev` | Eng | GT1 Task 92 |

### 20.11 Sources for §20

Read on 2026-10-16.

- Enroute: https://enroute.sh; https://enroute.sh/blog/everyone-is-doing-git-on-s3-wrong (2026-10-13: ledger, segments, layers, delta rule, inline layers and flush, buffer pool, forgemark figures, comparison with git 2.56.0, the critique of WAL-on-S3 designs, what is unbuilt); https://github.com/enroute-sh/enroute (README; Apache-2.0; Rust); https://github.com/enroute-sh/enroute/releases and the GitHub releases API (v0.2.0 2026-09-25T16:13Z, v0.3.0 2026-10-14T11:15Z). All Enroute figures are the vendor's, not reproduced.
- Scion: Anthropic Engineering, "Redesigning source control at Anthropic", https://www.anthropic.com/engineering/redesigning-source-control (2026-10-13, updated 2026-10-15). All figures are Anthropic's.
- https://github.com/CarterT27/scion (Apache-2.0; README: CockroachDB and S3-compatible storage, v1 scope, git protocol compatibility out of scope); https://github.com/tinyhumansai/openscion (GPL-3.0-only per the GitHub API; only its README and licence were read, to classify it).
- walgit: https://github.com/tobi/walgit (MIT and a Continuity-style manifest CAS, per search summaries; not read).
- Repository: §4.4's measurements (`bench/results/git-wal/20261012T0955Z-gt1-t11.jsonl`), `crates/loams-tikv` (R1, D124), D130, D179, D838.
