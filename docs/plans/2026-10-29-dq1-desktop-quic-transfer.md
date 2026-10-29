# DQ1 — Desktop Resumable Transfers and Job Streams over QUIC

> **For agentic workers:** Work task by task, test first. Where this plan gives exact names, paths, branches or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file. The plan's rulings use decision IDs D2444–D2449 and questions Q1518–Q1519.
>
> **Status: Active** (2026-10-29). The design is [§85](../design/85-desktop-resumable-quic-transfer.md) (D2420–D2443, Q1510–Q1517). **The owner approved its defaults on 2026-10-29**, as relayed by the coordinator. One lead builds it alone, one PR per task to `dev`. Status label: **preview**.

**Goal:** Loams Desktop can do two things over HTTP/3, falling back to HTTP/2+TLS:
- move a file of 10 GB or more to and from a deployment, surviving a crash and a disconnect, with a byte-identical result;
- follow a remote job run's log and artifacts, resuming after a disconnect.

**Architecture** (§85 §4):
- `crates/loams-transfer`:
  - `journal`: the fenced journal over a `Checkpoints` trait (a metastore adapter and an in-memory one).
  - `server`: the transfer and run services, and an `axum` router with an `Authorizer` trait.
  - `net`: an HTTP/3 server and client on `quinn` and `h3`, HTTP/2+TLS, and the race.
  - `client`: the upload and download pipelines.
  - `runner`: the job runner.
  - `agent`: the JSON-lines sidecar.
  - `bin/loams-transfer`: `agent`, `serve`, `runner`, `put`, `get` and `tail`.
- `crates/loams`: mounts the router and serves HTTP/3 on the TLS main port.
- `apps/desktop-electron/src/main/transfer/*`: the sidecar supervisor and IPC.
- `web/plugins/desktop-transfers`: the Transfers page.

## Global constraints

- **Worktrees:** `~/Documents/Ostriumlabs/loams-wt/dq1-<task>`. One branch and one PR per task, based on `dev`. A task that needs an unmerged one is stacked on it, and the PR says so. Commit with `git commit -s`. Never `git stash`.
- **Builds:** run through `~/Documents/Ostriumlabs/loams-wt/.he0-shared/buildslot.sh` with `-j 4`, and build only the touched crates. Never build in `/tmp`, and never set `CARGO_TARGET_DIR`.
- **Disk:** test data is generated, sparse or streamed, and deleted afterwards. The 10 GiB test never materialises its file in memory or as dense blocks.
- **Labels:** `review:greptile` on Tasks 2, 4 and 5, which add APIs.
- **Pins:** `quinn` =0.11.12, `h3` =0.0.8, `h3-quinn` =0.0.10 (CT1 Task 0). No other new runtime dependencies beyond the workspace and the lockfile.
- **Honest labels:** the Transfers page and the docs say **preview**.

## Review focus

1. **A crash loses or corrupts data.** Tests: `kill_midway_resumes_byte_identical` (256 MiB in CI, 10 GiB ignored), `download_kill_resumes_byte_identical`, `zombie_commit_is_harmless`.
2. **Two writers.** Tests: `second_holder_gets_lease_held`, `takeover_after_expiry_fences_old_holder`, `stale_epoch_commit_refused`.
3. **A committed chunk is rewritten.** Tests: `same_chunk_twice_is_noop`, `changed_chunk_conflicts`, `resume_with_changed_size_refused`.
4. **Memory grows with the file.** Test: `upload_memory_bounded` (peak RSS under the target for 2 GiB vs 256 MiB).
5. **Fallback.** Tests: `h3_and_h2_serve_the_same_router`, `udp_blocked_falls_back_to_h2`, `udp_failure_remembered`.
6. **Runs.** Tests: `log_append_idempotent_by_offset`, `tail_resumes_from_offset`, `runner_refuses_unknown_job`, `params_only_via_env_allowlist`.

## Tasks

