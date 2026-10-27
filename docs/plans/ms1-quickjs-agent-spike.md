# MS1 Task 1 — QuickJS agent-profile spike: report

Date: 2026-10-26. Branch `ms1-t1-quickjs-agent-spike`. Plan: MS1 Task 1
(`docs/plans/2026-10-25-ms1-marketplace-seed.md`), decision D1807.

## Verdict: go

A JavaScript agent runs in Live's `Isolated` QuickJS worker. Every
capability of the agent environment contract (D1453) arrives as a host
call that agentd answers. The agent speaks ACP to agentd, and it starts
about 6× faster and uses about 5× less memory than the same agent on
Node.

The plan sets four conditions for go:

| Condition | Result |
|---|---|
| Every planned test passes (`ai-sdk` may fail and is reported) | **Yes.** 21/21 in `tests/agent_profile.rs`, `ai-sdk` included (it needs a bundled Web-streams polyfill, finding 5). Live's suites pass unchanged, with and without the feature |
| `echo-acp` and `acp-sdk` run unmodified apart from the bundling | **Yes.** `acp-sdk` uses the real `@agentclientprotocol/sdk` 1.8.0 and bundles `web-streams-polyfill` |
| Cold start p95 under 100 ms, idle RSS under 32 MiB | **Yes for small bundles:** 13.7 ms and 8.9 MiB (`tool-agent`). **No for 1 MB bundles:** the SDK fixtures take 160–200 ms p95, all of it QuickJS parsing the bundle. Node and Bun are no faster on those bundles. Precompiled bytecode is the fix for Task 2 (risk R1) |
| No change to the seccomp allowlist | **Yes.** `sandbox_linux::apply` is reused unchanged: same allowlist, landlock, `RLIMIT_NOFILE` = 3 and single-thread rule |

The one miss is cold start for large bundles. The owner should rule on it
before Task 2 (Q below).

## What was built (all behind the off-by-default `agent` feature)

`crates/loams-live-js` now has two features:

- `live` (on by default) covers everything Live had. Its code is unchanged
  and only placed behind `cfg`s.
- `agent` adds the agent profile. It also builds without `live`
  (`--no-default-features --features agent`).

Code:

- `src/agent/worker.rs` is the worker. It is one thread and one event
  loop:
  - it reads `AgentLoad`, creates the runtime and seals itself (the same
    `apply` Live uses);
  - it evaluates the bundle and answers `AgentLoaded`;
  - then, in a loop, it runs the job queue dry, writes `AgentIdle` and
    blocks in `read` on stdin.
  - Promises resolved from frames are enough. rquickjs's `futures` feature
    is **not** needed.
- `src/agent/host.rs` is agentd's side:
  - `AgentWorker` spawns the worker with `env_clear` and `kill_on_drop`;
  - the `AgentHosts` trait stands for agentd's egress proxy, model proxy
    (D1454), MCP shim (D1468) and the Place token;
  - it checks the `Manifest` (allowed egress hosts, declared MCP tools);
  - it runs the timers, refills entropy, and keeps the busy watchdog.
- `src/agent/acp.rs` turns `AcpMessage` frames into the newline-delimited
  JSON-RPC that `AcpHarness` reads, and back.
- `src/agent/globals.rs` holds the entropy pool: ChaCha20, keyed by the
  host's 32 bytes and rekeyed by `Entropy` frames.
- `src/agent/prelude.js` defines `loams:agent` (`acp`, `model`, `mcp`,
  `loams`) and the agent globals.
- `src/agent/fake.rs` holds the scripted model proxy, MCP shim, egress
  proxy and token.
- `src/bin/loams-agent-worker.rs` has two modes: `worker` (the sealed
  worker) and `acp <bundle> --fake-hosts` (agentd's side as a process, for
  `AcpHarness` and the measurements).

Wire format: new messages in `loams.live.worker.v1`:

- `AgentLoad` / `AgentLoaded`, `AcpMessage`;
- `AgentCall` with `FetchCall`, `ModelCall`, `McpCall` and `TokenCall`;
- `AgentReply` with `error`, `json` or `FetchHead`;
- `BodyChunk` (at most 1 MiB, in both directions);
- `TimerSet`, `TimerClear`, `TimerFired`;
- `Entropy`, `EntropyRequest`;
- `AgentIdle`, `AgentLog`, `AgentStopped`;
- `PROBE_KIND_SPAWN_THREAD`.

A function worker never receives the agent messages. Live's protocol code
is unchanged.

