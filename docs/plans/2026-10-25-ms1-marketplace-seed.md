# MS1 — The Marketplace Seed

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Where this plan gives exact names, paths, fields or tests, use them verbatim. Record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Approved stub** (owner, 2026-10-25). Track MS, the first part ("seed") of the Loams Marketplace, under design [§73](../design/73-agent-marketplace.md) and its [§18 addendum](../design/73-agent-marketplace.md#18-ms1-addendum-2026-10-25) (D1806–D1814, Q1330–Q1332). The owner's word "start" (2026-10-25) lifts the 2026-10-24 pause of track MK **for MS1's scope only** (D1806); MK0's remaining tasks stay paused, except Task 4b (the daemon wiring), which Task 2 needs and which therefore falls inside MS1's scope. This plan specifies **Tasks 0, 1, 3 and 4** in full and Tasks 2 and 5–10 at task level; each of those is specified when the task before it merges. On 2026-10-25 the owner confirmed Task 4b and accepted the remaining proposals ("finish asap with your recommendations"), so D1812–D1814 are approved.

**Goal:** a self-hosted Loams node runs marketplace agents on **QuickJS** by default, **Mastra** agents on Node or Bun, and **Goose** as a first-party ACP harness, all under agentd's environment contract; public agent artifacts, a read-only verifiable project knowledge cache and a build cache tier are served from **Walrus** over plain HTTP, every byte checked by SHA-256 against a signed record, with the bucket still the source of truth. A person sees all of it working in Studio, in the browser and on the desktop (Task 10).

**Architecture** (§73 §3, §5, §7; §36 §8; §45 §3.1):
- **`crates/loams-live-js`** gains an **`agent` profile** beside Live's function profile (Task 1, spike first). It reuses the `Isolated` [`WorkerPool`] (seccomp, landlock, one worker per agent). The agent has no sockets, files or timers of its own: `fetch`, `model.*`, `mcp.call` and timers are host calls that agentd answers through its egress proxy, its model proxy and its MCP shim. The agent speaks ACP as frames on the worker's existing framed stdio.
- **`crates/loams-agentd-marketplace`** gains a **`quickjs` runtime** (Task 2) and an **`engine: node | bun`** choice on the `node` runtime (Task 6).
- **`crates/loams-agentd-harness`** gains **`goose_spec`** next to `grok_spec`: a first-party ACP harness launched as `goose acp` from a pinned release archive (Task 3).
- **`crates/loams-walrus`** (new): the Walrus **fast path**, HTTP only, with hedged reads across aggregators, SHA-256-verified reads and a local verified disk cache; no Sui dependency (Task 4).
- **Walrus as a public read-only tier** for three uses: listing artifacts (Task 5), project knowledge (Task 7) and the build cache (Task 8). The bucket stays the source of truth (D1); Walrus is never a `Store` backend.

**Tech stack:** Rust 1.97 (workspace dependencies only: `reqwest` 0.13, `sha2` 0.11, `base64` 0.23, `axum` 0.8 for test fakes; `rquickjs` `=0.14.0` as pinned); goose v1.54.0 release binaries (Apache-2.0); Walrus Testnet HTTP endpoints, then Mainnet aggregators (Q1330 for writes); Bun at the version D1791 pins; TypeScript and Node ≥ 22.13 for tooling. New dependencies follow the 14-day rule.

**Spec:** [§73](../design/73-agent-marketplace.md) §3.2 (D1452), §3.3 (D1453), §3.4 (D1454), §3.7 (D1464), §5 (D1460), §6 (D1459), §7, §18; [§45](../design/45-loams-live-production.md) §3.1 (D681); [§36](../design/36-loams-git.md) §8 (D398); plan [MK1](2026-10-21-mk1-agent-marketplace.md) and its rulings; Goose's ACP guide ([goose-docs.ai, "Use goose as an ACP agent"](https://goose-docs.ai/docs/gdk/acp), source `documentation/docs/gdk/acp/index.md` in [aaif-goose/goose](https://github.com/aaif-goose/goose)); Walrus's [web API](https://docs.wal.app/usage/web-api.html), [reading blobs](https://docs.wal.app/docs/http-api/reading-blobs), [quilt HTTP APIs](https://docs.wal.app/docs/http-api/quilt-http-apis), [network reference](https://docs.wal.app/docs/network-reference) and the aggregator's OpenAPI (`crates/walrus-service/aggregator_openapi.yaml` in [MystenLabs/walrus](https://github.com/MystenLabs/walrus)). All read 2026-10-25.

## Facts checked for this plan (2026-10-25)

**Goose** ([aaif-goose/goose](https://github.com/aaif-goose/goose), Apache-2.0):
- `goose acp` serves ACP over stdio; `goose serve` serves it over HTTP or WebSocket at `/acp`. MS1 uses only `goose acp`.
- Releases are per-target archives with SHA-256 digests in the release metadata. Linux and macOS are `.tar.gz` or `.tar.bz2` (`goose-x86_64-unknown-linux-gnu.tar.gz`, `goose-aarch64-apple-darwin.tar.gz`, musl builds too); Windows is `.zip`. v1.54.0 (2026-10-08) is the newest release at least 14 days old; v1.55.0 and v1.56.0 are too new.
- `session/new`'s `mcpServers` are added to the session's extensions (`add_mcp_servers` in `crates/goose/src/acp/server.rs`).
- `GOOSE_SYSTEM_PROMPT_FILE_PATH` is read only by the CLI's session builder (`crates/goose-cli/src/session/builder.rs`), not by the ACP server. Hint files are: `CONTEXT_FILE_NAMES` (default `[".goosehints", "AGENTS.md"]`) in the working directory, plus a global hints file.
- Configuration through the environment: `GOOSE_PROVIDER`, `GOOSE_MODEL`, the provider's host and key variables (`OPENAI_HOST`, `OPENAI_BASE_PATH`, `ANTHROPIC_HOST`), `GOOSE_MODE` (`auto`, `approve`, `chat`, `smart_approve`; default `auto`), `GOOSE_DISABLE_KEYRING` (any value disables), `GOOSE_PATH_ROOT` (moves all data, config and state), and `GOOSE_TELEMETRY_ENABLED`.
- The owner's link `https://goose-docs.ai/docs/guides/acp-clients` answers 404; the page now lives at `/docs/gdk/acp`.

**Walrus** ([MystenLabs/walrus](https://github.com/MystenLabs/walrus), Apache-2.0; `testnet-v1.59.0`, `mainnet-v1.58.1`):

Reads, from an aggregator:
- `GET /v1/blobs/{blob_id}`, which accepts a `Range` header and answers `206`. Its OpenAPI says only the fetched slivers are consistency-checked for a range.
- `GET /v1/blobs/{blob_id}/byte-range?start=&length=`.
- `GET /v1/blobs/by-object-id/{object_id}`.
- `GET /v1/blobs/by-quilt-id/{quilt_id}/{identifier}` and `GET /v1/blobs/by-quilt-patch-id/{patch_id}`.
- `GET /v1/quilts/{quilt_id}/patches`, which lists a quilt's patches.

Writes, to a publisher:
- `PUT /v1/blobs?epochs=N` (deletable by default; `permanent=true`; `send_object_to=`) answers `newlyCreated { blobObject { blobId, size, storage { endEpoch … } … }, cost }` or `alreadyCertified { blobId, event, endEpoch }`.
- `PUT /v1/quilts?epochs=N` takes multipart fields named by identifier and answers `blobStoreResult` plus `storedQuiltBlobs[] { identifier, quiltPatchId }`.

Endpoints and limits:
- Testnet has a public aggregator and publisher (`https://aggregator.walrus-testnet.walrus.space`, `https://publisher.walrus-testnet.walrus.space`).
- Mainnet has a public aggregator (`https://aggregator.walrus-mainnet.walrus.space`) and **no public publisher**. Mainnet writes need an own authenticated publisher, the Upload Relay (`https://upload-relay.mainnet.walrus.space`) or the TypeScript SDK (Q1330).
- Most public aggregators and publishers limit requests to 10 MiB.
- An epoch is 1 day on Testnet and 2 weeks on Mainnet. Storage can be bought at most 53 epochs ahead.
- A CDN in front of an aggregator can cache a `404` just after certification, so reads retry with backoff.

**This repository** (`origin/dev` `bc902971d`):
- MK0 Tasks 1–6 are merged (`8799cc1a9` … `dd3a73a4e`). MK1's status table, dated 2026-10-24, still lists them as unmerged. Task 4b (daemon wiring) and Tasks 7–11 are not started.
- The isolated worker (`crates/loams-live-js/src/sandbox_linux.rs`) allows `read`/`write` on fds 0–2 and memory, clock, `futex`, `sched_yield`, `rt_sigreturn` and `exit_group` only. It has `RLIMIT_NOFILE` = 3 and refuses a process with more than one thread. It has no `socket`, no timer system call and no `getrandom`.
- The worker protocol (`ipc.rs`, `loams.live.worker.v1`) frames messages with a 4-byte big-endian length, at most 64 MiB each. The worker host types are tied to `loams_live` (`LiveTxn`, `HostDriver`).
- `AcpAgentSpec` (`crates/loams-agentd-harness/src/acp/mod.rs`) is static, has no environment field, and its harness inherits agentd's environment.
- `ArchivePin` pins `sha512`, and `archive_install` extracts **zip only**.
- agentd's model proxy (D1454) is **designed, not built**: `contract.rs` carries its URL, and nothing serves it.
- `loams-buildcache` **does not exist** (GT1 Tasks 68–71; the `loams` crate reserves an empty `buildcache` feature).
- `BlobStore` (`crates/loams-git/src/blob.rs:163`) is create-only and content-named, under GC through `Store`.

## Global Constraints

- **Worktrees and branches.** `/home/keshav/Ostrium/loams-wt/ms1-t<N>`; one PR per task into `dev`, or into `loams-marketplace` `main` where a task says. `git commit -s`.
- **Light Rust builds.** `cargo build -p <crate>`; never `--workspace` locally; never build in `/tmp`.
- **Off by default.** `[walrus] enabled = false` everywhere. With it off, no Walrus endpoint is contacted, and every feature behaves as on `dev` today.
- **The bucket is the source of truth** (D1). Walrus holds public copies only. Losing every Walrus blob loses no data and breaks no install that has an HTTPS fallback.
- **Public data only on Walrus.** Nothing from a private repository, private listing, secret, token, user data or agent state is ever written to Walrus. Walrus blobs are public and cannot be made private after the fact.
- **Every Walrus read is verified** against a SHA-256 taken from a signed record (the index, a signed knowledge manifest, a signed cache manifest) before any byte is used. An aggregator is untrusted.
- **No Sui dependency.** No Sui SDK, wallet, key or WAL token handling in any Loams crate. Writes go through a publisher over HTTP (Q1330 for Mainnet).
- **Testnet first.** Every Walrus task is tested against fakes in CI and against Testnet by hand. Mainnet reads come after Task 10; Mainnet writes wait on Q1330 and Q1332.
- **The agent environment contract** (D1453) is unchanged. A QuickJS agent gets the contract's capabilities as host calls instead of variables (Task 1); Goose gets it through its own variables (Task 3).
- **No telemetry.** No new code reports installs, reads or usage outside the user's own Meter.

## Review Focus

1. **A QuickJS agent reaches the network, the filesystem or another process directly.** Expected: never. Tests: `agent_worker_has_no_socket`, `agent_worker_cannot_open_files` (Task 1).
2. **A QuickJS agent's `fetch` reaches a host the manifest does not declare.** Expected: refused by agentd. Test: `agent_fetch_undeclared_host_refused` (Task 1).
3. **Goose reads the user's own configuration, keyring or credentials.** Expected: never. Test: `goose_env_is_private` (Task 3).
4. **A Walrus read is used before its SHA-256 matches.** Expected: never; a mismatch is an error, and the bytes are discarded and not cached. Tests: `read_rejects_wrong_sha256`, `cache_never_stores_unverified` (Task 4).
5. **Private data reaches a publisher.** Expected: never. Tests: `publish_refuses_private_repo` (Task 7), `untrusted_class_never_publishes` (Task 8).
6. **Something contacts Walrus while `[walrus] enabled = false`.** Expected: never. Test: `disabled_client_makes_no_requests` (Task 4).

## Tasks

### Task 0 — This plan and the decisions

Files:
- this plan;
- `docs/design/13-decision-log.md`: D1806–D1814 after D1805, and Q1330–Q1332 in the open questions;
- `docs/design/73-agent-marketplace.md`: §13 gains Goose and Walrus rows, and the new §18 "MS1 addendum (2026-10-25)";
- `docs/plans/README.md`: the MS1 row under track MK.

Checks: the question ids Q1330–Q1332 and the decision ids D1806–D1814 are unused anywhere under `docs/` (grep, 2026-10-25); every Goose and Walrus fact above was read from the linked source on 2026-10-25.

Tests: none. Gate: the PR is reviewed; nothing else merges under MS1 before it.

Commit: `docs: MS1, the marketplace seed plan and decisions D1806–D1814 (MS1 Task 0)`.

### Task 1 — QuickJS agent-profile spike

A spike with a written verdict. The code merges behind an off-by-default cargo feature if the verdict is go. If it is no-go, only the report merges, and Task 2 is replanned.

**Question.** Can an agent written in JavaScript run in the `Isolated` QuickJS worker with every capability of the agent environment contract (D1453) supplied as host calls through agentd, speak ACP to agentd, and start and run cheaply enough to be the self-hosted default (D1807)?

**What the profile is.**
- **`Profile::Agent`** beside Live's function profile in `crates/loams-live-js`. Live's profile is unchanged: its determinism rules, its `ctx.db` host API and its limits stay as they are.
- **One long-lived worker per running agent**, not one call per worker. The worker loads one ES module bundle and runs until agentd stops it. It reuses `WorkerPool`'s spawn, sandbox, crash backoff, the wall-clock kill (per host-call-free JavaScript slice) and the `degraded` mark.
- **The sandbox is not loosened.** The seccomp allowlist, landlock, `RLIMIT_NOFILE` = 3 and the single-thread rule apply unchanged. Anything the agent needs that the allowlist forbids is a host call.
- **Globals** (the agent allowlist, distinct from `GLOBALS`):
  - `Date` and `performance.now()` are real (`clock_gettime` is allowed);
  - `Math.random` is seeded from 32 bytes of entropy the host sends in `AgentLoad`;
  - `crypto.getRandomValues` and `crypto.randomUUID` draw from a host-refilled entropy pool;
  - `TextEncoder`, `TextDecoder`, `URL`, `URLSearchParams`, `Headers`, `Request`, `Response`, `AbortController`, `structuredClone`, `queueMicrotask` and `console`;
  - no `WebAssembly`, `eval` or `Function` constructor;
  - no Node built-ins.
- **Host calls** (new `loams.live.worker.v1` messages; every one is answered by agentd, never by the worker host itself):
  - `fetch(input, init)`: agentd performs the request through the install's **egress proxy**, which enforces the manifest's hosts. Bodies stream in frames of at most 1 MiB in both directions.
  - `model.chat`, `model.responses`, `model.messages` (from `loams:agent`): calls to agentd's **model proxy** in the Chat Completions, Responses and Anthropic Messages shapes (D1454), streamed as frames.
  - `mcp.call(server, tool, args)` and `mcp.list(server)`: through **agentd's MCP shim** (`loams-agentd-mcp`), which hides undeclared tools (D1468).
  - `setTimeout`, `setInterval`, `clearTimeout`, `clearInterval`: the worker sends `TimerSet { id, ms }` and `TimerClear { id }`; agentd's side sends `TimerFired { id }`. The worker has no timer of its own and waits only in `read` on stdin.
  - `loams.token()`: the current 15-minute Place token (D1453). Configuration values come in `AgentLoad`.
- **ACP over the worker's stdio.** ACP JSON-RPC messages travel as `AcpMessage { json }` frames, one message per frame, interleaved with host-call frames on the same pipes. On agentd's side an adapter turns them into the newline-delimited byte stream that `AcpHarness` already reads, so the harness code does not change. In the worker, `loams:agent` exports `acp.connect(handler)`. The ACP TypeScript SDK runs on top of it through a stream shim.
- **The event loop.** One thread: the worker runs pending jobs (`Runtime::execute_pending_job`) until none are left, then blocks reading the next frame. A frame is a host-call answer, a timer, an ACP message, cancel or stop. The spike decides whether rquickjs's `futures` feature is needed or whether promises resolved from frames are enough.
- **Who runs the worker.** agentd must start it, but `loams-live-js` depends on `loams-live` (`LiveTxn`, `HostDriver`). The spike measures what `loams-agentd` gains by depending on `loams-live-js` with only the agent profile. It then rules between two options:
  - a cargo feature `agent` that compiles the agent profile without the Live host;
  - splitting the sandbox, framing and pool into a new crate `loams-js-sandbox` that both use.

  The worker binary is `loams-agentd agent-worker` (a hidden subcommand), so a self-hosted node does not need the `loams` binary for agents.

**Files** (spike branch `ms1-t1-quickjs-agent-spike`):
- `crates/loams-live-js/src/agent/{mod,host,globals,acp}.rs`;
- `crates/loams-live-js/src/agent/prelude.js` (`loams:agent`);
- `proto/loams/live/worker/v1/` (the agent messages: `AgentLoad`, `AcpMessage`, `HostCall` variants `Fetch`, `Model`, `Mcp`, `Token`, `TimerSet`, `TimerClear`, `TimerFired`, `BodyChunk`, `Entropy`);
- `crates/loams-live-js/Cargo.toml` (feature `agent`, off by default);
- `crates/loams-live-js/tests/agent_profile.rs`;
- fixtures under `crates/loams-live-js/testdata/agents/`:
  - `echo-acp/`: a hand-written ACP agent with no dependencies;
  - `acp-sdk/`: an agent on the ACP TypeScript SDK, bundled;
  - `tool-agent/`: a model call through `model.chat`, then an `mcp.call`, then a `fetch`;
  - `ai-sdk/`: the AI SDK's `generateText` with a custom `fetch`, bundled; it may fail, and its failure is a finding;
  - `hostile/`: tries sockets, files, threads, a spin loop and a memory blow-up.

  Bundles are built by a checked-in script with `bun build --target=browser --format=esm` (falling back to esbuild), and their sizes are recorded.
- the report, `docs/plans/ms1-quickjs-agent-spike.md`.

**Tests** (Linux; the worker is Linux-only, as for Live):
- `echo_agent_answers_acp_initialize`, `echo_agent_completes_a_prompt_turn`;
- `acp_sdk_agent_answers_initialize`;
- `agent_fetch_goes_through_egress_proxy`, `agent_fetch_undeclared_host_refused`;
- `agent_model_call_reaches_scripted_proxy`, `agent_mcp_call_reaches_shim`, `agent_undeclared_mcp_tool_hidden`;
- `agent_timers_fire_in_order`, `agent_cancel_stops_turn`, `agent_sigterm_exits_within_10s` (LAPC-C7);
- `agent_worker_has_no_socket`, `agent_worker_cannot_open_files`, `agent_worker_cannot_spawn_thread`;
- `agent_spin_loop_killed`, `agent_memory_limit_enforced`;
- `live_function_profile_unchanged`, which runs the existing Live worker tests with the feature on.

The model proxy and the egress proxy are test fakes (a scripted endpoint as LAPC uses, and a recording proxy), because the model proxy is not built (Facts).

**Measurements** (the report gives each for the QuickJS profile and for the same agent on `node` (Node ≥ 22.13) and on Bun, on one named machine, with p50/p95 over 50 runs):
- cold start: spawn to ACP `initialize` answered;
- warm start, if the pool keeps a pre-spawned worker;
- RSS of an idle agent, and of an agent after a 20-turn scripted session;
- bundle size per fixture, minified and gzipped;
- host-call round trip for an empty call, a 1 MiB `fetch` body and a streamed model response;
- the build-time and binary-size cost to `loams-agentd` of the chosen option.

**Go or no-go.** Go when all of these hold:
- every test above passes, except `ai-sdk`, which is reported;
- `echo-acp` and `acp-sdk` run unmodified apart from the bundling;
- cold start p95 is under 100 ms, and an idle agent's RSS is under 32 MiB (targets, estimate);
- no change to the seccomp allowlist was needed.

Otherwise the report names each failure and the change it would take, and the owner rules before Task 2.

Commit: `live-js: an agent profile on the isolated QuickJS worker, spike and report (MS1 Task 1, D1807)`.

### Task 2 — The `quickjs` agent runtime (task level)

Depends on Task 1's go, on MK0 Tasks 1–6 (merged), and on MK0 Task 4b (the daemon wiring, not started), without which no marketplace harness reaches `ListHarnesses`.
- `runtime.kind` gains `quickjs` in `schemas/marketplace/listing.v1.json` and `listing.proto`, with the `artifact.type` `npm` (a package whose `main` is the bundle) or `archive`.
- `crates/loams-agentd-marketplace/src/install/quickjs.rs` installs the bundle by integrity, with no install step at all.
- The launcher starts `loams-agentd agent-worker` and registers the agent as an ACP harness (`HarnessId::Marketplace`, MK1 Task 0 ruling 7).
- The consent plan shows "Runs in QuickJS: no direct network, files or processes".
- `quickjs` becomes the default runtime the templates and `loams agent new` produce for non-Mastra JavaScript agents on self-hosted nodes (D1807). LAPC Core runs against it.

Tests: `quickjs_listing_validates`, `quickjs_install_runs_no_code`, `quickjs_agent_appears_in_list_harnesses`, LAPC-C1 to C8 on the `echo-acp` fixture.

Commit: `agentd: the quickjs agent runtime (MS1 Task 2, D1807)`.

### Task 3 — Goose as a first-party ACP harness

Goose runs as a process that agentd launches; it is not linked as a library (D1809).

**Launch.**
- `goose_spec()` beside `grok_spec()` in `crates/loams-agentd-harness/src/acp/mod.rs`:
  - `id: HarnessId::Goose`, `display_name: "Goose"`, `executable: "goose"`, `env_override: "GOOSE_EXECUTABLE"`, `args: &["acp"]`;
  - `npm_package: None`, `archive: goose_archive()`.
- `goose_archive()` pins the **v1.54.0** release archive for the platform. The Linux builds are `goose-<arch>-unknown-linux-gnu.tar.gz`, or the musl build where glibc is missing (ruled in the task); macOS is `goose-<arch>-apple-darwin.tar.gz`; Windows is `goose-x86_64-pc-windows-msvc.zip`.
- `ArchivePin` pins `sha512`, so the task downloads each archive and checks it against the SHA-256 GitHub publishes for the asset. It then pins the SHA-512 and records both digests in its ruling.
- `archive_install` extracts zip only, so the task adds `.tar.gz` extraction there, with the same limits (`MAX_ARCHIVE_BYTES`, `MAX_EXTRACTED_BYTES`, `MAX_ARCHIVE_ENTRIES`, no paths outside the destination, no links that leave it).
- `cli_executable: "goose"`, with the user's own install found on `PATH`, `~/.local/bin` and Homebrew. **The managed archive is always the one launched**; a user's own goose only marks it "installed" in the picker. This differs from other harnesses, because a user's goose would read the user's configuration.

**The environment contract for goose** (`crates/loams-agentd-harness/src/acp/goose_env.rs`). The child starts from an **empty environment** (`env_clear`), unlike the other ACP harnesses, and gets only:
- `PATH` (agentd's managed bin directories, then system directories), `LANG`;
- `HOME` and `TMPDIR`: a private directory per agentd data root, `<agentd data>/harnesses/goose/home`;
- `GOOSE_PATH_ROOT=<agentd data>/harnesses/goose/root`;
- `GOOSE_DISABLE_KEYRING=1`, `GOOSE_TELEMETRY_ENABLED=false`, `GOOSE_DISABLE_SESSION_NAMING=1`;
- `GOOSE_MODE=approve`, so every tool call asks over ACP `session/request_permission` and agentd's approvals decide. A full-access thread auto-accepts, as for every ACP harness;
- `GOOSE_PROVIDER` and `GOOSE_MODEL`, with the provider's host variable pointed at **agentd's model proxy** and a dummy key:
  - `GOOSE_PROVIDER=openai`, `OPENAI_HOST=http://127.0.0.1:<port>`, `OPENAI_BASE_PATH=v1/chat/completions`, `OPENAI_API_KEY=<dummy>`;
  - or `anthropic` with `ANTHROPIC_HOST`, chosen by the model's shape (D1454);
- `CONTEXT_FILE_NAMES='[".goosehints","AGENTS.md"]'` (goose's default, set explicitly so a release that changes it does not change behaviour).

Every variable name above is checked against v1.54.0's source in this task. A name that differs is a ruling, not a silent change.

**MCP.** agentd's MCP shim and the thread's MCP servers are passed in `session/new` `mcpServers` (`acp_mcp_servers`, which exists). Goose adds them as extensions. A `config.yaml` written under `GOOSE_PATH_ROOT` enables only goose's `developer` built-in extension, so goose loads no extension from anywhere else.

**Persona.** Goose's ACP server takes no system prompt (`GOOSE_SYSTEM_PROMPT_FILE_PATH` is CLI-only; Facts). A Loams persona or a listing's instructions are therefore written to goose's **global hints file** under `GOOSE_PATH_ROOT`'s config directory before launch. The exact path is read from v1.54.0's `load_hints.rs` and recorded. The project's own `.goosehints` and `AGENTS.md` in the working directory are left to goose, as for any harness reading project instructions.

**Models.** One discovered list from goose's ACP model options when it advertises them; otherwise the models the user configured in the model proxy.

**Wire identity.**
- `HarnessId::Goose` in `crates/loams-agentd-proto/src/agent.rs`, wire name `goose`;
- the harness registry descriptor;
- the regenerated TypeScript bindings;
- a first-party harness listing `ostrium/goose` in `loams-marketplace`, once MK0 Task 11 runs, mapped through `InstallHarness` (D1464).

**Depends on** agentd's model proxy for real use. Until it serves requests, Goose is listed but **not selectable**: the picker shows "Needs the model proxy". The tests use a scripted proxy.

Files:
- `crates/loams-agentd-harness/src/acp/mod.rs`;
- `crates/loams-agentd-harness/src/acp/goose_env.rs`;
- `crates/loams-agentd-harness/src/archive_install.rs` (tar.gz);
- `crates/loams-agentd-proto/src/agent.rs`;
- the registry descriptor and the TS bindings;
- `crates/loams-agentd-harness/tests/goose.rs`;
- a fake `goose` ACP binary under the crate's test fixtures for CI.

Tests:
- `goose_spec_launches_goose_acp`;
- `goose_archive_pins_match_release_digests` (offline: the pinned SHA-512 against a fixture);
- `archive_install_extracts_tar_gz`, `archive_install_tar_gz_refuses_escape`;
- `goose_env_is_private`: the child's environment is exactly the list above, and no user variable, `XDG_*`, `SSH_AUTH_SOCK` or provider key leaks;
- `goose_env_points_provider_at_model_proxy`;
- `goose_session_new_passes_mcp_servers`;
- `goose_persona_written_to_hints_file`;
- `goose_permission_request_maps_to_approval`;
- `goose_unselectable_without_model_proxy`.

One manual run against the real v1.54.0 binary and a scripted proxy is recorded in the ruling.

Commit: `agentd: Goose as a first-party ACP harness (MS1 Task 3, D1809)`.

### Task 4 — `crates/loams-walrus`, the fast path

A small HTTP client for Walrus aggregators and publishers. No Sui dependency, no Walrus SDK, no wallet (D1811).

**Config** (`WalrusConfig`, deserialised under `[walrus]` by the binaries that use it):
```toml
[walrus]
enabled = false                 # nothing is contacted while false
network = "testnet"             # "testnet" | "mainnet"; labels, and picks the defaults below
aggregators = ["https://aggregator.walrus-testnet.walrus.space"]
publisher = "https://publisher.walrus-testnet.walrus.space"   # optional; unset means read-only
publisher_auth = ""             # optional bearer token file for an own publisher (Q1330)
epochs = 5                      # default storage epochs for writes
hedge_after_ms = 300            # start the next aggregator after this long
request_timeout_ms = 30000
max_blob_bytes = 10485760       # 10 MiB, the public endpoints' limit
cache_dir = ""                  # default: <data dir>/walrus-cache
cache_max_bytes = 4294967296    # 4 GiB, LRU by last read
```
Mainnet has no public publisher, so `network = "mainnet"` with no `publisher` is read-only, and a write answers `WalrusError::NoPublisher`.

**API** (`src/lib.rs`):
- `Walrus::new(cfg) -> Result<Walrus>`, which, when `enabled = false`, returns a client whose every call answers `WalrusError::Disabled` without I/O;
- `read(&BlobId, &Sha256) -> Result<Bytes>`;
- `read_quilt_patch(&QuiltId, identifier, &Sha256)` and `read_patch(&QuiltPatchId, &Sha256)`;
- `read_range(&BlobId, Range<u64>) -> Result<Unverified<Bytes>>`;
- `list_patches(&QuiltId)`;
- `store(Bytes, StoreOpts { epochs, permanent }) -> Result<Stored>`;
- `store_quilt(Vec<(identifier, Bytes)>, StoreOpts) -> Result<StoredQuilt>`.

Types (`src/types.rs`):
- `BlobId` is 32 bytes, URL-safe base64 without padding, as Walrus prints it;
- `QuiltId` is a `BlobId`; `QuiltPatchId` is the opaque string Walrus returns;
- `Stored { blob_id, end_epoch, newly_created: bool, size }` parses both `newlyCreated.blobObject` and `alreadyCertified`;
- `StoredQuilt { blob: Stored, patches: Vec<(identifier, QuiltPatchId)> }`.

**Reads** (`src/read.rs`, `src/hedge.rs`):
- `GET {aggregator}/v1/blobs/{id}`, `/v1/blobs/by-quilt-id/{id}/{identifier}`, `/v1/blobs/by-quilt-patch-id/{patch}` and `/v1/quilts/{id}/patches`.
- **Hedged:** start the first aggregator in the configured order; if it has not answered within `hedge_after_ms`, start the next, and so on. The first verified answer wins and the rest are cancelled.
- A `404` within 60 s of a known write retries with backoff (CDN cache, Facts). Any other `404` moves on to the next aggregator; all `404` is `NotFound`.
- The body is bounded by `max_blob_bytes` while streaming.
- **Verified:** the SHA-256 of the whole body must equal the expected value. A mismatch is `WalrusError::Integrity { aggregator }`; the bytes are dropped, the next aggregator is tried, and the mismatch is logged with the aggregator's URL.
- `read_range` uses the `Range` header and returns `Unverified<T>`, which callers must check against their own record. No MS1 caller uses it on bytes it trusts; the knowledge cache and the build cache read whole blobs or patches.

**Writes** (`src/write.rs`):
- `PUT {publisher}/v1/blobs?epochs=N[&permanent=true]` and `PUT {publisher}/v1/quilts?epochs=N` (multipart, one field per identifier).
- The response is parsed into `Stored`. A body over `max_blob_bytes` is refused before upload.
- After a write, the client reads the blob back through an aggregator and checks its SHA-256 before returning `Ok`.

**Cache** (`src/cache.rs`):
- On disk under `cache_dir`, keyed by **SHA-256** (`sha256/<2>/<hex>`), not by blob id, so the cache stores only bytes whose hash it checked.
- An index file maps blob id or patch id to SHA-256.
- Entries are written to a temporary file and renamed; LRU eviction above `cache_max_bytes`.
- A read hits the cache only when the expected SHA-256 is present. A cached file is re-hashed on first use after a restart.

Files:
- `crates/loams-walrus/{Cargo.toml,src/{lib,config,types,read,hedge,write,cache,error}.rs}`;
- `crates/loams-walrus/tests/{read,write,cache}.rs`, with an `axum` fake aggregator and publisher in `tests/support/`;
- the workspace `Cargo.toml` member;
- `deny.toml`, if a licence entry is needed.

Tests:
- `disabled_client_makes_no_requests`;
- `read_verifies_sha256`, `read_rejects_wrong_sha256`;
- `hedge_starts_second_after_delay`, `hedge_first_verified_wins`, `hedge_skips_lying_aggregator`;
- `not_found_after_write_retries`, `body_over_limit_refused`;
- `quilt_patch_read_by_identifier`, `list_patches_parses`;
- `store_parses_newly_created`, `store_parses_already_certified`, `store_quilt_parses_patches`, `store_reads_back_and_verifies`, `mainnet_without_publisher_is_read_only`;
- `cache_hit_needs_sha256`, `cache_never_stores_unverified`, `cache_evicts_lru`, `cache_rehashes_after_restart`;
- `range_read_is_unverified` (a type-level check).

A manual Testnet run (store, read back, quilt store, patch read) is recorded in the ruling, with the blob ids.

Commit: `walrus: loams-walrus, the HTTP fast path with hedged verified reads and a local cache (MS1 Task 4, D1811)`.

### Task 5 — Public agent artifacts on Walrus (task level)

- `artifact` gains the source kind `{ "kind": "walrus", "blobId": "<base64url>", "sha256": "<hex>" }` in `schemas/marketplace/listing.v1.json` and `proto/loams/marketplace/v1/listing.proto`. It is an **additional location** beside the artifact's existing npm or archive reference. It never replaces it, and `proto_and_schema_agree` covers it.
- The index stays metadata-only (D1460, D1812): it carries the blob id and SHA-256, never bytes.
- The installer (`crates/loams-agentd-marketplace/src/install/`) runs, for an artifact with a `walrus` location while Walrus is enabled:
  1. fetch through `loams-walrus`;
  2. check the SHA-256 against the signed index;
  3. check the artifact's integrity and Sigstore provenance exactly as today (D1459);
  4. install.

  On any Walrus failure it falls back to the HTTPS location. With Walrus disabled it uses HTTPS only. Provenance is never skipped because bytes came from Walrus.

Tests: `walrus_source_validates`, `walrus_install_verifies_sha256_then_sigstore`, `walrus_failure_falls_back_to_https`, `walrus_disabled_uses_https`.

Commit: `marketplace: walrus artifact locations (MS1 Task 5, D1812)`.

### Task 6 — Bun engine for Mastra agents (task level)

- The `node` runtime accepts `runtime.engine: "node" | "bun"` (default `node`) in schema and proto.
- agentd resolves the engine as it resolves Node (MK1 Task 0 ruling 9), at the Bun version D1791 pins, and launches `bun <entrypoint>`.
- Installs stay script-free (`bun install --ignore-scripts --frozen-lockfile`, or the npm tarball path unchanged).
- The Mastra profile (`@loams/agent-host`) runs on both engines. Its LAPC Core run is required on each engine a listing declares.

Tests: `engine_defaults_to_node`, `bun_engine_launches_bun`, `bun_install_runs_no_scripts`, and LAPC Core on the Mastra starter under Bun.

Commit: `marketplace: the node runtime runs Mastra on node or bun (MS1 Task 6, D1808)`.

### Task 7 — Read-only project knowledge cache (task level)

Design: D1813.
- **Publishing.** `loams knowledge publish <repo> --commit <sha>` reads a **public** repository's knowledge set at one commit (Q1331), stores it as one Walrus quilt (identifier = path), and writes a **signed manifest** (`{ repo, commit, quiltId, entries: [{ path, quiltPatchId, sha256, bytes }], created, endEpoch }`, Sigstore keyless) to the bucket and beside the repository's releases. It refuses a private repository.
- **Reading.** agentd's MCP shim gains two read-only tools:
  - `knowledge.get { repo, commit?, path }` returns a verified file;
  - `knowledge.search { repo, commit?, query, limit }` searches the manifest's paths and a small index built locally from fetched entries.

  Both verify the manifest's signature, then each entry's SHA-256, and are served from `loams-walrus`'s cache. Both are `risk: read` and need no approval.

Tests: `publish_refuses_private_repo`, `manifest_signature_required`, `knowledge_get_verifies_entry`, `knowledge_tools_are_read_only`.

Commit: `knowledge: a read-only verifiable project knowledge cache on Walrus (MS1 Task 7, D1813)`.

### Task 8 — Build cache Walrus tier (task level)

Depends on `loams-buildcache` (GT1 Tasks 68–71), which does not exist yet (Facts).
- A read-through tier in `loams-buildcache`'s sccache gateway path, after the bucket: local → bucket → Walrus.
- **Only the `trusted` class on public repositories publishes** (CI on protected branches; D398). The `untrusted` class (forks, PRs, agent sandboxes and developer machines) reads only, and never publishes.
- The map from sccache key to `{ blobId, sha256 }` is a manifest **signed by the trusted CI identity and kept in the bucket**, under `ns/<ns>/cache/sccache/<repo>/trusted/walrus-manifest`. A Walrus hit is used only if the key is in a manifest whose signature verifies and the SHA-256 matches.
- A hit is copied into the bucket's `trusted/` prefix only when the reader is itself trusted.
- Private repositories never use the tier.

Tests: `untrusted_class_never_publishes`, `walrus_hit_needs_signed_manifest`, `private_repo_skips_walrus_tier`, `bucket_miss_reads_walrus_then_verifies`.

Commit: `buildcache: a read-through Walrus tier for public repositories (MS1 Task 8, D1814)`.

### Task 9 — Publish tooling (task level)

- `loams-listing publish --walrus` stores a release artifact on Walrus (Testnet unless the configured publisher is a Mainnet one) and writes the `walrus` location into the listing.
- `loams knowledge publish` is Task 7's command, made scriptable for CI.
- `loams-listing verify` checks each `walrus` location (it reads the blob and compares the SHA-256). It **warns** when the blob's `endEpoch` is within 4 epochs of the current one, and fails when it has passed.
- A GitHub Actions example renews epochs before expiry, once Q1332 is answered.

Tests: `publish_walrus_writes_location`, `verify_warns_near_epoch_expiry`, `verify_fails_after_expiry`.

Commit: `marketplace: walrus publish tooling and epoch checks (MS1 Task 9)`.

### Task 10 — End to end on a self-hosted node in Studio (task level)

On one self-hosted node with `[walrus] enabled = true` on Testnet, in Studio in the browser and on Loams Desktop:
1. install a QuickJS agent whose artifact has a `walrus` location, and run a thread with it;
2. install a Mastra agent on Bun, and run a thread;
3. pick Goose in the harness picker, and run a thread that calls `knowledge.get` through the MCP shim;
4. build a public fixture repository twice in trusted CI, and see Walrus hits on a clean bucket prefix.

A Playwright run in `studio-e2e` covers steps 1–3 on the browser; steps 1–4 are recorded by hand on Desktop.

Report: `docs/plans/ms1-exit-report.md`.

Commit: `docs: MS1 exit report (MS1 Task 10)`.

## Out of scope

- **Walrus as a `Store` backend.** It has no conditional writes or listing and would fail D61's `Store` provider conformance; the bucket stays the only durable truth (D1).
- **Walrus behind `BlobStore`** (`loams-git`): Loams Git's blobs include private repositories and are deleted by GC.
- **Private data on Walrus**, in any form, encrypted or not.
- **Sui wallets, WAL payments**, Seal and Walrus Sites.
- **Embedding `goose-sdk`** or any goose crate in agentd.
- **Goose recipes as listings.**
- MK0 Tasks 7–11, which stay paused.

## Rulings made during execution

None yet.