| # | Task | Files | Tests | PR label |
|---|---|---|---|---|
| 0 | Design §85, this plan, the decision-log rows and the index | `docs/design/85-desktop-resumable-quic-transfer.md`, this file, `docs/design/13-decision-log.md`, `docs/design/README.md` | `scripts/docs/check-decision-ids.sh` | — |
| 1 | **Journal, checkpoint and lease**: the `Checkpoints` trait with a `MetaStore` adapter and an in-memory one; the journal (segments, snapshots, group commit); transfer state (create, lease, commit chunk, complete, abandon); run state (queue, claim, append log, artifacts, finish) | `crates/loams-transfer/{Cargo.toml,BUCK,src/lib.rs,src/journal.rs,src/checkpoint.rs,src/transfer.rs,src/run.rs,src/keys.rs}` | `journal_roundtrip_with_snapshots`, `same_chunk_twice_is_noop`, `changed_chunk_conflicts`, `resume_with_changed_size_refused`, `second_holder_gets_lease_held`, `takeover_after_expiry_fences_old_holder`, `stale_epoch_commit_refused`, `complete_requires_all_chunks`, `root_is_order_and_length_bound`, `log_append_idempotent_by_offset`, `claim_is_exclusive`, plus a `MetaStore` adapter test on a single-node `loams-meta` | — |
| 2 | **The HTTP API and the network**: the `axum` router and `Authorizer`; chunk PUT streamed into `put_multipart` while hashing; the HTTP/3 server and client with streamed bodies; the HTTP/2+TLS server and client; the race with its 15-minute memory; `loams-transfer serve` | `crates/loams-transfer/src/{server.rs,net/mod.rs,net/h3.rs,net/h2.rs,net/race.rs,net/tls.rs,bin/loams-transfer.rs}` | `h3_and_h2_serve_the_same_router`, `chunk_put_streams_without_buffering`, `bad_hash_refused_422`, `unauthorized_refused`, `udp_blocked_falls_back_to_h2`, `udp_failure_remembered`, `pinned_cert_accepted_other_refused` | `review:greptile` |
| 3 | **The client pipelines and the sidecar**: upload and download with the window, two-pass streaming, the whole-file hash, the local checkpoint with `fdatasync` then rename, retry and backoff; `loams-transfer agent` (JSON lines); `put` and `get` | `crates/loams-transfer/src/{client/mod.rs,client/upload.rs,client/download.rs,client/local.rs,agent.rs}`, `crates/loams-transfer/tests/{resume.rs,agent.rs}` | `kill_midway_resumes_byte_identical` (256 MiB; 10 GiB `#[ignore]`, run locally and recorded below), `download_kill_resumes_byte_identical`, `zombie_commit_is_harmless`, `upload_memory_bounded`, `agent_protocol_roundtrip`, `progress_throttled` | — |
| 4 | **Job runs**: the runs API, `loams-transfer runner` with `jobs.toml`, spooling, log append, artifacts as transfers, cancel and timeout; the tail client | `crates/loams-transfer/src/{runner.rs,client/tail.rs}`, `crates/loams-transfer/tests/runs.rs` | `runner_refuses_unknown_job`, `params_only_via_env_allowlist`, `tail_resumes_from_offset` (50 MiB log, two disconnects, byte-identical), `artifacts_uploaded_and_listed`, `cancel_stops_job`, `timeout_marks_timed_out` | `review:greptile` |
| 5 | **The engine**: mount the router under `/v1/namespaces/{ns}/{transfers,runs}`; an `Authorizer` on the engine's principals; HTTP/3 on UDP at the TLS main port (`LOAMS_TRANSFER_H3`, on by default with TLS) | `crates/loams/{Cargo.toml,src/api/transfers.rs,src/api/mod.rs,src/main.rs}`, `crates/loams/tests/transfers.rs` | `transfers_mounted_and_authorized`, `h3_on_main_port_with_tls`, `no_h3_without_tls` | `review:greptile` |
| 6 | **Desktop**: the sidecar binary lookup and supervisor; IPC channels; contracts and preload; API keys in `safeStorage`; the `desktop-transfers` console plugin (progress, throughput, ETA, resume state, runs and the log view); fake-desktop data; the packaging of the sidecar beside the engine binary | `apps/desktop-electron/src/main/transfer/{binary.ts,supervisor.ts,protocol.ts,ipc.electron.ts,keys.ts}`, `apps/desktop-electron/src/shared/contracts.ts`, `apps/desktop-electron/src/preload/index.ts`, `apps/desktop-electron/scripts/fetch-engine.mjs`, `web/plugins/desktop-transfers/*`, `web/apps/console/src/cordis/fake-desktop.ts` | vitest: `supervisor_restarts_with_backoff`, `protocol_parses_events`, `eta_and_rate`, `page_shows_resume_state`, `log_view_follows_offset` | — |
| 7 | **Release, benchmark and docs**: ship `loams-transfer` in the release archives the desktop fetches; a loopback and tailnet h3-vs-h2 measurement for desktop bulk; the user guide | `.github/workflows/release-*.yml`, `docs/guides/desktop-transfers.md`, `bench/transfer/README.md` | the release dry run; the recorded measurements | — |

## Gates (target)

- **Byte-identical resume:** the 10 GiB kill-and-resume upload and download yield SHA-256 and `cmp` equality (Task 3, run locally and recorded).
- **Memory:** the sidecar's peak RSS during a 10 GiB upload stays under 128 MiB (Task 3).
- **Throughput:** 300 MB/s or more on loopback over HTTP/3 (Tasks 3 and 7). The tailnet run is within 15 % of `iperf3` (Task 7).
- **Fallback:** with UDP blocked, transfers complete over HTTP/2 (Task 2).

## Starting order (2026-10-29)

0 → 1 → 2 → 3 → 4, then 5 and 6 (they need only Task 2's router and Task 3's agent protocol), then 7.

## Rulings made during execution

| Task | Ruling | Summary |
|---|---|---|