Globals the agent sees:

- `Date` and `performance.now()` are real;
- `Math.random`, `crypto.getRandomValues` and `crypto.randomUUID` draw
  from the host-keyed pool;
- `TextEncoder`, `TextDecoder`, `URL`, `URLSearchParams`, `Headers`,
  `Request`, `Response`, `AbortController`/`AbortSignal`,
  `DOMException`, `structuredClone`, `queueMicrotask`, `console`, `fetch`
  and the four timer functions are defined.

What it does not see:

- No `eval`. The `Eval` intrinsic must stay, because module compilation
  uses `JS_Eval`, so the prelude deletes `eval` instead.
- No `Function` constructor, nor the async and generator ones (they
  throw `EvalError`).
- No `WebAssembly`, `require`, `process`, `std`/`os`, or importable
  modules other than `loams:agent`.

Fixtures (`testdata/agents/`, built by `build.sh`, which uses `bun build
--target=browser --format=esm` when bun is on PATH and esbuild 0.25.12
otherwise; dependencies are pinned and installed with scripts off):

- `echo-acp`: a hand-written ACP agent;
- `acp-sdk`: an agent on the ACP TypeScript SDK;
- `tool-agent`: model, then MCP, then fetch, then a timer, with
  cancellation, plus a `bench` command;
- `ai-sdk`: `generateText` through `@ai-sdk/openai-compatible`;
- `hostile`: tries escape hatches, a spin loop, a C built-in and a memory
  blow-up.

## Tests (Linux; `cargo test -j 2 -p loams-live-js --features agent`)

`tests/agent_profile.rs`, 21 passed:

- `echo_agent_answers_acp_initialize`, `echo_agent_completes_a_prompt_turn`;
- `acp_sdk_agent_answers_initialize`;
- `agent_model_call_reaches_scripted_proxy`: the stream arrives as one
  chunk per word;
- `agent_mcp_call_reaches_shim`, `agent_undeclared_mcp_tool_hidden`: the
  list is filtered, and a call to an undeclared tool never reaches the
  shim;
- `agent_fetch_goes_through_egress_proxy`: includes a 3 MiB body in
  1 MiB frames;
- `agent_fetch_undeclared_host_refused`: tries an undeclared host, a
  suffix trick, userinfo and loopback;
- `agent_timers_fire_in_order`;
- `agent_token_entropy_and_config_are_host_supplied`;
- `agent_cancel_stops_turn`: ACP `session/cancel` gives `cancelled`, and
  the agent serves the next turn;
- `agent_sigterm_exits_within_10s`;
- `agent_hostile_javascript_finds_no_escape_hatch`;
- `agent_worker_has_no_socket`, `agent_worker_cannot_open_files`,
  `agent_worker_cannot_spawn_thread`, `agent_worker_cannot_exec`: each is
  killed by `SIGSYS`;
- `agent_spin_loop_killed`: the interrupt handler stops it at the 200 ms
  slice;
- `agent_c_builtin_killed_by_the_watchdog`: an `Array.prototype.reverse`
  over 2^53 − 1 elements is killed after the slice plus the grace;
- `agent_memory_limit_enforced`;
- `ai_sdk_generate_text_through_the_egress_proxy`.

Live (`live_function_profile_unchanged`): every existing suite passes both
with `--features agent` and with the defaults. The suites are the unit
tests, determinism, functions, limits, sandbox, validators and worker.
One run of `determinism::…leftover_jobs_never_run_in_the_next_call`
missed its 500 ms timing bound under parallel load. It passed on three
reruns and on the full rerun.

`cargo clippy -p loams-live-js --features agent --all-targets` and the
agent-only `--lib --bins` build are clean. `cargo fmt` is clean.

## Measurements

Machine: Intel Core Ultra 5 225H, 14 threads, Linux 7.0.0-34, release
build. Node v24.21.0, Bun 1.4.3. The driver is
`testdata/agents/compare/measure.py`, which talks ACP from outside:

- QuickJS runs `loams-agent-worker acp <bundle> --fake-hosts`, which is
  two processes: agentd's side and the sealed worker;
- Node and Bun run `compare/host.mjs <bundle>`, with the same scripted
  hosts in process.

p50/p95 are over 50 runs for cold start and 20 for turns.

`tool-agent` (6.1 KB bundle):

