# 85 — Desktop: Resumable Transfers and Job Streams over QUIC

Status: **Approved** · 2026-10-29. The owner asked for this on 2026-10-29 and approved the defaults, as relayed by the coordinator.
- Decisions **D2420–D2443** are in the [decision log](13-decision-log.md) with status **Approved**.
- Questions **Q1510–Q1517** are answered with their defaults (§16).
- D2444–D2449 and Q1518–Q1519 are kept free for the plan's rulings.

Plan: [DQ1](../plans/2026-10-29-dq1-desktop-quic-transfer.md). Status label: **preview** until DQ1's gates (§15) pass.

Source: the owner, 2026-10-29: "in intelligence repo there is a single mjs script that reserved 4gb for node that [loads] 1 billion events to tiger db, use that concept in electron app to stream 10 gb file or run ci etc over quic".

Builds on [§83](83-cluster-transport-http3-mtls.md) (HTTP/3 over QUIC with mTLS, plan CT1), [§03](03-storage-formats.md) §3.3 (immutable objects committed by a fenced pointer compare-and-swap), [§18](18-metastore-backends-and-router.md) (the metastore's leases and pointers) and the desktop app ([§37](37-desktop-and-mobile-apps.md)).

Markers, as in §83 and §84:
- **(verify)**: not yet checked against running code. The task that depends on it checks it first.
- **(target)**: a number this document sets as a gate.

---

## 1. Summary

Loams Desktop gets one Rust sidecar, `loams-transfer`, that moves large files to and from a Loams deployment and follows remote job runs. The Electron main process drives it over stdio and shows progress. The sidecar speaks **HTTP/3 over QUIC** to the deployment and falls back to **HTTP/2 over TLS 1.3** when UDP is blocked (§83 §8).

The protocol takes its shape from a single-worker seeding script the owner pointed at (§2). It has five properties:
- **Idempotent writes.** Every chunk has a deterministic key that includes its hash, so sending a chunk twice changes nothing.
- **A checkpoint that commits with the data.** A chunk counts only once a metastore pointer that names it has been advanced. That pointer move is the commit.
- **A single writer.** A metastore lease, carried as a fence on every commit, replaces the script's advisory lock.
- **Bounded memory.** Files stream from disk in fixed-size chunks. A window bounds the chunks in flight, and nothing holds the whole file.
- **Expansion, never deletion.** A resume never removes committed data. A transfer whose source changed is refused, not overwritten.

The same journal carries **job runs**: a runner appends log segments and artifact references under its lease. A desktop that disconnects resumes the log at the byte offset it last received.

## 2. The concept: `seed-tigerdata.mjs`

The owner's reference is a private seeding script, `deploy/local/seed-tigerdata.mjs`. One Node process loaded about 10⁹ rows into a Postgres-compatible database with a 4 GB heap. The script itself stays private; this design takes only its shape:

| In the script | Here |
|---|---|
| A deterministic key per row (`seed_event_key`) plus a unique index, so `INSERT … ON CONFLICT DO NOTHING` makes a retry harmless | A deterministic chunk key `…/chunks/<index>-<sha256>` (D2424). Re-sending a committed chunk is a no-op (D2427) |
| A progress table (`next_event`) advanced **in the same transaction** as each batch | A metastore pointer per transfer, advanced by compare-and-swap. The swap is the commit, and the chunks become visible only through it (D2425) |
| A session advisory lock: a single writer | A metastore lease per transfer, and the lease's fence on every commit (D2428) |
| Fixed-size batches, with progress and throughput logging | Fixed-size chunks (8–64 MiB), a window of chunks in flight, and progress events with throughput and ETA (D2423, D2436) |
| Target expansion only, never deleting | A resume never deletes or rewrites committed chunks. A mismatched size, chunk size or chunk hash is refused (D2426) |
| A 4 GB Node heap | **Not copied.** Memory is bounded by the window, not by the file (D2432) |

## 3. Scope

In scope:
- Uploading a local file of 10 GB or more to a namespace's transfer area in the deployment's bucket, and downloading one back, resumably and verified byte for byte.
- Job runs: starting a named job on a remote runner, then streaming its log and artifacts back to the desktop, resumably.
- The desktop's progress UI: throughput, ETA and resume state.

Out of scope:
- **A general file browser.** The desktop lists transfers and runs. A Files product is a later design (Q1515).
- **Peer-to-peer or desktop-to-desktop transfer.**
- **Arbitrary remote commands.** A run names a job that the runner's operator configured (D2440).
- **Replacing CT1.** Cluster-internal hops stay §83's. This design adds a client-facing path, and reuses CT1's stack and fallback rules.

## 4. Architecture

```
Electron renderer (console plugin "Transfers")
        │  contextBridge IPC (CH.transfer*)
Electron main ── spawns, supervises ──► loams-transfer agent   (Rust sidecar, JSON lines on stdio)
                                            │  HTTP/3 over QUIC (h3), or HTTP/2 + TLS 1.3 fallback
                                            ▼
                       loams engine: /v1/namespaces/{ns}/transfers/*, /runs/*
                       ├─ metastore: lease  transfer/<id>,  pointer  transfer/<id>   (the checkpoint)
                       └─ bucket (loams-store): chunk objects and journal segments
```

- **The sidecar** (D2420) is a small Rust binary built from `crates/loams-transfer`. Electron's Node has no stable QUIC, and Rust gives `quinn`, `h3` and `rustls`, the stack CT1 chose (D2305). The desktop already ships the `loams` engine binary. `loams-transfer` ships beside it, so no new runtime enters the app.
- **The server side** (D2421) lives in the engine's main HTTP router, next to the other `/v1/namespaces/{ns}/…` routes. It uses the engine's existing `MetaStore` and `Store`, and the engine's principals for authorization. The logic is in `loams-transfer`'s `server` module, so the engine only mounts it. For tests and dev there is also a standalone `loams-transfer serve`, with a single-node metastore and a local or S3 bucket.
- **The gateway is not involved** in v1. It fronts client wire protocols (§57). Transfers are the engine's own HTTP API, and the engine's main port already speaks TLS (`LOAMS_TLS_CERT_FILE`). A gateway route can come later (Q1514).

## 5. Storage layout

All keys are under the namespace's id:

```
transfers/<ns>/<tid>/chunks/<index:08>-<sha256>        one chunk's bytes (immutable)
transfers/<ns>/<tid>/journal/<seq:08>-<epoch>.json     a journal segment (immutable)
runs/<ns>/<rid>/log/<start:016>-<sha256>               one log segment's bytes (immutable)
runs/<ns>/<rid>/journal/<seq:08>-<epoch>.json          the run's journal segment
metastore: lease   transfer/<tid> · run/<rid>
metastore: pointer transfer/<tid> · run/<rid>          (namespace-scoped; ≤ 1 KiB value)
```

### 5.1 The journal (D2425)

The metastore's pointer values are at most 1 KiB (`MAX_KEY_LEN`), and pointers are never deleted. So the pointer holds a small head, and the list of committed entries lives in immutable journal segments in the bucket:

- A **segment** is an immutable JSON object: `{seq, prev, snapshot, entries[]}`. `prev` is the previous segment's key. Every 64th segment is a **snapshot** that repeats every entry committed so far, so a reader never walks back more than 64 segments.
- The **pointer** value is `{v, seq, head, n, bytes, state}`: the head segment's key, the committed entry count, the committed bytes and the state (`open`, `complete`, `abandoned`).
- **To commit**: write the segment with `put_if_absent` under a key that includes the lease epoch. Then `cas_pointer(expected = seq, value = new head, fence = {lease, epoch})`. The swap is the commit. A segment that a fenced-out writer left behind is unreferenced, and garbage collection removes it.
- **Group commit.** One server process serialises a transfer's commits behind an in-process lock. Chunks that finish together go into one segment and one swap.

This is the metastore's own discipline: immutable objects, then a fenced pointer compare-and-swap (§03 §3.3, the collection manifest). It works the same on a local folder, RustFS and S3, because it never needs the bucket's conditional overwrite. A local folder has none.

### 5.2 Chunk keys (D2424)

A chunk's key is its index plus its SHA-256. Two sends of the same chunk write the same bytes to the same key. A chunk whose bytes differ (the source file changed) gets a different key, so a committed chunk is never overwritten.

## 6. The transfer protocol (D2422)

Every route is under `/v1/namespaces/{ns}/transfers`. Request and response bodies are JSON, except chunk bodies, which are raw bytes.

| Route | What it does |
|---|---|
| `POST /` `{name, size, chunk_size, client_id, file_sha256?}` | Create a transfer. Returns `{tid}`. Idempotent per `(client_id, name, size, chunk_size)` within the open set: the client keeps `tid` in its local state anyway |
| `GET /{tid}` | The checkpoint: state, `size`, `chunk_size`, `committed` (a run-length list of index ranges), `bytes`, the lease holder and its deadline |
| `POST /{tid}/lease` `{holder}` | Acquire or renew the lease. Returns `{epoch, deadline_ms}`. 409 `lease_held` names the holder and its deadline |
| `PUT /{tid}/chunks/{i}` + headers `Loams-Chunk-Sha256`, `Loams-Lease-Epoch` | Stream the chunk's bytes in. The server hashes while it writes, refuses a mismatch (422), then group-commits. Returns the new checkpoint summary. A committed index with the same hash returns 200 at once, with no write. One with another hash returns 409 `chunk_conflict` |
| `POST /{tid}/complete` `{file_sha256, epoch}` | Requires every chunk to be committed. Writes the final snapshot with the chunk-hash root (§9) and the client's whole-file hash, then swaps the pointer to `complete`. Idempotent |
| `GET /{tid}/chunks/{i}` | Stream one committed chunk back, with `Loams-Chunk-Sha256` |
| `GET /` | List the namespace's transfers (pointers with the prefix `transfer/`) |
| `POST /{tid}/abandon` | The owner gives up: the state becomes `abandoned`, and the bytes go after the retention window (Q1513) |

Limits (D2423): `chunk_size` is a power of two from 8 MiB to 64 MiB, and the default is 16 MiB. The client picks the smallest size that keeps a transfer at or under 16,384 chunks, which allows up to 1 TiB at 64 MiB. Larger files are refused in v1 (Q1511).

## 7. The client algorithms

### 7.1 Upload

1. **Local state.** The sidecar keeps `<state>/transfers/<tid>.json` (`tid`, path, size, mtime, chunk size and server), and takes an exclusive file lock on `<state>/agent.lock`. That is the local half of the single-writer rule.
2. **Create or resume.** Create the transfer, or read the checkpoint of the one in the local state. Size, chunk size or mtime that do not match the local file stop the transfer: the file changed, so it starts over as a new transfer (D2426).
3. **Lease.** Acquire it, with the holder `<install-id>`, and renew it at a third of its 30 s TTL (D2428).
4. **Pipeline.** Walk the uncommitted chunk indexes in order, with up to `window` chunks in flight (default 8, D2431). For each chunk:
   - **Pass 1:** read the range from disk in 1 MiB blocks and hash it.
   - **Pass 2:** open a request stream and send the same range from disk in 1 MiB blocks, with the hash in the header. The page cache usually serves pass 2.
   Each chunk is its own HTTP/3 request, so its own QUIC stream. A lost packet stalls one chunk, not the others.
5. **Whole-file hash.** A separate sequential reader hashes the entire file beside the pipeline. On a resume it re-reads the committed prefix from local disk, which is fast and keeps the hash honest.
6. **Complete** with the whole-file hash. Then the local state file goes.

On any error the chunk is retried with backoff (1, 2, 4 … 30 s). The checkpoint says what to skip, so a crash at any point costs at most the chunks that were in flight.

### 7.2 Download

1. **The target file.** Read the checkpoint, which must be `complete`. Create `<dest>.loams-part` at the full size, as a sparse file, plus a local checkpoint `<dest>.loams-part.json` that lists the verified chunk indexes.
2. **Pipeline.** Fetch the chunks that are not verified yet, `window` at a time. Stream each one into the part file at its offset (`pwrite` in 1 MiB blocks), hashing as it goes. A mismatch discards that chunk and retries it.
3. **The local commit.** After every few chunks: `fdatasync` the part file, then rewrite the local checkpoint through a temporary file and a rename. The data is durable before the checkpoint says so. This is the script's "same transaction", on one machine.
4. **Finish.** Hash the whole part file sequentially, and compare against `file_sha256`. On a match, rename it to `<dest>`.

## 8. Leases and fencing (D2428–D2430)

- **The holder** is the desktop's install id. The exclusive file lock (§7.1) guarantees one live sidecar per install, so the metastore's rule of a unique owner per process incarnation holds. A sidecar that restarts after a crash takes its own lease again at once. Re-acquiring an owned lease keeps its epoch (§18).
- **Another desktop** gets `lease_held` until the lease expires, 30 s after the holder's last renewal. Then it takes over at the next epoch, and the old holder's commits are fenced out (D2429).
- **Zombie requests.** A chunk PUT from a process that has died can still finish on the server. It writes the same bytes to the same key and commits the same entry, so it is harmless. That is why chunk keys carry the hash (D2424).
- **The server never trusts the client's view.** Every commit carries `Loams-Lease-Epoch`. The swap's fence rejects a stale epoch, and the server answers 409 `lease_lost`.

## 9. Integrity (D2433, D2434)

- **Per chunk:** SHA-256. The client sends it, the server checks it while writing, and the reader checks it while receiving.
- **Per transfer:** the root, a SHA-256 over the ordered list of `(index, length, chunk sha256)`. The server computes it at `complete`, from the journal alone, without re-reading the bytes.
- **Whole file:** SHA-256 over the file's bytes. The uploader computes it, the server stores it, and every downloader checks it before the final rename. So "byte-identical" is checked end to end, by a party other than the writer.
- **`?verify=full`** on `GET /{tid}` makes the server stream every chunk and recompute both hashes. It is an operator tool and is not on the hot path.

## 10. Transport (D2435–D2438)

- **HTTP/3 over QUIC** by default: `quinn` 0.11.12, `h3` 0.0.8 and `h3-quinn` 0.0.10 on `rustls` 0.23 with aws-lc-rs, the pins CT1 Task 0 made.
- **Fallback, as in §83 §8 (D2317).** Race QUIC against TCP+TLS: QUIC starts first, and TCP starts after 250 ms. The sidecar remembers per server that UDP failed for 15 minutes, in its state directory. `mode = auto | h3 | h2` can be set per server.
- **One port.** The engine serves HTTP/3 on UDP at the same port number as its TLS main listener. HTTP/2 on TCP is the main listener itself, so the fallback needs no new listener. A local engine on loopback without TLS is reached over plain HTTP/1.1. That is the only plain path, and it is never used for a remote host (D2437).
- **Bulk connection.** Transfers use their own QUIC connection, so they never block the console's small RPCs (D2319). The QUIC receive windows are 16 MiB per stream and 128 MiB per connection (target, retuned by CT1 Task 11).
- **TLS trust.** The server's certificate chain is checked against, in this order: a CA bundle configured for that server; the deployment's CA, once CT1's CA service exists; then the system roots. A dev engine can be pinned by certificate SHA-256 instead (D2438).
- **Authentication** is the user's API key as a bearer token, the same as the engine's REST API. mTLS device certificates are later work (Q1512). `loams-transport`'s `Endpoint` and `Client` (CT1 Task 7) replace the sidecar's own HTTP/3 and HTTP/2 code when they land. DQ1 keeps that code in one module so the swap is local.

## 11. Job runs (D2439–D2442)

The same journal, with other entries:

- **Runs.** `POST /v1/namespaces/{ns}/runs {job, params}` queues a run. Its pointer is `run/<rid>`, with the state `queued`.
- **Runners.** A runner is `loams-transfer runner --jobs jobs.toml` on a remote host, such as a CI machine or a self-hosted node. `jobs.toml` names the jobs, each with its argv, working directory, environment allowlist, timeout and artifact globs. A run can only name one of those jobs, and its `params` become environment variables from the allowlist only (D2440).
- **Claiming.** A runner takes a run by acquiring lease `run/<rid>` and swapping the pointer from `queued` to `running` under that fence. Only one runner can win.
- **Logs.** The runner spools the job's stdout and stderr to a local file. It appends segments of up to 1 MiB or 1 s: each segment is written as an object, then committed as a journal entry `{start, len, sha256, key}` under the fence. If the runner's view of the log length is wrong, the server answers 409 with the committed length, and the runner resends from there. So an append is idempotent by offset (D2441).
- **Tailing.** `GET /runs/{rid}/log?from=N` streams the committed bytes from offset N, then waits for more until the run ends. The desktop stores the offset it has received, so after a disconnect it asks again from there (D2442).
- **Artifacts** are uploaded by the runner as transfers, using §7.1, and referenced from the run's journal. The desktop downloads them with §7.2.
- **The final state** (`succeeded`, `failed` with its exit code, `cancelled` or `timed_out`) is the run's last journal entry. `POST /runs/{rid}/cancel` sets a flag that the runner reads on renewal.

## 12. The desktop (D2420, D2436, D2443)

- **The sidecar's process.** Electron main starts one `loams-transfer agent --state <userData>/transfers` when the first transfer starts, and restarts it with backoff, like the engine supervisor. Commands are JSON lines on stdin (`upload`, `download`, `pause`, `resume`, `cancel`, `list`, `run.start`, `run.tail`). Events are JSON lines on stdout (`progress`, `state`, `log`, `error`).
- **Secrets.** The API key is stored with Electron `safeStorage` and passed in a command's body on stdin. It never goes in argv or the environment.
- **On restart, nothing is lost.** Unfinished transfers in the state directory show as **Paused, resumable from 4.2 GB**, and resume on a click, or automatically if the user turned that on.
- **The UI** is a console plugin, `@loams/plugin-desktop-transfers`, with a "Transfers" page:
  - each transfer's bytes done and total, throughput (a 5 s moving average), ETA, `h3` or `h2`, chunks in flight and retries;
  - the resume state: fresh, resumed from N bytes, paused, or lease held elsewhere;
  - runs, with a live log view that follows the offset, and their artifacts.
  Progress events are throttled to 4 per second.

## 13. Memory and throughput bounds (D2431, D2432)

- **Client memory** is at most `window × 2 MiB` of I/O buffers, plus the HTTP/3 send buffers, whatever the file size. The default is under 64 MiB (target, measured in DQ1 Task 3). There is no large Node heap. Electron main sees only progress events.
- **Server memory** per chunk in flight is one multipart part buffer (at least 5 MiB, `loams-store`'s minimum) plus the hasher. A per-server semaphore, `transfer.max_inflight_chunks` (default 64), bounds the total.
- **Throughput target:** a 10 GiB upload over loopback HTTP/3 at 300 MB/s or more on the dev laptop. Over a 1 Gbit/s tailnet link it should come within 15 % of `iperf3`'s figure (target, measured in DQ1 Task 7).

## 14. Security (D2443)

- **Authorization.** Every route needs a principal with namespace access: `writer` to create, write, abandon and start a run, and `reader` to read and download. Runners use a service API key with the `runner` relation on the namespace.
- **Isolation.** A transfer id is a ULID, and it is scoped to its namespace in every key.
- **Untrusted input.** Paths in `name` are labels only: the server never uses them as filesystem paths, and the downloader writes only to the destination the user picked.
- **Runner isolation.** A runner runs only jobs from its own `jobs.toml`. It runs them under the OS user it was started as, and inside `loams-sandbox` once SB1 lands. Logs can hold secrets, so readers need the same namespace access as for the run.

## 15. Testing and gates

- **Unit tests** of the journal, the checkpoint and the lease fence, against the in-memory store and an in-memory metastore.
- **Kill and resume (target, Task 3).** Generate a 10 GiB sparse source file with patterned non-zero blocks, so it is not all zeros, never materialised in memory. Upload it through `loams-transfer serve` on a local-folder bucket, `SIGKILL` the sidecar midway, start it again, and resume. Then download it the same way, with a kill in the middle. **Pass:** the downloaded file's SHA-256 equals the source's, and `cmp` reports no difference. CI runs the same test at 256 MiB. The 10 GiB run is `#[ignore]` and runs locally, recorded in the plan. The test data is deleted afterwards.
- **Fallback.** With UDP blocked to the port, the transfer completes over HTTP/2, and the sidecar's state remembers that UDP failed.
- **Lease.** A second client gets `lease_held`, and after expiry takes the transfer over. The first client's next commit gets `lease_lost`.
- **Runs.** A job prints 50 MiB of log. The tail is disconnected twice, and the received log is byte-identical to the runner's spool.

## 16. Questions (answered with their defaults)

| ID | Question | Default (approved) |
|---|---|---|
| Q1510 | Chunk size and window? | **16 MiB, 8 in flight** by default; 8–64 MiB as a power of two; at most 16,384 chunks |
| Q1511 | Files over 1 TiB? | **Refused in v1.** A later version adds hierarchical journal snapshots |
| Q1512 | Desktop mTLS device certificates? | **Later.** v1 uses API keys over server-authenticated TLS. §83 treats the desktop as a client |
| Q1513 | Retention for abandoned and incomplete transfers? | **7 days without a commit**, then garbage collection deletes the chunks and journal. Completed transfers stay until deleted |
| Q1514 | A gateway route for transfers? | **Not in v1.** The engine serves them. The gateway can route `/v1/…/transfers` later |
| Q1515 | A Files product (browse, share, mount)? | **Separate design.** v1 lists transfers and runs only |
| Q1516 | Content-addressed dedup across transfers? | **No.** Keys carry the hash for idempotency, not for dedup |
| Q1517 | Run arbitrary commands from the desktop? | **No.** Named jobs from the runner's `jobs.toml` only (D2440) |

## 17. Decisions

| ID | Decision | § |
|---|---|---|
| D2420 | The transfer client is a Rust sidecar, `loams-transfer agent`, driven by Electron main over JSON lines on stdio. It ships beside the `loams` engine binary in the desktop app | 4, 12 |
| D2421 | The server side is in `crates/loams-transfer` (module `server`), mounted in the engine's main router under `/v1/namespaces/{ns}/transfers` and `/runs`. A standalone `loams-transfer serve` exists for tests and dev | 4 |
| D2422 | The transfer API is HTTP plus JSON, with raw chunk bodies (§6). Requests are independent, so each chunk is one request and one QUIC stream | 6 |
| D2423 | Chunks are fixed size: a power of two from 8 to 64 MiB, 16 MiB by default, and at most 16,384 per transfer | 6 |
| D2424 | The chunk key is `chunks/<index:08>-<sha256>`: sending twice is idempotent, and a committed chunk is never overwritten | 5.2 |
| D2425 | The checkpoint is a journal: immutable segments in the bucket, plus a metastore pointer swapped by compare-and-swap with a lease fence. The swap is the commit. There is a snapshot every 64 segments, and group commit within a process | 5.1 |
| D2426 | Expansion only: a resume never deletes or rewrites committed data. A changed size, chunk size, mtime or chunk hash is refused, and the client starts a new transfer | 6, 7.1 |
| D2427 | A PUT of a committed index with the same hash answers 200 without writing; with a different hash, 409 `chunk_conflict` | 6 |
| D2428 | Single writer: lease `transfer/<id>` (TTL 30 s, renewed every 10 s). The holder is the install id, guarded locally by an exclusive file lock | 8 |
| D2429 | Takeover by another holder only after expiry, at the next epoch. A stale epoch gets `lease_lost` | 8 |
| D2430 | The server checks the lease epoch on every commit, through the pointer swap's fence, not through a separate read | 8 |
| D2431 | The client pipelines up to `window` chunks (default 8), each one its own request stream, with retry and backoff per chunk | 7, 13 |
| D2432 | Bounded memory: two passes per chunk, both streamed from disk in 1 MiB blocks. Nothing buffers a chunk or the file. Downloads `pwrite` into a sparse part file | 7, 13 |
| D2433 | Integrity: SHA-256 per chunk, checked on write and on read; a root over the chunk list; and a whole-file SHA-256 that every downloader checks before the rename | 9 |
| D2434 | Downloads commit locally as data `fdatasync`, then the checkpoint is replaced with a temporary file and a rename | 7.2 |
| D2435 | HTTP/3 (quinn, h3 and h3-quinn at CT1's pins) by default, with §83's race and 15-minute memory for the HTTP/2+TLS fallback | 10 |
| D2436 | Progress events carry bytes done and total, throughput, ETA, the transport, chunks in flight, retries and the resume state, at most 4 per second | 12 |
| D2437 | HTTP/3 is on UDP at the TLS main listener's port. The HTTP/2 fallback is the main listener. Plain HTTP only to a loopback engine | 10 |
| D2438 | TLS trust order: a per-server CA bundle, then the deployment CA, then the system roots. Certificate pinning for dev. API-key bearer auth | 10 |
| D2439 | Job runs reuse the journal: the pointer `run/<rid>`, log segments and artifact references | 11 |
| D2440 | Runners run only named jobs from their own `jobs.toml`. Params pass only through an environment allowlist | 11, 14 |
| D2441 | A log append is idempotent by offset: a mismatch gets 409 with the committed length, and the runner resends from its spool | 11 |
| D2442 | Tailing resumes from the byte offset the desktop received, streamed and followed until the run ends | 11 |
| D2443 | Authorization: `writer` and `reader` on the namespace, plus the `runner` relation for runners. Transfer names are labels, never paths | 14 |

## 18. Plan

[DQ1](../plans/2026-10-29-dq1-desktop-quic-transfer.md). One lead, one PR per task to `dev`, and the status file `loams-wt/.he0-shared/dq1-status.md`.