| | QuickJS | Node | Bun |
|---|---|---|---|
| Cold start: spawn to ACP `initialize` answered (p50 / p95) | **9.8 / 13.7 ms** | 60.3 / 162.9 ms | 42.2 / 52.3 ms |
| Idle RSS after `session/new` | **8.9 MiB** (agentd side 4.0 + worker 4.8) | 47.8 MiB | 23.6 MiB |
| RSS after a 20-turn session | **9.2 MiB** | 58.4 MiB | 29.6 MiB |
| Turn: one streamed model call, one MCP call, one fetch, one timer (p50 / p95) | 4.7 / 5.4 ms | 2.7 / 3.4 ms | 3.3 / 5.2 ms |
| Empty host call round trip (`loams.token()`, mean of 500) | 32 µs | 0.4 µs (in process) | 1.8 µs (in process) |
| 1 MiB `fetch` response body (mean of 20) | 6.0 ms | 5.8 ms | 2.9 ms |
| Streamed model response, 9 events (mean of 50) | 0.22 ms | 0.02 ms | 0.03 ms |

On Node and Bun the hosts are in process. A real Node agent reaches
agentd's proxies over loopback HTTP, which costs more than QuickJS's
32 µs frame round trip.

Large bundles (cold start p50 / p95 over 30 runs, then idle RSS):

| Bundle (raw / min+gz) | QuickJS | Node | Bun |
|---|---|---|---|
| `echo-acp` (1.3 KB / 0.5 KB) | 9.1 / 13.7 ms, 8.8 MiB | 58.1 / 66.1 ms, 47.8 MiB | 42.1 / 50.9 ms, 24.7 MiB |
| `acp-sdk` (993 KB / 122 KB; zod v4 dominates) | 160 / 200 ms, 22.4 MiB | 180 / 215 ms, 82.0 MiB | 137 / 171 ms, 45.7 MiB |
| `ai-sdk` (1.34 MB / 175 KB) | 140 / 160 ms, 20.5 MiB | 164 / 183 ms, 75.7 MiB | 133 / 148 ms, 41.2 MiB |

Other bundle sizes (raw / minified / minified + gzip):

- `tool-agent`: 6.1 KB / 3.8 KB / 1.8 KB;
- `hostile`: 2.2 KB / 1.5 KB / 0.8 KB.

What `loams-agentd` would gain by depending on `loams-live-js` with
`--no-default-features --features agent`:

- 108 crates in the graph. Only **23 are new to agentd**: rquickjs
  (`-sys`, `-core`), `loams-live-proto` with buffa/connectrpc codegen,
  landlock, seccompiler, rand/rand_chacha, `cc` and friends.
- The default (`live`) graph is 226 crates.
- A release build of the agent-only worker binary takes 2 m 50 s with
  `-j 2` from a cold target, most of it QuickJS's C.
- The agent-only release binary (`loams-agent-worker`) is 4.5 MB.

## Findings

1. **The sandbox did not have to move.** Every other need is a frame:
   timers are `TimerSet`/`TimerFired`, entropy arrives in `AgentLoad` and
   `Entropy`, and the worker waits only in `read`.
2. **QuickJS C built-ins that never poll the interrupt handler** are
   stopped by a watchdog on agentd's side. On quickjs-ng 0.16.2 these are
   `Array.prototype.{reverse,fill,copyWithin,join,with,toReversed}`,
   `String.prototype.repeat`, typed-array `fill` and `ArrayBuffer.slice`.
   - The worker writes `AgentIdle` after every frame it handles. agentd
     kills a worker that stays busy longer than its CPU slice plus a
     grace.
   - A turn's wall time stays unbounded, because model waits are
     legitimate. Only uninterrupted JavaScript is bounded.
   - In-process there is no such backstop, so an in-process agent mode
     would be for trusted code only, as for Live (D681).
3. **Out of memory can be catchable.** An allocation refused by the
   allocator can surface as a catchable `InternalError` if the interrupt
   handler has not run in between. The worker therefore checks the
   meter's flag after every dispatch and stops itself.
4. **`Function.prototype` must stay reachable.** When the prelude replaces
   the `Function` constructor, the replacement has to keep `.prototype`:
   bundled code calls `Function.prototype.apply.call(…)`. This was the
   first `acp-sdk` failure.
5. **Web streams.** QuickJS has no `ReadableStream`, `WritableStream` or
   `TransformStream`. The ACP SDK and the AI SDK both need them, and the
   AI SDK needs them at module load. A bundled `web-streams-polyfill`
   (about 90 KB) makes both work unmodified. Task 2 should ship the
   streams natively in the prelude, so that every bundle does not carry
   its own polyfill.
6. **UTF-8 decoding in JavaScript is slow.** `TextDecoder` is in
   JavaScript because reading an `ArrayBuffer` from Rust needs `unsafe`,
   which `loams-live-js` forbids. Decoding a 3 MiB body as text timed out
   in a debug build. Task 2 should do the decoding natively, in an
   `unsafe`-allowed helper crate like `loams-live-js-alloc`.
7. **`SIGSYS` kills can dump core.** With systemd-coredump as the core
   pattern, a worker killed by `SIGSYS` keeps its pipes open while the
   core is written. agentd's watchdog may then see "busy" before it sees
   "exited". Task 2 should set `RLIMIT_CORE` = 0 in the seal (for Live
   too). That tightens the seal and does not loosen it.
8. **Two processes per agent.** `AcpHarness` takes an executable, so in
   the spike agentd's side runs as a process (`loams-agent-worker acp`,
   4 MiB RSS). To drop that process, Task 2 needs a harness entry point
   that takes a byte stream instead of a child process. A harness that
   spawns `loams-agentd agent-worker --acp <bundle>` also works without
   any harness change.
9. **Mastra stays on Node or Bun (D1808), confirmed.** A reduced
   `new Agent({…})` from `@mastra/core` 1.78.0, bundled for
   `--platform=browser`, fails to resolve **71 imports of Node
   built-ins**:
   - `path` ×10, `crypto` ×9, `os` ×6, `stream/web` ×5, `fs` ×5,
     `fs/promises` ×4, `events` ×4, `async_hooks` ×4 (and
     `node:async_hooks` ×2), `stream` ×3, `child_process` ×3, `url` ×2,
     `module` ×2;
   - one each of `node:zlib`, `node:util`, `node:readline`, `net`,
     `https`, `http` and `dns`.
10. **The proto crate needs `protoc`.** `loams-live-proto` needs a system
    `protoc` at build time. agentd does not need it today.

## Recommendation for MS1 Task 2

- **Crate layout: the cargo feature.** agentd depends on `loams-live-js`
  with `default-features = false, features = ["agent"]`.
  - Gating Live's host behind `live` took only `cfg`s. The agent profile
    reuses `sandbox_linux`, the framing in `ipc.rs` and
    `loams-live-js-alloc` directly. It adds 23 crates to agentd.
  - Split out a `loams-js-sandbox` only when a third user appears, or
    when `loams-sandbox::seal_self` (D991) replaces `sandbox_linux` for
    both.
  - To keep `protoc` out of agentd's build, move `loams.live.worker.v1`
    to checked-in generated code, or into a buffa-only crate.
- **Worker binary.** `loams-agentd agent-worker` becomes the hidden
  subcommand calling `agent_worker_main()` before any runtime starts.
  `AgentWorker` and `acp::serve_stdio` run inside agentd, and
  `AgentHosts` is implemented on:
  - the egress proxy (`set_net_policy` hosts);
  - the model proxy (D1454, once built);
  - `loams-agentd-mcp`'s shim;
  - the token file.
- **Host-call protocol.** Keep the frames above.
  - Add `AgentCancel` only if ACP's own `session/cancel` proves
    insufficient. In the spike it was enough.
  - Stream fetch response bodies to JavaScript as `ReadableStream`. The
    spike buffers the whole body.
  - Abort a host call whose JavaScript `AbortSignal` fires. The spike
    abandons it on the worker side only.
- **Cold start for large bundles.** Compile the bundle to QuickJS bytecode
  at install (`JS_WriteObject`) and load bytecode in the worker. Measure
  against the 100 ms target before shipping.

## Risks

- **R1: large bundles miss the 100 ms cold-start target in QuickJS**
  (160–200 ms p95 for 1 MB bundles). The mitigations are precompiled
  bytecode and keeping one pre-spawned warm worker per installed agent.
- **R2: QuickJS memory-safety bugs.** The kernel seal is the boundary, as
  for Live. Third-party agents must never run in process.
- **R3: Web API gaps.** The prelude's `URL` has no IDNA, there is no
  `Response.clone`, and bodies are buffered. Libraries may assume more.
  Each gap is a prelude addition, not a sandbox change.
- **R4: the BUCK files are not regenerated** on this branch: `gen.py`
  needs reindeer, which is not bootstrapped here. They must be regenerated
  before merge.

## Question for the owner

Q: is the go accepted, given that cold start for 1 MB bundles is
160–200 ms (p95), with Task 2 required to add bytecode precompilation?
