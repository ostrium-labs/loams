# DD1 — Loams Desktop Agent Daemon Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, **test first**: write the named tests, run them and watch them fail, then implement until they pass. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact values (names, paths, methods, exit codes, defaults, reasons), use them verbatim. The code is not pre-written in this plan (M0.3 Ruling 1). Record every deviation in "Rulings made during execution" at the end of this file.
>
> **Status: Planned** (2026-10-09). **Track DD**, the desktop daemon (design [§50](../design/50-loams-desktop-daemon.md), D780–D799, Q700–Q714). It carries out the owner's five decisions of 2026-10-09: one agent system in a per-user Rust daemon, no edge, no GPUI, the chat and plugin UI from dsh-desktop, and Linux linger off by default. It has six milestones, DD1a–DD1f, run in order; inside DD1d and DD1e some tasks run in parallel (see "Milestones and order"). Branches `dd1<m>-t<N>` (for example `dd1b-t7`), stacked per milestone, based on `dev`; PRs target `dev`.

**Goal:** Loams Desktop has one agent system, `loams-agentd`:
- a headless per-user Rust daemon, built from the zeron fork's engine and harnesses (without edge, WorkOS, Cursor, self-update, push or GPUI) plus the Electron agent ported to Rust;
- it supervises `loams dev` and the compose stacks, so the data plane runs while the window is closed;
- its agent turns survive the UI closing and resume after a crash or reboot, through `loams-durable`;
- it is installed as a per-user service only with the user's consent, and secured by a per-user token that never reaches the renderer;
- Electron is its client, with a chat, sessions, terminal and diff UI adapted from dsh-desktop's DSH client UI;
- every merge keeps the shipping app working, and the TypeScript agent path is removed only after the end-to-end durability gate passes.

**Architecture:**
- **The daemon** is `crates/loams-agentd` (binary and library) in the root Cargo workspace, with the moved fork crates `loams-agentd-{sessions,harness,proto,rpc,doc,store,mcp,link,preview}` and the new crates `loams-agentd-{llm,loop,tools,factory,supervisor}` (§50 §4.1).
- **Wire.** ndjson over a loopback WebSocket on an ephemeral port, as the fork's `rpc` crate does, with a token checked before the upgrade, `Hello`, roles and a generated method table (§50 §5.4, §7).
- **TypeScript** types and the method table are generated with ts-rs into `web/packages/agentd-client` (`@loams/agentd-client`); its Node client runs only in Electron main; the renderer goes through preload (§50 §8).
- **The native loop** is `HarnessId::LoamsAgent` inside the sessions engine, so every agent shares one session model (§50 §10).
- **Durability.** An embedded Resonate server (SQLite, no HTTP listener) runs each native turn as a durable function with checkpointed model and tool steps; harness turns resume through their native session ids (§50 §11).
- **Electron** keeps the console protocol and proxy, the factory app views, the updater, the tray and deep links; it installs and talks to the daemon; feature flags `engine.owner` and `agent.runtime` switch each area over with a fallback until DD1f (§50 §16).

**Tech Stack:**
- **Rust:** 1.97.1, edition 2024, workspace lints (the fork's `rust-toolchain.toml` pin is the same).
- **Moved Rust dependencies** (from `apps/desktop/native/Cargo.lock`; Task 0 checks each against the root lock and `deny.toml`): `loro` 1.13, `rusqlite` 0.32.1 (`bundled`, same `libsqlite3-sys` 0.30.1 as the root), `tokio-tungstenite` 0.24, `portable-pty`, `keyring` 4.2, `connectrpc` 0.9.1, `reqwest` 0.12, `notify`, `ignore`, `nucleo-matcher`, `similar`, `json5`, `deser-hjson`, `plist`.
- **New Rust dependencies** (Task 0 checks versions at least 14 days old, licences and that they build together):
  - `ts-rs` (MIT), features `serde-compat`, `serde-json-impl`, `uuid-impl`;
  - `jsonschema` (MIT), JSON Schema 2020-12 for tool arguments;
  - `tokio-postgres` (MIT OR Apache-2.0) with `tokio-postgres-rustls`, and `mysql_async` (MIT OR Apache-2.0), default features off and rustls on;
  - `notify-rust` (MIT OR Apache-2.0), Linux and Windows only.
- **Reused Rust:** `loams-durable` and the pinned Resonate crates, `loams-proto` (generated Connect clients), `tokio`, `serde`, `tracing`, `sha2`, `libc`, `windows-sys`, `tempfile`, `proptest`.
- **TypeScript:** the workspace toolchain (TypeScript 5.9.3, Node ≥ 22, pnpm, Biome, Vitest 5, Playwright 1.63), Electron 44.4.5 (pinned by AP1e), React 19, `@loams/ui`, `@loams/slots`, `@loams/cordis`. New, pinned exactly and at least 14 days old: `ws` (MIT; the WHATWG `WebSocket` cannot send an `Authorization` header), `@xterm/xterm` and `@xterm/addon-fit` (MIT).
- **Platforms:** Linux x86_64 and aarch64 (systemd user units), macOS arm64 and x64 (launchd), Windows x86_64 (HKCU Run entry). CI has Linux runners for every gate; the Windows and macOS jobs of `desktop-electron.yml` build and run the unit tests.

**Spec:**
- [§50](../design/50-loams-desktop-daemon.md): all of it.
- [§37](../design/37-desktop-and-mobile-apps.md) §19 (D652–D679), §18 for the fork's history.
- [§21](../design/21-durable-execution.md) for Resonate, D138 and D141.
- [Decision log](../design/13-decision-log.md): D138, D141, D220, D426, D433, D485–D489, D497, D652–D679, D780–D799; Q420, Q620, Q700–Q714.
- As built: [AP1e](2026-10-08-ap1e-electron-desktop.md) (Tasks 15, 22, 24, 28, 29 and its rulings), [AP1n](2026-10-02-ap1n-native-desktop-zeron.md), the fork's `apps/desktop/native/LOAMS.md`, [LV1](2026-10-08-lv1-live-production.md) rulings T0-10 to T0-14 (the desktop follow-ups the supervisor port inherits).
- AP1e console conventions: `.superpowers/sdd/2026-10-08-ap1e-electron-desktop/page-plugin-conventions.md` in the main checkout.
- dsh-desktop (`~/Documents/Ostriumlabs/dsh-desktop`, read-only, HEAD `51f9896`): `LICENSE`, `package-lock.json`, `docs/{architecture,patch-plugin-contract}.md`, `patches/`.

## Global Constraints

- **Worktree.** `~/Documents/Ostriumlabs/loams-wt/dd1-<milestone>` (for example `dd1b-process-model`). Never commit in the main checkout. `git commit -s` (DCO). Never `git stash`.
- **Commit areas:** `agentd`, `durable`, `desktop`, `web`, `ci`, `docs`.
- **The build machine.** One cargo build at a time, the shared target directory from `~/Documents/.cargo/config.toml`. Never set `CARGO_TARGET_DIR`, never pass `--target-dir`, never build in `/tmp`. Build and test with `-p <crate>`. The moved fork used `--target-dir target`; Task 1 removes every such use.
- **The app works at every merge.** Each PR that touches `apps/desktop-electron` or `web/` runs `pnpm nx run-many -t test typecheck -p loams-desktop-electron @loams/console plugins`, `pnpm --filter @loams/console build`, and the Linux Playwright smoke (`test/e2e`). An area moves to the daemon only behind its flag (`engine.owner`, `agent.runtime`) with the TypeScript path as the fallback, until Task 35.
- **The renderer never sees the token.** Only Electron main reads `agentd.token`. Preload forwards only methods whose generated spec has `renderer: true`.
- **No secrets in logs, replies or agent environments.** Provider keys, factory credentials, the RPC token and scoped run tokens never reach a log line, a span, an error text, an RPC reply, the journal, a session doc or a child process's environment. The canary tests of Tasks 7, 19 and 22 enforce it.
- **No network calls** from the daemon except to: model providers the user configured, the user's factory apps, the supervised engine and stacks on loopback, and harness installs the user started. No edge, no update feed, no telemetry (D663).
- **Wire changes** go through `crates/loams-agentd-proto` and are followed by regenerating `web/packages/agentd-client/src/gen/` (`cargo test -p loams-agentd-proto export_bindings`). `ts_bindings_are_fresh` must pass.
- **Platform code** for Windows and macOS is written in the same task as the Linux code, behind `cfg`, and compiles in the `desktop-electron.yml` Windows and macOS jobs. Linux carries the gates.
- **dsh-desktop is read-only.** Code from the DSH client UI packages is taken only from the published tarballs verified against `dsh-desktop/package-lock.json`'s `integrity` hashes (Task 28), and every adapted file carries the header of §50 §14.1.
- **AP1e's console rules hold** (page-plugin conventions): `@loams/ui` components first, Tailwind utilities with token colours only, `rounded-none`/`rounded-pill`, lucide icons, loading, error and empty states, no raw HTML, no remote images.

## Rulings made while writing this plan

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | **The fork crates move with `git mv` in one commit per crate group, then are edited.** No history rewriting, no subtree split | `git log --follow` keeps working; review sees moves and edits apart | Larger first PR |
| 2 | **The RPC framing stays the fork's ndjson** (`{id, method, params}`, `{id, ok|err|item|done}`), not Connect or gRPC | The engine, `mcp` and their tests already speak it; streams and cancellation are built; no codegen for Rust | A second wire beside Connect in the repository |
| 3 | **Method names stay flat PascalCase** (`ResolveApproval`, `WatchEngine`), as the fork's `methods` module has them | One namespace, no renames of the 109 existing names that survive | Collisions are caught by the method table test |
| 4 | **`agentd.json` and `agentd.token` live in the daemon's data directory, not `$XDG_RUNTIME_DIR`** | One place on every OS; the runtime dir disappears at logout, which breaks linger | The token file survives a reboot; it is regenerated at every start anyway |
| 5 | **Electron copies the binaries into the runtime directory by calling the bundled binary**: `resources/bin/loams-agentd install-runtime --from <resources/bin> --to <userData>/agentd/runtime` | The copy, hash check, `current` switch and garbage collection are Rust and tested once for three OSes | Electron depends on the bundled binary being runnable before install |
| 6 | **Native transcripts are a table in the session store** (`native_chats`), not Resonate step results | Steps hold one turn; the transcript spans the chat and must be read without replay | Two writes per step (store and checkpoint) |
| 7 | **The native loop's events reuse `AgentEvent`**; only `ApprovalRequested` and `ApprovalResolved` are new variants | The UI renders one event type for every harness | Provider-specific detail (thinking signatures) stays out of events and in the transcript |
| 8 | **Approvals resolve only through `ResolveApproval` with role `owner`**; scoped run tokens can never call it | A harness agent must not approve the native loop's writes through its MCP shim | None |
| 9 | **`loams-durable` gains `DurableConfig.serve_http` (default `true`)** instead of the daemon running Resonate without the crate | One durable stack; `loams`'s behaviour is unchanged | A field the engine never sets |
| 10 | **The TypeScript factory adapters are not ported; only the D660 read-only ops are**, with a request-parity suite | The adapters' full surface is used by the browser console, not by the desktop | Two implementations of about 20 read-only ops until the console moves to the daemon too |

## Milestones and order

| Milestone | Tasks | Scope | Runs |
|---|---|---|---|
| — | 0 | Reconcile with the code as built | First |
| **DD1a** — headless daemon crates | 1–5 (5) | Move and rename the fork crates into the root workspace; delete GPUI and headed mode; strip the edge, WorkOS, Cursor, updates and push; the headless CI guard; notices | Second. Electron is untouched; nothing ships |
| **DD1b** — process model and contract | 6–13 (8) | Lock, discovery, token, `Hello`; RPC security and roles; per-user services and linger; ts-rs types, method table and fixtures; the Electron client; the daemon ships alongside; consent, tray and stop; upgrades | Third. After Task 11 every build ships the daemon in child mode |
| **DD1c** — supervision moves | 14–17 (4) | The Rust engine supervisor and stack manager; Electron behind `engine.owner`; the default flips | Fourth |
| **DD1d** — the agent in Rust | 18–27 (10) | Providers, tool registry and scrubbing, engine tools, SQL tools, secrets and their import, factory ops, the loop as a harness with approvals, durable turns, chat import, Electron behind `agent.runtime` | Fifth. Tasks 18–23 run in parallel after Task 9; 24 needs 18–19; 25 needs 24; 26 needs 24; 27 needs 20–26 |
| **DD1e** — the UI from dsh-desktop | 28–32 (5) | The DSH study and port map; the session store and chat components; sessions and reattach; terminal and diffs; plugin slots and Settings › Plugins | Task 28 may start any time after Task 0; 29–32 need Task 9 and a fake daemon, and land after Task 27 |
| **DD1f** — switch, remove, review, release | 33–38 (6) | The end-to-end durability gate; the agent default flips; the TypeScript path is removed; security review; docs; exit report | Last, in order |

39 tasks in all (0–38).

## Review Focus

1. **Local access control.** No client without the token reaches a method; a browser never does; the renderer reaches only `renderer: true` methods; a scoped run token reaches only its own session's MCP methods and never `ResolveApproval`. Tests: Task 7 (`upgrade_without_token_is_401`, `origin_header_is_403`, `foreign_host_is_403`, `scoped_token_cannot_resolve_approval`, `scoped_token_bound_to_session`), Task 10 (`client_sends_no_origin`), Task 11 (`preload_forwards_only_renderer_methods`).
2. **Secrets.** No secret appears in a log, span, reply, journal, doc or child environment; keys stay bound to their origin; the migration deletes the old vault only after verification. Tests: Task 19 (`canary_never_leaves`), Task 22 (`key_is_origin_bound`, `configure_reply_has_no_key`, `import_then_verify_then_delete`), Task 14 (`engine_env_is_scrubbed`).
3. **Durability semantics.** A write tool runs at most once across crashes; an approval survives a restart and cannot be settled from outside; budgets do not reset. Tests: Task 25 (`write_tool_runs_at_most_once_across_crash`, `pending_approval_survives_restart`, `durable_api_has_no_listener`, `budgets_continue_after_restart`), Task 33 (the e2e gate).
4. **Behavioural parity of the ports.** The Rust supervisor, stack manager, providers, loop, SQL fences and factory ops behave as their TypeScript originals on the same inputs. Tests: Task 14 (the `engine.test.ts` cases), Task 15 (`stacks.test.ts` cases), Task 18 (`sse_fixtures_match_ts_events`), Task 21 (`sql_lex_corpus_agrees`), Task 23 (`factory_requests_match_ts_adapters`), Task 24 (`loop_scenarios_match_ts`).
5. **Headless.** No GUI crate in the daemon's tree on any target. Test: Task 4 (`scripts/ci/agentd-deps.sh` in CI, with a negative self-test).
6. **No big-bang switch.** Each flag has a working fallback and a test for it. Tests: Task 16 (`falls_back_to_ts_supervisor_when_daemon_absent`), Task 27 (`chat_api_unchanged_on_daemon_runtime`).

## File structure

```
Cargo.toml                                        # + workspace deps: loro, keyring, portable-pty, ts-rs, jsonschema, tokio-postgres, mysql_async, notify-rust …
deny.toml                                         # licences of the moved and new crates (Task 0, Task 4)
crates/loams-agentd/                              # bin + lib (from apps/desktop/native/apps/loams-desktop, rewritten)
  src/{main.rs,lib.rs,cli.rs,config.rs,paths.rs,lock.rs,discovery.rs,token.rs,hello.rs,server.rs,roles.rs,
       drain.rs,notify.rs,runtime_install.rs,import.rs,durable.rs}
  src/service/{mod.rs,systemd.rs,launchd.rs,windows.rs,linger.rs}
  tests/{lock.rs,discovery.rs,security.rs,service.rs,drain.rs,import.rs,durable.rs,e2e_support.rs}
  {LICENSE,NOTICE,THIRD_PARTY_NOTICES.md,SCOPED_NOTICE.md,import-provenance.json}
crates/loams-agentd-proto/                        # from crates/proto (+ brand, ts-rs, rpc.rs, fixtures)
  src/{lib.rs,agent.rs,entities.rs,view.rs,…,rpc.rs,daemon.rs,chat.rs,secrets.rs,factory.rs,engine.rs,stacks.rs,brand.rs}
  fixtures/*.json   tests/{fixtures.rs,ts_contract.rs}
crates/loams-agentd-sessions/                     # from crates/engine, remote parts removed
crates/loams-agentd-harness/                      # from crates/harness, cursor removed
crates/loams-agentd-rpc/                          # from crates/rpc, device_room removed, token + roles
crates/loams-agentd-doc/  crates/loams-agentd-store/  crates/loams-agentd-mcp/
crates/loams-agentd-link/  crates/loams-agentd-preview/
crates/loams-agentd-llm/                          # new: anthropic.rs, openai.rs, sse.rs, presets.rs, fixtures/
crates/loams-agentd-loop/                         # new: loop.rs, budgets.rs, registry.rs, scrub.rs, secret.rs, approvals.rs, harness.rs, transcript.rs, durable_turn.rs
crates/loams-agentd-tools/                        # new: engine.rs, live.rs, durable.rs, connectors.rs, sql/{lex.rs,caps.rs,pg.rs,mysql.rs,neon.rs}
crates/loams-agentd-factory/                      # new: ops.rs, apps.rs, health.rs, fixtures/
crates/loams-agentd-supervisor/                   # new: engine.rs, binary.rs, ports.rs, log_rotate.rs, env.rs, stacks.rs, compose.rs, tests/bin/fake-engine.rs
crates/loams-durable/src/{config.rs,embed.rs}     # serve_http (Task 25)
apps/desktop/                                     # deleted (Task 1; provenance moves to crates/loams-agentd)
apps/desktop-electron/
  scripts/fetch-agentd.mjs                        # Task 11
  electron-builder.config.cjs  NOTICE             # Tasks 11, 28, 37
  src/main/agentd/{client.ts,discover.ts,runtime.ts,mode.ts,consent.ts,upgrade.ts,ipc.electron.ts,relay.ts,import-chats.ts,import-secrets.ts}
  src/main/{index.ts,shell/{tray-model.ts,tray.electron.ts,quit.ts},engine/ipc.electron.ts,stacks/ipc.electron.ts,agent/ipc.electron.ts,factory/ipc.electron.ts,sql/ipc.electron.ts}
  src/preload/index.ts  src/shared/{contracts.ts,deeplink.ts}
  test/agentd-*.test.ts  test/e2e/{daemon.spec.ts,durability.spec.ts}
web/packages/agentd-client/                       # new: @loams/agentd-client
  src/{index.ts,types.ts,client.ts,frames.ts,reducer.ts,gen/**}  test/{fixtures.test.ts,client.test.ts,requests.test.ts}
web/packages/slots/src/                           # slot contract fields (Task 32)
web/plugins/agent/src/{session/*,chat/*,sessions/*,terminal/*,diffs/*,markdown.ts,index.tsx}
web/plugins/desktop-settings/src/{plugins.tsx,background.tsx}
web/apps/console/src/cordis/{desktop.ts,safe-mode.ts}
scripts/ci/agentd-deps.sh
.github/workflows/{ci.yml,monorepo.yml,desktop-electron.yml,desktop-electron-release.yml,desktop-agentd-e2e.yml}
docs/design/{50-loams-desktop-daemon.md,37-desktop-and-mobile-apps.md,13-decision-log.md,README.md}
docs/plans/{README.md,dd1-exit-report.md}  docs/security/agentd-threat-model.md  docs/guides/desktop/background-agents.md
```

## Shared contracts (all tasks use these names)

- **Binary and subcommands:** `loams-agentd run [--service|--child|--worker] --config <path>`, `loams-agentd service install|uninstall|start|stop|status|enable-linger|disable-linger`, `loams-agentd install-runtime --from <dir> --to <dir>`, `loams-agentd mcp`, `loams-agentd status`, `loams-agentd version`.
- **Exit codes:** `0` stopped; `3` already running; `4` invalid config; `70` internal error.
- **Service names:** systemd `loams-agentd.service`; launchd label `dev.loams.agentd`; Windows Run value `LoamsAgentd`; keyring service `dev.loams.agentd`.
- **Files in `<userData>/agentd/`:** `config.toml`, `agentd.json`, `agentd.token`, `agentd.lock`, `runtime/<version>/`, `runtime/current` (Windows `runtime/current.txt`), `store/`, `durable.db`, `logs/agentd.log`.
- **Bundled files in `resources/bin/`:** `loams`, `loams-agentd` (`.exe` on Windows), `SHA256SUMS`, `VERSION`.
- **Protocol:** `AGENTD_PROTOCOL: u32 = 1` (`loams_agentd_proto::rpc`).
- **Environment:** `LOAMS_AGENTD_RUN_TOKEN`, `LOAMS_AGENTD_PORT`, `LOAMS_AGENTD_SESSION_ID` (set only for the injected MCP shim); `LOAMS_AGENTD_CONFIG` (development override of `--config`).
- **Config (`config.toml`):**

  ```toml
  [paths]
  engine_data = "<userData>/engine"
  logs = "<logs>"
  stacks_source = "<resources>/stacks"
  stacks_run = "<userData>/stacks"
  connectors = "<resources>/connectors.json"
  [engine]
  auto_start = true
  [daemon]
  mode = "child"            # child | service
  notifications = true
  ```

- **Electron settings (`settings.json`):** `daemon.background` (`ask` | `on` | `off`, default `ask`), `daemon.linger` (default `false`), `daemon.notifications` (default `true`), `engine.owner` (`electron` | `daemon`), `agent.runtime` (`electron` | `daemon`), `plugins.disabled` (string array).
- **Rust types** (`loams-agentd-proto`):

  ```rust
  pub struct MethodSpec { pub name: &'static str, pub kind: MethodKind, pub params: &'static str,
                          pub result: &'static str, pub role: Role, pub renderer: bool }
  pub enum MethodKind { Unary, Stream }
  pub enum Role { Owner, Mcp }
  pub struct Hello { pub client: ClientKind, pub client_version: String, pub protocol: u32 }
  pub struct HelloReply { pub protocol: u32, pub version: String, pub build_sha: String, pub mode: DaemonMode,
                          pub pid: u32, pub started_at_ms: i64, pub role: Role, pub features: Vec<String> }
  pub enum DaemonMode { Service, Child }
  pub enum ToolRisk { Read, Write }
  pub enum ApprovalDecision { Once, Always, Deny }
  // AgentEvent gains:
  ApprovalRequested { call_id: String, tool: String, args: serde_json::Value, risk: ToolRisk },
  ApprovalResolved  { call_id: String, decision: ApprovalDecision },
  // HarnessId gains LoamsAgent (wire "loams-agent") and loses Cursor
  pub enum EngineState { Stopped, Starting { attempt: u32 }, Ready { url, es_url, flight_url, durable_url, live_url: Option<String>, pid },
                         Failed { reason: String, log_path: String } }
  pub struct DaemonStatus { pub mode: DaemonMode, pub draining: bool, pub running_turns: u32,
                            pub pending_approvals: u32, pub engine: EngineState, pub version: String }
  ```

- **New RPC methods** (with role and `renderer`): `Hello` (any, no), `DaemonStatus` (owner, yes), `WatchDaemonStatus` (owner, yes), `Drain` (owner, no), `Shutdown` (owner, no), `SetConfig` (owner, no), `EngineState` / `WatchEngine` / `StartEngine` / `StopEngine` (owner, yes), `SetLivePd` (owner, no), `EngineLogPath` (owner, yes), `StacksList` / `StackState` / `WatchStacks` / `StartStack` / `StopStack` / `StackLogPath` (owner, yes), `ListProviders` / `ConfigureProvider` / `TestProvider` (owner, yes), `ImportSecrets` (owner, no), `ImportChats` (owner, no), `ResolveApproval` (owner, yes), `WatchSession` (owner, yes), `FactoryList` / `FactoryConfigure` / `FactoryTest` / `FactoryRemove` / `FactoryQuery` (owner, yes), `PgTenants` / `PgTimelines` / `PgCreateBranch` / `PgWalStatus` / `PgConnection` / `PgRevealPassword` / `PgQuery` (owner, yes), `WesqlConnection` / `WesqlRevealPassword` / `WesqlSchemas` / `WesqlTables` / `WesqlQuery` (owner, yes). The fork's surviving methods keep their names; Task 9 sets their role and `renderer`.
- **Error reasons** (`{err}` text prefix, also the TypeScript `code`): `unauthenticated`, `forbidden_role`, `protocol_mismatch`, `agentd_draining`, `unknown_method`, `bad_params`, `key_required`, `unknown_provider`, `invalid_url`, `tool_unavailable_remote_server`, `not_found`, `engine_not_ready`, `keyring_unavailable`.
- **Resonate ids:** function `loams.agentd.turn`; promises `t<turnId>:llm:<n>`, `t<turnId>:approval:<callId>`, `t<turnId>:tool:<callId>:intent`, `t<turnId>:tool:<callId>`; group `loams-agentd`, process `agentd`.
- **Preload API** (`LoamsDesktopApi.agentd`, `apps/desktop-electron/src/shared/contracts.ts`):

  ```ts
  agentd: {
    status(): Promise<DaemonStatus | { unavailable: true; reason: string }>;
    onStatus(cb: (s: DaemonStatus | { unavailable: true; reason: string }) => void): () => void;
    call<M extends RendererMethod>(method: M, params: Params<M>): Promise<IpcResult<Result<M>>>;
    subscribe<M extends RendererStream>(method: M, params: Params<M>, cb: (item: Item<M>) => void,
                                        onEnd?: (err?: string) => void): () => void;
    background: { get(): Promise<'ask' | 'on' | 'off'>; set(v: 'on' | 'off'): Promise<IpcResult<void>>;
                  linger(on: boolean): Promise<IpcResult<void>>; stop(): Promise<void> };
  };
  ```

---

### Task 0: Reconcile with the code as built

**Files:** read:
- `apps/desktop/native/` (every crate, `Cargo.toml`, `Cargo.lock`, `LOAMS.md`, `NOTICE`, `THIRD_PARTY_NOTICES.md`, `SCOPED_NOTICE.md`), `apps/desktop/{project.json,import-provenance.json}`;
- `apps/desktop-electron/src/main/**`, `src/shared/contracts.ts`, `src/preload/index.ts`, `test/**`, `electron-builder.config.cjs`, `scripts/fetch-engine.mjs`;
- `crates/loams-durable/src/**`; root `Cargo.toml`, `Cargo.lock`, `deny.toml`;
- `web/plugins/agent/`, `web/packages/{slots,platform-electron,console-host}/`, `web/apps/console/src/cordis/desktop.ts`;
- `.github/workflows/{monorepo,ci,desktop-electron,desktop-electron-release,desktop-sign}.yml`;
- `dsh-desktop/{package.json,package-lock.json}`.

Write the reconciliation into this plan's "Rulings made during execution" (`T0-1`…).

**Consumes:** §50 §2 (each row checked against `dev`; every difference is listed with its resolution).

**Produces:** a findings note covering:
1. **The fork's RPC surface.** Every name in `rpc/src/lib.rs::methods`, marked keep, delete (edge, WorkOS, updates, push, GPUI-only) or renamed, with the deletions matching §50 §4.2. The list becomes Task 9's input.
2. **Edge and WorkOS reach.** Every module and function that touches `edge_url`, `EdgeConfig`, `Auth`, `LinkCache`, `HostRelay`, `chat2`, `/blob/`, `/preview/{org}/ws` or WorkOS; where removing it changes local behaviour (for example `WorkspaceHost` rows that only exist for sync).
3. **"Push" as built.** What the fork means by push (edge nudge, device relay, any mobile push) and where each lives, so Task 3 removes all of it.
4. **Dependency merge.** For every fork dependency: the root lock's version, whether one version satisfies both, `links` conflicts, and `deny.toml` verdicts. The new dependencies of the Tech Stack with versions at least 14 days old and their licences.
5. **`loams-durable`'s embed.** Whether a server can run with no listener today; the exact Rust SDK API for durable functions, steps (`ctx.run`) and promises that Task 25 uses; how a task is redelivered after a crash.
6. **Harness resume support.** For Claude Code, Codex, opencode, Pi and each ACP agent: how the driver resumes a native session today, if at all (input to Task 25 and Q706).
7. **Electron's userData and logs paths** on each OS as `app-paths.ts` resolves them (input to `config.toml`), and whether any path contains characters a systemd `ExecStart` or a launchd plist must escape.
8. **Platform facts flagged (verify) in §50:** SMAppService with an unsigned app; a logon-triggered scheduled task created by a standard user; polkit's default for `set-self-linger`; `notify-rust` on macOS from an unbundled binary; whether a systemd user manager can run in the CI container for Task 33. Each with its source.
9. **The owner's answers to Q700–Q714,** if given. Otherwise §50's defaults stand.

**Tests:** none (a reading task).

**Steps:** read the files listed; build nothing beyond `cargo metadata` and `cargo tree`; write the findings as `T0-*` rows; for each difference from §50, say which task absorbs it; commit.

**Commit:** `docs: DD1 task 0 findings`.

---

## DD1a — The headless daemon crates

### Task 1: Move the fork's headless crates into the root workspace

**Files:**
- moved with `git mv` (Ruling 1): `apps/desktop/native/crates/{engine,harness,proto,rpc,doc,sync,mcp,loams-desktop-link,preview}` → `crates/loams-agentd-{sessions,harness,proto,rpc,doc,store,mcp,link,preview}`; `apps/desktop/native/apps/loams-desktop` → `crates/loams-agentd`; `apps/desktop/native/{LICENSE,NOTICE,THIRD_PARTY_NOTICES.md,SCOPED_NOTICE.md}` and `apps/desktop/import-provenance.json` → `crates/loams-agentd/`;
- deleted: `apps/desktop/native/crates/{ui,voice,syntax,markdown,theme,loams-desktop-brand}` (brand strings move into `loams-agentd-proto/src/brand.rs`), `apps/desktop/native/{Cargo.toml,Cargo.lock,rust-toolchain.toml,README.md,LOAMS.md,dist,scripts}`, `apps/desktop/{project.json,README.md,VALIDATION.md,verify_import.py,test_verify_import.py}`;
- changed: root `Cargo.toml` (workspace dependencies), every moved `Cargo.toml` (package names, `workspace = true` dependencies, `license = "MIT"` kept), `crates/loams-agentd/src/main.rs`, `pnpm-workspace.yaml` or `nx.json` if they name `apps/desktop`.

**Consumes:** T0 items 1, 2 and 4.

**Produces:**
- packages `loams-agentd`, `loams-agentd-{sessions,harness,proto,rpc,doc,store,mcp,link,preview}`, all building in the root workspace;
- `loams-agentd` with `clap` subcommands `run`, `mcp`, `status`, `version` (the others arrive in Tasks 6 and 8). `run` is the fork's `headless` path. There is no headed mode, no `appshot`, no `--noop-browser`, and no dependency on any deleted crate;
- `sync` reduced to `loams-agentd-store` only as far as compiling requires in this task; its edge clients are deleted in Task 2.

**Tests:**
- Every moved crate's existing test suite passes under its new name, except tests of deleted features, each listed with its reason in the ruling row.
- `loams_agentd_version_runs`: `loams-agentd version` prints the workspace version and exits 0.
- `no_crate_named_loams_desktop`: a workspace test reads `cargo metadata` and fails if any package is named `loams-desktop*`.
- `scripts/docs/check-decision-ids.sh` reports no dangling citation under `crates/loams-agentd*`. The fork's `loams-desktop-link` cites decisions 467 and 468, which the decision log does not declare for this purpose (checked 2026-10-09); Task 1 rewrites those comments to cite §37 §18 instead.

**Steps:**
1. Write the two new tests (they fail: no package).
2. `git mv` each group in its own commit.
3. Merge dependencies into the root `Cargo.toml` per T0-4; remove `--target-dir` from every script that moved.
4. Delete the GUI crates and headed mode; fix imports until `cargo check -p loams-agentd` passes.
5. Run `cargo test -p <crate>` for each moved crate, one at a time.
6. Commit.

**Commit:** `agentd: move the zeron fork's headless crates into the root workspace`; `agentd: delete GPUI, headed mode and the UI-only crates`.

### Task 2: Strip the edge

**Files:**
- `crates/loams-agentd-rpc/src/{lib.rs,device_room.rs}` (deleted);
- `crates/loams-agentd-store/src/*` (keep `store.rs`, `types.rs`; delete `chat_client*`, `registry*`, `socket*`, `dial.rs`, `wake.rs`, `net_path.rs`, `sync_jobs.rs`, `budget.rs`, `chat_frames.rs`);
- `crates/loams-agentd-sessions/src/{lib.rs,doc_host.rs,workspace_host.rs,chat2_host.rs,diff_sync.rs,rpc.rs,sessions.rs}`;
- `crates/loams-agentd-preview/src/{signaling.rs,peer.rs,mux.rs,login.rs}` (deleted), `lib.rs`, `service.rs`;
- `crates/loams-agentd/src/main.rs` (`DEFAULT_EDGE_URL` and `edge_url_from_env` gone).

**Consumes:** T0-1, T0-2.

**Produces:**
- `EngineConfig` without `edge_url`, `edge_token`, `org_id`; `DocHostConfig`, `WorkspaceHostConfig` and `CheckoutDiffSync::start` without `EdgeConfig`;
- the local profile only (`EngineProfile::development` renamed `EngineProfile::local`);
- `PreviewService` with local discovery and routing only;
- the RPC methods T0-1 marked edge-only removed from `methods` and from `EngineRpc`.

**Tests:**
- `no_edge_symbols`: a test greps the `loams-agentd*` sources (excluding `tests/fixtures`) for `edge_url`, `EdgeConfig`, `device_room`, `chat2`, `HostRelay`, `LinkCache`, `/blob/`, `edge.loams.invalid` and fails on any hit.
- `local_session_roundtrip` (sessions): a mock-harness run in a fresh data dir produces the same doc rows as before the strip (the fork's existing mock-run test, kept green).
- `preview_local_routing_still_works`: the fork's local discovery test, kept green.

**Steps:** write `no_edge_symbols` (fails); delete module by module, keeping `cargo test -p loams-agentd-sessions` green after each; run the preview and rpc tests; commit.

**Commit:** `agentd: remove the edge sync, relay and preview signaling (D781)`.

### Task 3: Strip WorkOS, Cursor, self-update and push

**Files:**
- `crates/loams-agentd-sessions/src/{auth.rs,local_import.rs}` (deleted), `profile.rs`, `lib.rs`, `harness_updates.rs`, `rpc.rs`;
- `crates/loams-agentd/src/{auth_cli.rs,update_cli.rs}` (deleted), `main.rs`;
- `crates/loams-agentd-harness/src/cursor/` (deleted), `lib.rs`, `catalog.rs`;
- `crates/loams-agentd-proto/src/agent.rs` (`HarnessId::Cursor` removed);
- the `update` crate's last references.

**Consumes:** T0-1, T0-3.

**Produces:**
- no WorkOS client id, no `Auth`, no synced profile, no local-to-synced import;
- `HarnessId` without `Cursor`; a stored `"cursor"` harness value deserializes to `HarnessId::Unknown(String)` (new variant) and such sessions open read-only with the notice "This agent is no longer supported";
- `harness_updates.rs` without background polling (`start()` removed); the manual `UpdateHarness` method stays;
- no `UpdateStatus`, `ApplyUpdate`, nudge, relay, `RetryDelivery`, `RelayCommand`, `FocusChat` or connectivity methods.

**Tests:**
- `no_remote_feature_symbols`: grep, as in Task 2, for `workos`, `WORKOS`, `cursor_sdk`, `@cursor/sdk`, `Updater`, `ApplyUpdate`, `Nudge`, `RelayCommand`.
- `old_cursor_session_opens_read_only`: a doc fixture with harness `"cursor"` loads, and a send answers `harness_unsupported`.
- `harness_updates_has_no_timer`: constructing the coordinator spawns no task (a test hook counts spawns).

**Steps:** tests first; delete; keep `cargo test -p loams-agentd-sessions -p loams-agentd-harness -p loams-agentd-proto` green; commit.

**Commit:** `agentd: remove WorkOS, the Cursor shim, self-update and push (D781)`.

### Task 4: The headless guard and CI

**Files:** `scripts/ci/agentd-deps.sh`, `scripts/ci/agentd-deps.test.sh`, `.github/workflows/ci.yml`, `deny.toml`.

**Consumes:** Tasks 1–3.

**Produces:**
- `agentd-deps.sh` as in §50 §4.3; it prints the offending packages and their path (`cargo tree -i`);
- a CI job `agentd` on changes to `crates/loams-agentd*/**`, `Cargo.lock`, `scripts/ci/agentd-*`: `agentd-deps.sh`, `cargo clippy -p 'loams-agentd*' -- -D warnings`, `cargo test` for each `loams-agentd*` crate;
- (the `monorepo.yml` desktop job was already removed in Task 1, T1-4);
- `deny.toml` updated for the moved crates' licences (MIT for zeron code) and any new licence T0-4 found.

**Tests:**
- `agentd-deps.test.sh`: runs the script's filter on a canned `cargo tree` output containing `wry v0.50.0` and expects exit 1, and on a clean list expects exit 0.
- The `agentd` CI job is green on the PR.

**Steps:** write the self-test (fails); write the script; wire CI; `cargo deny check licenses`; drop the `missing_debug_implementations` and `clippy::unwrap_used` entries and the other T1-12 allow-list entries (T1-13) by fixing the code, so `cargo clippy -p 'loams-agentd*' --all-targets -- -D warnings` passes with no crate-level allow list beyond generated code; commit.

**Commit:** `ci: guard loams-agentd against GUI dependencies (D782)`.

### Task 5: Notices and provenance

**Files:** `crates/loams-agentd/{NOTICE,README.md}`, `crates/loams-agentd/import-provenance.json`, root `NOTICE` (if the repository has one), `docs/design/37-desktop-and-mobile-apps.md` (a status line on §18 pointing at §50).

**Consumes:** Task 1's moved notices.

**Produces:**
- `crates/loams-agentd/NOTICE`: zeron's MIT notice ("Copyright (c) 2026 Wing"), the statement that the `loams-agentd*` crates marked `license = "MIT"` derive from zeron at the commit in `import-provenance.json`, and that the other crates are Apache-2.0;
- `README.md`: what the daemon is, how to run it from a checkout (`cargo run -p loams-agentd -- run --child --config <file>`), and a link to §50;
- every moved crate's `Cargo.toml` keeps `license = "MIT"`; every new crate (Tasks 14–23) declares the workspace licence.

**Tests:** `licence_fields_are_set`: a workspace test reads `cargo metadata` and checks each `loams-agentd*` package's `license` against the table in `crates/loams-agentd/README.md`.

**Steps:** write `licence_fields_are_set` (fails until the table exists); write the NOTICE and README; set the fields; run `cargo test -p loams-agentd`; commit.

**Commit:** `docs: notices and provenance for loams-agentd`.

---

## DD1b — The process model and the contract

### Task 6: Data directory, single instance, discovery, token and `Hello`

**Files:** `crates/loams-agentd/src/{config.rs,paths.rs,lock.rs,discovery.rs,token.rs,hello.rs,main.rs,cli.rs}`, `tests/{lock.rs,discovery.rs}`; `crates/loams-agentd-proto/src/{rpc.rs,daemon.rs}`.

**Consumes:** the fork's `InstanceLock`; Shared contracts (files, exit codes, `Hello`).

**Produces:**
- `Config::load(path) -> Result<Config, ConfigError>` for `config.toml`; an invalid file exits 4 with the field named;
- `Paths::new(data_dir)` creating `<data>/agentd` as 0700 (an owner-only DACL on Windows);
- `lock::acquire(&Paths) -> Result<InstanceLock, AlreadyRunning>`;
- `token::rotate(&Paths) -> Result<Token, io::Error>`: 32 random bytes, base64url, written 0600 (owner-only DACL), atomically;
- `discovery::publish(&Paths, &Discovery)` (atomic, 0600) and `discovery::read(&Paths) -> Option<Discovery>`; removed on clean exit;
- the listener bound to `127.0.0.1:0`;
- `Hello` handling and `AGENTD_PROTOCOL = 1`;
- a second `run` exits 3 after confirming the first with `Hello`.

**Tests:**
- `second_instance_exits_3`: start one daemon in a temp data dir (under the test's target-relative temp root, never `/tmp`), start a second; it exits 3 and prints the first pid.
- `discovery_written_after_bind_and_removed_on_exit`.
- `token_file_is_owner_only`: mode `0o600` on Unix; on Windows the DACL has exactly the owner and SYSTEM.
- `token_rotates_every_start`.
- `hello_reports_versions_and_mode`.
- `protocol_mismatch_closes`: `Hello { protocol: 2 }` answers `protocol_mismatch` and the connection closes.
- `stale_discovery_is_ignored`: a discovery file with a dead pid does not stop a new daemon.
- `invalid_config_exits_4`.

**Steps:** tests first (they fail); implement; `cargo test -p loams-agentd`; commit.

**Commit:** `agentd: single instance, discovery, token and Hello (D787)`.

### Task 7: RPC security and roles

**Files:** `crates/loams-agentd-rpc/src/{server.rs,auth.rs,roles.rs,limits.rs}`, `crates/loams-agentd/src/{server.rs,roles.rs}`, `crates/loams-agentd-sessions/src/sessions.rs` (the MCP shim injection), `crates/loams-agentd-mcp/src/{lib.rs,loams_desktop.rs}`, `crates/loams-agentd/tests/security.rs`, `crates/loams-agentd-rpc/fuzz/` (target `frame`).

**Consumes:** Task 6's token and listener; the fork's `serve_ws_socket` Origin check.

**Produces:**
- the upgrade callback checks, in order: `Host == 127.0.0.1:<port>` (403); no `Origin` (403); `Authorization: Bearer` matching the main token (role `Owner`) or a live scoped run token (role `Mcp`) in constant time (401); otherwise the upgrade proceeds;
- `OPTIONS` and any non-upgrade request answer 403 with no `Access-Control-*` header;
- dispatch checks each method's `Role` from `METHODS` (`forbidden_role`), and for `Mcp` that the call's `sessionId` equals the token's session;
- `ScopedTokens::issue(session_id) -> RunToken` and `revoke(run_id)`; the MCP shim gets `LOAMS_AGENTD_RUN_TOKEN`, `LOAMS_AGENTD_PORT`, `LOAMS_AGENTD_SESSION_ID`, never the main token; `loams-agentd mcp` reads those;
- limits: 16 MiB frames, 64 streams per connection, 8 owner connections, and 250 ms added after each failed upgrade;
- a failed authentication is logged with the peer port only.

**Tests:**
- `upgrade_without_token_is_401`, `wrong_token_is_401`, `origin_header_is_403` (even with the right token), `foreign_host_is_403` (`Host: evil.example:port`), `options_is_403_without_cors_headers`.
- `scoped_token_cannot_resolve_approval`, `scoped_token_bound_to_session`, `scoped_token_dies_with_run`.
- `mcp_shim_env_has_no_main_token`: the injected `McpServer.env` holds only the three scoped keys.
- `token_never_logged`: a `tracing` capture over the auth tests contains neither token.
- `frame_limit_enforced`.
- Fuzz target `frame` builds; CI runs it 60 s.

**Steps:** tests first; implement; run `cargo test -p loams-agentd-rpc -p loams-agentd`; commit.

**Commit:** `agentd: token, Origin, Host and role checks on the RPC (D788)`.

### Task 8: Per-user services and linger

**Files:** `crates/loams-agentd/src/service/{mod.rs,systemd.rs,launchd.rs,windows.rs,linger.rs}`, `src/runtime_install.rs`, `src/main.rs`, `tests/service.rs`.

**Consumes:** Task 6's paths and exit codes; the fork's `daemon.rs` (plist and unit rendering, `exec_path_for`).

**Produces:**
- `service install|uninstall|start|stop|status`:
  - Linux: renders `loams-agentd.service` exactly as §50 §5.1 (quoted `ExecStart` to `runtime/current/loams-agentd run --service --config "<config>"`, no `Environment=` lines), `daemon-reload`, `enable --now`;
  - macOS: renders `dev.loams.agentd.plist` (`RunAtLoad`, `KeepAlive {SuccessfulExit=false, Crashed=true}`, `ThrottleInterval 10`, stdout and stderr to `logs/agentd.log`), `launchctl bootstrap gui/<uid>`;
  - Windows: writes the Run value with the quoted command and starts it detached;
- `run --service` on Windows: the parent spawns `run --worker` in a Job Object (kill on close) and restarts it on exit codes other than 0, 3 and 4 with backoff 1, 2, 4, 8, 16, 30 s, at most 5 in 10 minutes;
- `service enable-linger|disable-linger` (Linux only; elsewhere `unsupported`), running `loginctl enable-linger|disable-linger "$USER"` and returning its error text;
- `install-runtime --from <dir> --to <dir>` (Ruling 5): verifies `SHA256SUMS`, copies both binaries to `<to>/<VERSION>/`, switches `current` atomically (symlink rename; `current.txt` on Windows), and keeps the newest two versions;
- the login-shell `PATH` resolved at start through the harness's `shell_env`.

**Tests:**
- `systemd_unit_golden`, `launchd_plist_golden`, `windows_run_value_golden`: rendering against golden files, with a `userData` path containing a space.
- `unit_has_no_environment_lines`.
- `install_runtime_checks_hashes`: a tampered copy fails and `current` does not move.
- `install_runtime_keeps_two_versions`.
- `windows_parent_restarts_worker` (Windows only; `#[cfg(windows)]`, runs in the Windows CI job): a worker that exits 70 is restarted; one that exits 4 is not.
- `linger_unsupported_off_linux`.
- `systemd_install_roundtrip` (Linux, skipped with `skipped: needs a systemd user manager` when `systemctl --user is-system-running` fails): install, status shows active, uninstall removes the unit.

**Steps:** goldens first; implement per OS; commit.

**Commit:** `agentd: per-user service install, the runtime directory and linger (D784, D786)`.

### Task 9: Wire types, the method table and fixtures (ts-rs)

**Files:**
- `crates/loams-agentd-proto/src/{lib.rs,rpc.rs,daemon.rs,chat.rs,secrets.rs,factory.rs,engine.rs,stacks.rs}` and every existing type file (`#[derive(TS)]`);
- `crates/loams-agentd-proto/{fixtures/*.json,tests/{fixtures.rs,ts_contract.rs}}`;
- new package `web/packages/agentd-client/{package.json,tsconfig.json,vitest.config.ts,src/{index.ts,types.ts,gen/**,fixtures.gen.ts},test/{fixtures.test.ts,requests.test.ts}}`;
- `pnpm-workspace.yaml` (if packages are listed), Nx project wiring.

**Consumes:** T0-1's surviving method list; Shared contracts.

**Produces:**
- `#[derive(TS)] #[ts(export)]` on every wire type, with `export_to` the package's `src/gen/`;
- `METHODS: &[MethodSpec]` covering every surviving and new method, each with `role` and `renderer`;
- a test `export_bindings` that writes `src/gen/*.ts`, `src/gen/methods.ts` (the table and the `Methods` type map) and `src/fixtures.gen.ts`;
- `@loams/agentd-client` exporting `./types` (generated types and the method map; no Node imports) and `./client` (Task 10);
- canonical fixtures for every type in `fixtures/`, written by `write_fixtures` from typed constructors.

**Tests:**
- Rust: `fixtures_roundtrip` (deserialize, serialize, identical canonical JSON); `ts_bindings_are_fresh` (regenerate into a temp dir under the target directory and diff); `methods_unique_and_complete` (unique names; every `EngineRpc` dispatch arm has a spec and vice versa); `ts_requests_parse` (every entry of `web/packages/agentd-client/test/requests.json` parses into its method's params type).
- TypeScript: `fixtures.test.ts` (`tsc` checks `fixtures.gen.ts`'s `satisfies` lines; every `AgentEvent` fixture passes through `reduceSession`, whose `switch` ends in `assertNever`); `requests.test.ts` (typed builders write `requests.json`; the test fails if the committed file differs).

**Steps:** write the Rust tests and an empty package (they fail); add the derives and the table; generate; write the TypeScript tests; commit generated files with the source.

**Commit:** `agentd: generated TypeScript wire types and method table with ts-rs (D789)`.

### Task 10: The Node client for Electron main

**Files:** `web/packages/agentd-client/src/{client.ts,frames.ts,discover.ts,errors.ts}`, `test/{client.test.ts,fake-daemon.ts}`; `apps/desktop-electron/src/main/agentd/{client.ts,discover.ts}`.

**Consumes:** Task 9's types; Task 6's discovery and token files; Task 7's rules.

**Produces:**

```ts
export class AgentdClient {
  static async connect(opts: { dataDir: string; clientVersion: string; signal?: AbortSignal }): Promise<AgentdClient>;
  readonly hello: HelloReply;
  call<M extends UnaryMethod>(method: M, params: Params<M>, opts?: { signal?: AbortSignal; timeoutMs?: number }): Promise<Result<M>>;
  subscribe<M extends StreamMethod>(method: M, params: Params<M>, onItem: (i: Item<M>) => void): Subscription; // { done: Promise<void>; cancel(): void }
  on(event: 'disconnected' | 'reconnected', cb: () => void): () => void;
  close(): Promise<void>;
}
```

- `ws` with `headers: { authorization: 'Bearer …' }`, no `origin`;
- reconnect with backoff 0.25, 0.5, 1, 2, 4 s (max 4 s), re-reading discovery and the token each time, and re-issuing open subscriptions;
- `AgentdError { code, message }` from `{err}` frames (the Shared contracts reasons);
- the token is held in a private field and never logged; `toJSON` and `inspect` show `[redacted]`.

**Tests:** against `fake-daemon.ts` (a `ws` server implementing `Hello`, a unary echo, a stream and errors):
- `client_sends_token_and_no_origin` (asserts the upgrade headers);
- `hello_mismatch_rejects`;
- `stream_items_then_done`, `cancel_sends_cancel_frame`;
- `reconnect_reissues_subscriptions` (the fake daemon restarts on a new port and rotates its token);
- `errors_carry_codes`;
- `token_not_in_errors_or_inspect`.

**Steps:** write the fake daemon and the six tests (they fail); implement `frames.ts`, `discover.ts`, then `client.ts`; wire `apps/desktop-electron/src/main/agentd/client.ts` as a thin factory; run `pnpm --filter @loams/agentd-client test` and the desktop typecheck; commit.

**Commit:** `web: @loams/agentd-client, the Node client for Electron main`.

### Task 11: The daemon ships alongside

**Files:**
- `apps/desktop-electron/scripts/fetch-agentd.mjs`, `electron-builder.config.cjs`, `package.json` (scripts `fetch-agentd`);
- `apps/desktop-electron/src/main/agentd/{runtime.ts,mode.ts,ipc.electron.ts,relay.ts}`, `src/main/index.ts`, `src/preload/index.ts`, `src/shared/contracts.ts`, `src/main/shell/{tray-model.ts,tray.electron.ts}`;
- `.github/workflows/{desktop-electron.yml,desktop-electron-release.yml}`, `desktop-sign.yml` and the SignPath artifact configuration;
- `apps/desktop-electron/test/{agentd-runtime.test.ts,agentd-preload.test.ts,tray.test.ts}`, `test/e2e/daemon.spec.ts`.

**Consumes:** Tasks 6, 8, 9, 10.

**Produces:**
- `fetch-agentd.mjs`: like `fetch-engine.mjs` (`LOAMS_AGENTD_BIN`, else `cargo metadata`'s target directory; strip on Unix; never `CARGO_TARGET_DIR`), then writes `resources/bin/SHA256SUMS` (both binaries) and `resources/bin/VERSION`;
- at launch, Electron runs `install-runtime` when `runtime/<VERSION>` is missing, writes `config.toml` (Shared contracts) and starts `runtime/current/loams-agentd run --child` with stdout and stderr to `<logs>/agentd.log`; it connects with `AgentdClient` and logs the `Hello`;
- in this task nothing else uses the daemon: `engine.owner` and `agent.runtime` stay `electron`;
- preload's `agentd` namespace (Shared contracts) with `call`/`subscribe` forwarded only for `renderer: true` methods (the list generated from `methods.ts`), subscriptions owned per `webContents` and cancelled on navigation, reload or close;
- the tray gains a line "Background agents: running" / "stopped" / "unavailable";
- the release workflows build `cargo build --release -p loams-agentd` with the engine, run `fetch-agentd`, and include `loams-agentd.exe` in SignPath's Windows artifact set.

**Tests:**
- `runtime_installed_once_per_version`, `config_toml_written_with_paths` (Vitest, with a fake `execFile`).
- `preload_forwards_only_renderer_methods`: calling `Drain` or `ImportSecrets` through the preload bridge is refused before main touches the client.
- `subscriptions_cancelled_on_reload`.
- `tray_shows_daemon_line`.
- e2e `daemon.spec.ts` (Linux): the packaged app starts, the daemon answers `Hello`, quitting the app stops the child daemon (child mode).
- The existing Playwright smoke stays green.

**Steps:** write the Vitest tests and the e2e spec (they fail); write `fetch-agentd.mjs` and the builder entries; add the runtime install, config writer and child start to `index.ts` behind a `try` that logs and continues (the app must start even if the daemon cannot); add the preload bridge and relay; update the workflows; run the desktop tests, `pnpm run package --linux --dir` and the smoke; commit.

**Commit:** `desktop: bundle loams-agentd and run it alongside in child mode`.

### Task 12: Consent, background mode, tray and explicit stop

**Files:** `apps/desktop-electron/src/main/agentd/{consent.ts,mode.ts}`, `src/main/shell/{tray-model.ts,quit.ts}`, `src/main/index.ts`, `src/shared/{contracts.ts,deeplink.ts}`, `web/plugins/desktop-settings/src/background.tsx`, `web/plugins/agent/src/consent.tsx`, `crates/loams-agentd/src/{notify.rs,drain.rs}`, tests in both.

**Consumes:** Tasks 8, 11; §50 §5.5–§5.6.

**Produces:**
- **Consent:** the first agent action, or opening Settings › Background agents while `daemon.background = ask`, shows the dialog of §50 §5.5 with no pre-selected button. The choice is stored, never asked again, and changeable in Settings.
- **`on`:** `service install`, then Electron reconnects to the service daemon; `off`: `service uninstall` if installed, child mode.
- **Quit:** in child mode, `Drain` then `Shutdown`; in service mode, nothing is stopped. `quitSequence` gains a `releaseDaemon` step before `stopEngine`.
- **Tray:** "Background agents: running · N turns · M approvals", "Stop background agents" (`Drain`, then `service stop`), "Quit Loams Desktop" with the hint "(agents keep running)" in service mode. The badge counts pending approvals from `WatchDaemonStatus`.
- **Settings › Background agents** (`web/plugins/desktop-settings`): the mode, "Keep running after I log out (Linux)" (only Linux, only in service mode, calls `enable-linger`/`disable-linger`, shows their errors), notifications on or off, "Remove background service".
- **Notifications** (daemon): with no `owner` client connected, a pending approval or question and a failed turn post an OS notification (Linux D-Bus, Windows toast; none on macOS in DD1) whose action opens `loams://open/agent/<sessionId>`; the deep-link allowlist gains `agent`.
- **Drain** (daemon): §50 §6.2 steps 1, 5 and 6 (the turn steps arrive with Tasks 24–25).

**Tests:**
- `consent_asked_once_without_default`, `background_on_installs_service`, `background_off_uninstalls_service` (fake service runner).
- `quit_child_mode_drains_then_shuts_down`, `quit_service_mode_leaves_daemon`.
- `tray_model_counts_from_daemon`, `stop_background_agents_drains_first`.
- `linger_toggle_hidden_off_linux`, `linger_errors_shown`.
- `deeplink_agent_allowed`, `deeplink_agent_rejects_bad_id`.
- Rust: `notifies_only_without_owner_client`, `notification_carries_deeplink` (a fake notifier).

**Steps:** tests first in both languages; implement the consent model as a pure module (`consent.ts`) with an Electron wrapper; the tray model; the settings section; the daemon's notifier behind a trait with a fake; run the desktop tests, the plugin tests and `cargo test -p loams-agentd`; commit.

**Commit:** `desktop: background consent, tray state and explicit stop (D785, D786)`.

### Task 13: Upgrades

**Files:** `apps/desktop-electron/src/main/agentd/upgrade.ts`, `src/main/update/updater.electron.ts`, `src/main/index.ts`, `crates/loams-agentd/src/drain.rs`, tests.

**Consumes:** Tasks 8, 11, 12; §50 §5.3.

**Produces:**
- at launch and after each reconnect, `planUpgrade(bundled: string, hello: HelloReply) -> 'none' | 'replace'`; `replace` when the versions or the protocol differ (the bundle is authoritative, downgrades included);
- `replace`: `install-runtime`, `Drain { reason: 'upgrade', timeoutMs: 60000 }`, then restart through the service manager (service mode) or a new child (child mode); a protocol mismatch skips `Drain` (the old daemon is told to `Shutdown` if it accepts the call, else its pid is terminated);
- `prepareToInstall` no longer stops the engine when `engine.owner = daemon`;
- the daemon's `Drain` replies when drained and then exits 0 in service mode (the manager restarts it with the new `current`).

**Tests:**
- `plan_upgrade_table` (same, newer, older, protocol mismatch).
- `upgrade_drains_then_restarts` (fake client and service runner).
- `protocol_mismatch_skips_drain`.
- `prepare_to_install_keeps_daemon_engine`.
- Rust: `drain_then_exit_zero_in_service_mode`.

**Steps:** write the pure `planUpgrade` table test first; implement; wire into the launch path after Task 11's runtime install; adjust `prepareToInstall`; run the tests and a manual upgrade between two locally built versions (recorded in the PR); commit.

**Commit:** `desktop: upgrade the daemon from the bundle after an app update (D798)`.

---

## DD1c — Supervision moves to the daemon

### Task 14: The engine supervisor in Rust

**Files:** new crate `crates/loams-agentd-supervisor/{Cargo.toml,src/{lib.rs,engine.rs,binary.rs,ports.rs,log_rotate.rs,env.rs},tests/{engine.rs,bin/fake-engine.rs}}`; `crates/loams-agentd/src/server.rs` (RPC wiring); `crates/loams-agentd-proto/src/engine.rs`.

**Consumes:** `apps/desktop-electron/src/main/engine/{supervisor,binary,ports,log-rotate}.ts` and `test/engine.test.ts`; Task 6's config.

**Produces:**

```rust
pub struct EngineSupervisor { /* … */ }
impl EngineSupervisor {
    pub fn new(cfg: EngineSupervisorConfig) -> Self;  // binary, data_dir, log_file, live_pd, grace, poll, timeouts
    pub fn state(&self) -> EngineState;
    pub fn watch(&self) -> tokio::sync::watch::Receiver<EngineState>;
    pub fn start(&self);
    pub async fn stop(&self);
    pub async fn dispose(&self);
    pub async fn set_live_pd(&self, pd: Option<String>);
    pub async fn adopt_or_start(&self, record: Option<EngineRecord>); // §50 §9.1 adoption
}
pub fn engine_args(o: &EngineArgsOpts) -> Vec<String>;     // same flags and order as engineArgs
pub fn help_supports_live(help: &str) -> bool;              // also matches --live-store (LV1 T0-14)
pub fn scrub_env(env: impl Iterator<Item=(OsString, OsString)>) -> Vec<(OsString, OsString)>;
```

with every number of §50 §9.1, `PR_SET_PDEATHSIG` on Linux, a Job Object on Windows, and `store/engine.json` for adoption; RPC `EngineState`, `WatchEngine`, `StartEngine`, `StopEngine`, `SetLivePd`, `EngineLogPath`.

**Tests** (`fake-engine` serves `GetInstance` and can crash, hang or exit on command):
- the cases of `engine.test.ts`, one Rust test each: `ready_after_get_instance_200`, `backoff_sequence_1_2_4_8_16_30`, `gives_up_after_5_restarts_in_10_minutes`, `not_ready_within_timeout_kills_and_fails`, `stop_sends_term_then_kill_after_grace`, `live_flags_only_when_supported_and_pd_set`, `set_live_pd_restarts_running_engine`, `missing_binary_fails_with_hint`.
- `engine_args_match_ts`: the argument vectors equal those recorded from `engineArgs` for four cases (fixtures written by a Vitest test in `apps/desktop-electron/test/engine-args-fixture.test.ts`).
- `engine_env_is_scrubbed`: `LOAMS_X_TOKEN`, `FOO_SECRET`, `BAR_API_KEY` and `LOAMS_AGENTD_RUN_TOKEN` are absent from the child's environment (the fake engine echoes its environment names).
- `log_rotates_at_10mb_keeps_5`.
- `adopts_running_engine`, `replaces_foreign_pid`.
- `engine_dies_with_daemon` (Linux).

**Steps:** export the argument fixtures from TypeScript; write the fake engine and the Rust tests (they fail); port module by module (`binary`, `ports`, `log_rotate`, `env`, then `engine`); wire the RPC; run `cargo test -p loams-agentd-supervisor`; commit.

**Commit:** `agentd: the engine supervisor, ported from Electron (D790)`.

### Task 15: The stack manager in Rust

**Files:** `crates/loams-agentd-supervisor/src/{stacks.rs,compose.rs}`, `tests/stacks.rs`; `crates/loams-agentd/src/server.rs`; `crates/loams-agentd-proto/src/stacks.rs`.

**Consumes:** `apps/desktop-electron/src/main/stacks/{stacks,runtime,ipc.electron}.ts`, `test/stacks.test.ts`.

**Produces:** `StackManager` with the `STACKS` table (postgres, wesql, tikv: directories, required services, ports), runtime detection, the per-user copy keyed by version, `start`/`stop`/`state`/`watch`, `tikv_ready`, the shared-port group, timeouts, logs, and `bind_live_to_tikv(&StackManager, &EngineSupervisor)`; RPC `StacksList`, `StackState`, `WatchStacks`, `StartStack`, `StopStack`, `StackLogPath`.

**Tests:** the cases of `stacks.test.ts` against a fake compose runner (`compose_runtime_detection_order`, `copy_once_per_version`, `required_services_decide_running`, `tikv_ready_requires_health_and_up_store`, `shared_port_group_blocks_second_stack`, `command_timeout_5_minutes`, `live_pd_follows_tikv_state`).

**Steps:** port the `stacks.test.ts` cases first against a fake compose runner; implement; wire the RPC and the Live binding; run `cargo test -p loams-agentd-supervisor`; commit.

**Commit:** `agentd: the compose stack manager, ported from Electron (D790)`.

### Task 16: Electron behind `engine.owner`

**Files:** `apps/desktop-electron/src/main/{index.ts,engine/ipc.electron.ts,stacks/ipc.electron.ts,protocol/handler.electron.ts,servers/registry.ts,shell/tray.electron.ts}`, `src/main/agentd/owner.ts`, tests.

**Consumes:** Tasks 11, 14, 15.

**Produces:**
- `engineOwner(settings, daemonUp) -> 'electron' | 'daemon'`;
- with `daemon`: the engine and stacks IPC handlers call the daemon (same `IpcResult` shapes), `engineState()` for the protocol handler and `registry.setLocalUrl` follow `WatchEngine`, `bindLiveToTikv` is not started in Electron, and the TypeScript supervisor is not constructed;
- if the daemon becomes unavailable while `engine.owner = daemon`, Electron constructs its own supervisor (as today) and the tray says "Engine: managed by the app (daemon unavailable)";
- default stays `electron` in this task.

**Tests:** `engine_ipc_routes_to_daemon`, `stacks_ipc_routes_to_daemon`, `falls_back_to_ts_supervisor_when_daemon_absent`, `local_url_follows_daemon_engine`, `no_double_supervision` (never both supervisors running).

**Steps:** tests first with a fake client; implement `owner.ts` as a pure decision and switch each IPC handler behind it; run the desktop tests and the smoke with `engine.owner` set both ways; commit.

**Commit:** `desktop: read the engine and stacks from the daemon behind engine.owner`.

### Task 17: The engine default moves to the daemon

**Files:** `apps/desktop-electron/src/main/settings.ts` (defaults), `test/e2e/daemon.spec.ts`, `docs/design/50-loams-desktop-daemon.md` (§9.3 as built).

**Consumes:** Tasks 12, 16.

**Produces:** `engine.owner` defaults to `daemon`; in service mode the engine keeps serving after Electron quits.

**Tests:** e2e (Linux, service mode via the Task 8 harness or the self-supervisor when no systemd user manager exists): `engine_survives_app_quit` (after quit, `GetInstance` on the engine URL from `agentd.json`'s `EngineState` answers 200), `reopen_shows_ready_engine_without_restart` (same pid). The full smoke stays green.

**Steps:** write the e2e tests (they fail with the old default); flip the default; run the e2e suite and the smoke; update §50 §9.3; commit.

**Commit:** `desktop: the daemon supervises the engine by default`.

---

## DD1d — The agent in Rust

### Task 18: Providers

**Files:** new crate `crates/loams-agentd-llm/{src/{lib.rs,types.rs,sse.rs,anthropic.rs,openai.rs,presets.rs,errors.rs},fixtures/**,tests/{anthropic.rs,openai.rs,sse.rs}}`; `apps/desktop-electron/test/fixtures/agent-sse/**` (exported once from the TypeScript tests).

**Consumes:** `apps/desktop-electron/src/main/agent/providers/*`, `test/agent-providers.test.ts`.

**Produces:**

```rust
pub enum ProviderEvent { Text{index,text}, Thinking{index,text}, Signature{index,signature}, RedactedThinking{index,data},
    ToolUse{index,id,name,input: Option<Value>,raw: String}, Fallback{index,from:Option<String>,to:Option<String>},
    Model{model}, Usage{input_tokens,output_tokens}, Stop{reason: ProviderStop} }
pub enum ProviderStop { EndTurn, ToolUse, MaxTokens, Refusal, Other }
pub struct TurnRequest { pub model: String, pub system: String, pub messages: Vec<Msg>, pub tools: Vec<ProviderTool> }
#[async_trait] pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    async fn stream_turn(&self, req: TurnRequest, cancel: CancellationToken) -> Result<BoxStream<'static, Result<ProviderEvent, ProviderError>>, ProviderError>;
}
pub fn anthropic(o: AnthropicOptions) -> Arc<dyn Provider>;   // base_url, api_key: Secret, fallback: bool, http
pub fn openai_compatible(o: OpenAiOptions) -> Arc<dyn Provider>; // id, base_url, api_key: Option<Secret>, include_usage
pub const PRESETS: &[Preset];                                   // anthropic, deepseek, openai, ollama
pub fn check_base_url(raw: &str) -> Result<Url, &'static str>;  // same messages as checkBaseUrl
```

with §50 §10.2's caching, adaptive thinking, fallback and usage rules; `ProviderError { status, message }` with the server's message capped at 500 characters.

**Tests:**
- `sse_fixtures_match_ts_events`: each recorded SSE stream (exported from the TypeScript provider tests with their expected events) yields the same events in Rust.
- `anthropic_request_golden`: the request body for a two-turn chat with tools (cache_control on system, last user block and top level for first-party; none for a proxy base URL).
- `fallback_only_when_opted_in_and_supported` (header and body).
- `adaptive_thinking_for_current_models`.
- `openai_tool_call_deltas_assemble`, `openai_refusal_maps_to_refusal`, `include_usage_per_preset`.
- `invalid_tool_json_kept_raw`.
- `check_base_url_rules` (the TypeScript cases).
- `error_body_capped_and_prefers_json_message`.

**Steps:** export the SSE fixtures from the TypeScript provider tests; write the Rust tests (they fail); port `sse`, then `anthropic`, `openai`, `presets`; run `cargo test -p loams-agentd-llm`; commit.

**Commit:** `agentd: Anthropic and OpenAI-compatible providers (D791)`.

### Task 19: Tool registry, scrubbing and secrets in memory

**Files:** new crate `crates/loams-agentd-loop/src/{lib.rs,registry.rs,scrub.rs,secret.rs}`, `tests/{registry.rs,scrub.rs,canary.rs}`.

**Consumes:** `agent/tools.ts`, `loop.ts::scrub`, `factory/host.ts::secretForms`, `redact.ts`, their tests.

**Produces:**

```rust
pub struct Secret(/* private */);          // Debug, Display, Serialize print "[redacted]"; reveal() -> &str
pub fn secret_forms(secrets: &[&str], plain: &[&str]) -> Vec<String>;  // raw, URL, form, base64 (±padding), Basic pairs
pub fn redact(text: &str, forms: &[String]) -> Cow<str>;
pub fn scrub_value(v: &mut serde_json::Value, forms: &[String]);
pub struct ToolDef { pub name: String, pub description: String, pub risk: ToolRisk, pub schema: Value, pub run: ToolFn }
pub struct ToolRegistry;  // register (name rule, duplicates, schema compiles), get, check(name, args) -> Option<String>, list
pub fn result_text(v: &ToolOutput, max: usize) -> String;  // 20 000 default, truncation note as truncate()
```

**Tests:** the TypeScript cases ported (`bad_name_rejected`, `duplicate_rejected`, `schema_errors_joined`, `truncate_note_exact`); `secret_forms_match_ts` (fixture from `redact.test.ts`); `canary_never_leaves`: a `Secret` canary passed through `Debug`, `Display`, `serde_json`, `tracing` fields, `anyhow` chains and `redact` never appears in output.

**Steps:** export the `redact` fixtures; tests first; implement; run `cargo test -p loams-agentd-loop`; commit.

**Commit:** `agentd: tool registry, secret type and scrubbing`.

### Task 20: Loams tools on the local engine

**Files:** new crate `crates/loams-agentd-tools/src/{lib.rs,engine.rs,live.rs,durable.rs,connectors.rs,remote.rs}`, `tests/{engine.rs,live.rs}` with an in-process fake engine.

**Consumes:** `agent/builtin-tools.ts`, `agent-tools/live.ts`, `agent/sql-guard.ts`, their tests; Task 14's `EngineState` URLs; Task 19's registry.

**Produces:** the read and write tools of §50 §10.4 except SQL-on-stacks and factory, with the same names, descriptions, JSON Schemas and risk tags (schemas copied from the TypeScript definitions into `schemas/*.json` and shared by a fixture test); `sql_query` with `read_only_violation` before the engine call; `connectors_search` over the bundled catalogue; every tool answers `tool_unavailable_remote_server` with a one-line explanation when Electron reports a remote active server (`SetConfig { activeServer: { kind } }`).

**Tests:**
- `tool_schemas_match_ts` (names, descriptions, risks, schemas equal to the exported TypeScript list).
- One behaviour test per tool against the fake engine, including `sql_query_refuses_writes_and_explain_analyze`, `live_mutate_is_write`, `durable_promise_create_is_write`.
- `remote_server_tools_explain`.
- `tool_errors_are_scrubbed`.

**Steps:** export the TypeScript tool list (names, descriptions, risks, schemas) to `schemas/`; write the fake engine and the tests; port the tools one by one; run `cargo test -p loams-agentd-tools`; commit.

**Commit:** `agentd: Loams engine tools over the local engine (D792)`.

### Task 21: SQL services and least-privilege tools

**Files:** `crates/loams-agentd-tools/src/sql/{lex.rs,caps.rs,pg.rs,mysql.rs,neon.rs}`, `tests/sql.rs`, `fixtures/sql-lex-corpus.json` (exported from `sql-lex.test.ts`); `crates/loams-agentd/src/server.rs` (the `Pg*` and `Wesql*` methods).

**Consumes:** `src/shared/sql-lex.ts`, `src/main/sql/{caps,pg,wesql,neon}.ts`, their tests.

**Produces:** the lexer (`is_single_statement`, `is_write`, `strip_sql` for postgres and mysql dialects); `SqlSession` over `tokio-postgres` (extended protocol, one statement, cursor fetch to the row cap) and `mysql_async` (streamed rows, connection dropped at the cap); caps 1 000 rows and 30 s; the read-only wrapper (`BEGIN READ ONLY` … `ROLLBACK`, `SET TRANSACTION READ ONLY` for mysql); tools `pg_sql`, `wesql_sql` (read) and `pg_branch_create` (write); the page RPCs with the exact shapes of the current IPC handlers; passwords returned only by `PgRevealPassword`/`WesqlRevealPassword` (owner role, renderer allowed, as today's reveal button).

**Tests:** `sql_lex_corpus_agrees` (every corpus case gives the TypeScript verdict); `pg_read_only_rolls_back` and `pg_single_statement_enforced` (against the neon stack when `LOAMS_TEST_PG` is set, else skipped with a message, as `sql-integration.test.ts`); `mysql_stream_stops_at_cap`; `timeout_maps_to_code`; `errors_redact_passwords`.

**Steps:** export the lexer corpus; tests first; port `lex`, `caps`, then the pg and mysql sessions and the tools; wire the page RPCs; run `cargo test -p loams-agentd-tools` (and the live-database cases with `LOAMS_TEST_PG` when a stack is up); commit.

**Commit:** `agentd: SQL services and least-privilege SQL tools`.

### Task 22: Secrets in the keyring and their import

**Files:** `crates/loams-agentd-loop/src/keys.rs`, `crates/loams-agentd/src/{secrets.rs,import.rs}`, `crates/loams-agentd/tests/secrets.rs`; `apps/desktop-electron/src/main/agentd/import-secrets.ts`, `test/agentd-import-secrets.test.ts`.

**Consumes:** `agent/providers/presets.ts::ProviderConfigs`, `factory/vault.ts`; Task 18's presets; Task 19's `Secret`.

**Produces:**
- `SecretStore` over `keyring` (service `dev.loams.agentd`, accounts `agent:<provider>`, `factory:<app>:<field>`) with a memory fallback (`persistent: false`) when the keyring is unavailable;
- `ProviderConfigs` (port): saved base URL, model and fallback in `store/providers.json`; the key bound to the origin it was entered for; `info`, `list`, `configure`, `create`;
- RPC `ListProviders`, `ConfigureProvider`, `TestProvider` (one tiny request, 20 s timeout, scrubbed errors), `ImportSecrets { entries: [{ key, url, fields }] } -> { imported, failed: [key] }`;
- Electron, on the first `agent.runtime = daemon` start (Task 27 calls it): decrypt the vault, `ImportSecrets`, verify `hasKey` per entry through `ListProviders` and `FactoryList`, then delete `credentials.bin`; on any failure keep the file and retry at the next start.

**Tests:**
- Rust: `key_is_origin_bound` (changing the base URL's origin without a key deletes it and answers `key_required`), `configure_reply_has_no_key`, `memory_fallback_when_no_keyring`, `import_is_idempotent`, `test_provider_errors_scrubbed`.
- Electron: `import_then_verify_then_delete`, `partial_import_keeps_vault`, `import_not_reachable_from_renderer`.

**Steps:** tests first in both languages; implement `SecretStore` with a fake keyring backend for tests; port `ProviderConfigs`; the RPC; the Electron importer; run `cargo test -p loams-agentd-loop -p loams-agentd` and the desktop tests; commit.

**Commit:** `agentd: provider keys and factory credentials in the OS keyring (D794)`.

### Task 23: Factory ops in the daemon

**Files:** new crate `crates/loams-agentd-factory/src/{lib.rs,apps.rs,ops.rs,health.rs}`, `fixtures/requests/*.json`, `tests/{ops.rs,parity.rs}`; `apps/desktop-electron/test/factory-requests-fixture.test.ts` (records the TypeScript adapters' requests); `crates/loams-agentd/src/server.rs`.

**Consumes:** `src/main/factory/{apps,ops,host}.ts`, `test/factory-{ops,host}.test.ts`; Task 22's `SecretStore`.

**Produces:** `FactoryApps` with the D660 op table (op, parameter schema, the HTTP request it makes), `health` (`unconfigured`, `ok`, `auth_failed`, `unreachable`), `test`, a 15 s timeout, `classify_error`, scrubbed errors; RPC `FactoryList`, `FactoryConfigure`, `FactoryTest`, `FactoryRemove`, `FactoryQuery`; the `factory_query` tool (read) listing the ops per app in its description as `builtin-tools.ts` does.

**Tests:** `factory_requests_match_ts_adapters` (each op's method, path, query and header names equal the recorded TypeScript request); `unknown_op_rejected`; `params_validated_per_op`; `health_states`; `credentials_never_in_replies_or_errors`; `openobserve_has_no_ops`.

**Steps:** record the TypeScript adapters' requests with the fixture test; write the Rust parity and op tests; port the op table app by app; wire the RPC and the tool; run `cargo test -p loams-agentd-factory`; commit.

**Commit:** `agentd: the factory vault and read-only ops move to the daemon (D795)`.

### Task 24: The native loop as a harness, with approvals

**Files:** `crates/loams-agentd-loop/src/{loop.rs,budgets.rs,approvals.rs,harness.rs,transcript.rs,prompt.rs}`, `tests/{loop.rs,scenarios.rs}`, `fixtures/scenarios/*.json` (exported from `agent-loop.test.ts`); `crates/loams-agentd-proto/src/agent.rs` (`LoamsAgent`, approval events); `crates/loams-agentd-sessions/src/{registry.rs,sessions.rs,rpc.rs}`; `crates/loams-agentd-store/src/store.rs` (`native_chats`).

**Consumes:** `agent/{loop,service}.ts`, `test/agent-loop.test.ts`; Tasks 18–23.

**Produces:**
- `LoamsAgentHarness: Harness` registered as `HarnessId::LoamsAgent`, with `models()` from the configured providers, `deterministic_turn_end() = true`, steering at iteration boundaries, and `run()` driving `run_turn`;
- `run_turn(transcript, user_text, deps) -> StopReason` with §50 §10.3's rules, emitting `AgentEvent`s and saving the provider-exact transcript to `native_chats`;
- approvals: `ApprovalRequested` before a `write` call without "always"; `ResolveApproval { sessionId, callId, decision }` (owner only); `ApprovalResolved`; the wall clock pauses while waiting; "always" persists on the session;
- `WatchSession { sessionId, afterSeq? }`: a snapshot (nodes, pending approvals and questions, running) then events, with `seq` on each.

**Tests:**
- `loop_scenarios_match_ts`: every scenario exported from `agent-loop.test.ts` (scripted provider events, tool results, approvals) yields the same stop reason, the same stored messages and the same event sequence.
- `budgets_and_clock_pause`, `max_tokens_answers_calls_without_running`, `dangling_tool_use_answered_on_cancel`, `refusal_stops_with_llm_error`, `fallback_drops_non_text_blocks`.
- `approval_roundtrip`, `always_allow_persists`, `deny_answers_denied`.
- `watch_session_snapshot_then_events`, `watch_session_after_seq_resumes`.
- `loams_agent_listed_with_other_harnesses`.

**Steps:** export the loop scenarios from `agent-loop.test.ts`; write the Rust scenario runner and the approval and watch tests (they fail); port the loop; implement the harness, the approval events and `WatchSession`; run `cargo test -p loams-agentd-loop -p loams-agentd-sessions`; commit.

**Commit:** `agentd: the native agent loop as the LoamsAgent harness, with approvals (D791)`.

### Task 25: Durable turns

**Files:** `crates/loams-durable/src/{config.rs,embed.rs}`, `crates/loams-durable/tests/no_listener.rs`; `crates/loams-agentd-loop/src/durable_turn.rs`; `crates/loams-agentd/src/durable.rs`, `tests/durable.rs`; `crates/loams-agentd-sessions/src/{sessions.rs,run_journal.rs}` (harness resume).

**Consumes:** T0-5, T0-6; Task 24.

**Produces:**
- `DurableConfig.serve_http: bool` (default `true`); `DurableServer` binds nothing when it is `false`; `loams`'s behaviour is unchanged;
- the daemon embeds the server on `durable.db` and a runtime in group `loams-agentd`;
- `loams.agentd.turn` with the steps and ids of the Shared contracts and §50 §11.2–§11.3 (model calls checkpointed; write intent before a write; approvals as promises settled only by `ResolveApproval`; budgets carried in step records);
- at start, unfinished turns resume; their docs get an `interrupted` mark on partial text and one "Resumed after a restart" divider;
- harness turns: a durable record of the native session id; on recovery, resume with the steer of §50 §11.5 when the harness supports it (per T0-6) and `resumeOnRestart` is on, else `interrupted` with a Resume action;
- `Drain` completes §50 §6.2 steps 2–4.

**Tests** (crashes are injected by a test hook that aborts the runtime between named points, and by killing a child daemon process in `tests/durable.rs`):
- `durable_api_has_no_listener` (no listening socket owned by the process except the RPC port).
- `completed_steps_not_reissued` (the scripted provider counts calls).
- `interrupted_model_call_reissued_once`.
- `write_tool_runs_at_most_once_across_crash` (crash after intent, before result: the tool's side-effect counter is 1 and the model gets the "may or may not" result).
- `pending_approval_survives_restart`, `approval_cannot_be_settled_without_owner_rpc`.
- `budgets_continue_after_restart`.
- `harness_turn_resumes_with_steer` (mock harness with resume), `harness_without_resume_marked_interrupted`.
- `drain_checkpoints_running_turns`.
- `loams` crate: its durable tests still pass with the default.

**Steps:** write `no_listener.rs` in `loams-durable` and add `serve_http`; run `cargo test -p loams-durable`; commit that alone; then write the crash tests (they fail), implement the turn function and recovery, then harness resume; run `cargo test -p loams-agentd-loop -p loams-agentd`; commit.

**Commit:** `durable: serve_http option`; `agentd: durable agent turns that resume after a crash (D793)`.

### Task 26: Chat import

**Files:** `crates/loams-agentd/src/import.rs`, `tests/import.rs`, `fixtures/chats/*.json`; `apps/desktop-electron/src/main/agentd/import-chats.ts`, `test/agentd-import-chats.test.ts`.

**Consumes:** `agent/store.ts` (`ChatRecord`, `isChatId`); Task 24's `native_chats` and session docs.

**Produces:** `ImportChats { chats: ChatRecord[] } -> { imported: string[], skipped: string[], invalid: string[] }` (owner, not renderer); the mapping of §50 §10.7; Electron's importer (batches of 20, rename to `chats.imported-<timestamp>` only when every file was imported or skipped).

**Tests:** `import_maps_messages_and_always_allow`, `import_idempotent_by_id`, `imported_chat_is_idle`, `thinking_signatures_preserved`; Electron: `invalid_file_left_in_place`, `directory_renamed_after_success`.

**Steps:** write the fixtures and tests first; implement the mapping and the RPC; the Electron importer; run the tests; commit.

**Commit:** `agentd: import Electron chats into daemon sessions (D796)`.

### Task 27: Electron behind `agent.runtime`

**Files:** `apps/desktop-electron/src/main/{agent/ipc.electron.ts,factory/ipc.electron.ts,sql/ipc.electron.ts,index.ts,agentd/runtime-switch.ts}`, tests.

**Consumes:** Tasks 20–26; Task 11's relay.

**Produces:**
- with `agent.runtime = daemon`: the existing `chat:*` IPC handlers call the daemon (`ListProviders`, `ConfigureProvider`, sessions for `list`/`get`/`create`/`send`/`cancel`/`remove`, `ResolveApproval`) and map `WatchSession` events back to today's `ChatEvent`s, so the current `@loams/plugin-agent` panel works unchanged; the `factory:*`, `pg:*` and `wesql:*` handlers call the daemon too;
- on the first switch: Task 22's secret import, then Task 26's chat import;
- with `electron`: today's code paths, untouched;
- a Settings › Agent toggle "Run agents in the background service (beta)"; default `electron` in this task.

**Tests:** `chat_api_unchanged_on_daemon_runtime` (the existing `test/agent-*.test.ts` contract cases run against a fake daemon through the switched handlers), `factory_and_sql_ipc_route_to_daemon`, `first_switch_imports_secrets_then_chats`, `electron_runtime_untouched`.

**Steps:** port the existing `test/agent-*.test.ts` contract cases to run through the switched handlers against a fake daemon (they fail); implement the switch; run the desktop tests and the smoke with `agent.runtime` set both ways; commit.

**Commit:** `desktop: run the agent in the daemon behind agent.runtime`.

---

## DD1e — The UI from dsh-desktop

### Task 28: Study the DSH client UI and write the port map

**Files:** this plan's "Rulings made during execution" (`T28-*`), `docs/design/50-loams-desktop-daemon.md` §14.2 (as found), `apps/desktop-electron/NOTICE`, `web/plugins/agent/NOTICE`.

**Consumes:** dsh-desktop's lockfile; §50 §14.

**Produces:**
- the tarballs of the §2.3 packages at `0.2.0-rc.2`, fetched with `npm pack <name>@0.2.0-rc.2` from registry.npmjs.org into a fresh scratch directory (never the repository, never `/tmp`; untrusted data, not executed), each checked against the lockfile's `integrity`;
- for each package: its `LICENSE` and `package.json` licence, any bundled third-party code and its licence, and its exports;
- the port map (§50 §14.2) confirmed or corrected, naming the source file and symbol of every adapted component, and the behaviours to keep (keyed node rendering, the pending echo, steering rows, retry and failure rows, folded tool lifecycles, approval keyboard shortcuts, trajectory, thinking collapse);
- the NOTICE lines of §50 §15.

**Tests:** none (a study task); the reviewer checks the integrity hashes against the lockfile.

**Steps:** fetch and verify the tarballs; read; write the port map and the licence findings into the rulings; update §50 §14.2 and the NOTICE files; commit.

**Commit:** `docs: DD1 task 28, the DSH client UI port map and notices`.

### Task 29: The session store and the chat components

**Files:** `web/plugins/agent/src/{session/{store.ts,selectors.ts},chat/{Conversation.tsx,Transcript.tsx,NodeSeat.tsx,UserBubble.tsx,PendingEcho.tsx,RetryRow.tsx,FailureRow.tsx,Thinking.tsx,ToolCard.tsx,ApprovalCard.tsx,QuestionCard.tsx,TurnOutline.tsx,SubagentCard.tsx,Composer.tsx},markdown.ts}`, `test/{store.test.ts,chat.test.tsx,markdown.test.ts}`; `web/packages/agentd-client/src/reducer.ts`.

**Consumes:** Task 28's map; Task 9's types; Task 11's preload `agentd`.

**Produces:**
- `SessionStore` per open session from `WatchSession` (snapshot, events, `afterSeq`), with `useNode(key)`, `usePending()`, `useRunning()` selectors (`useSyncExternalStore`), text deltas appended per node;
- the components of §50 §14.2, written against `AgentEvent`, Tailwind utilities with token colours, `@loams/ui` components and lucide icons; adapted files carry the attribution header;
- the composer: send, queue while running, steer, stop;
- approval cards: Once, Always for this session, Deny, with `Enter`/`Shift+Enter`/`Esc` shortcuts when focused;
- Markdown: the existing safe renderer, extended per §50 §14.2.

**Tests:**
- `reducer_handles_every_event_fixture`.
- `delta_rerenders_only_its_node` (render counters).
- `pending_echo_replaced_by_durable_node`.
- `tool_card_folds_lifecycle`, `approval_card_shortcuts`, `question_card_answers`.
- `thinking_collapsed_by_default`.
- Markdown: `no_raw_html`, `no_images_render_alt_only`, `links_http_only`, `code_block_copy`.
- `no_tailwind_palette_or_raw_colours` (a source scan, as the console's Tailwind test).

**Steps:** write the reducer and component tests (they fail); build the store, then the components in the port map's order; run `pnpm --filter @loams/plugin-agent test`, typecheck, Biome and `pnpm --filter @loams/console build`; check the panel in the Electron app as the conventions describe; commit.

**Commit:** `web: agent chat streaming components adapted from the DSH client UI (D797)`.

### Task 30: Sessions, new sessions and reattach

**Files:** `web/plugins/agent/src/{sessions/{SessionList.tsx,NewSession.tsx,HarnessSettings.tsx},index.tsx,panel.tsx}`, tests; `web/plugins/shell/src/icons.ts` (icon name `agent`); `web/apps/console/catalog/desktop.yml`, `web/apps/console/src/cordis/desktop.ts`.

**Consumes:** Task 29; the sessions RPCs (list, create, harness catalog, install, enable).

**Produces:**
- the right dock (`shell.dock.right`) shows the open session; a full page `/agent` (nav entry `agent`, group `Compute`, order 65) shows the session list beside the conversation;
- sessions of every harness with running, waiting-for-approval and interrupted badges; search; New session (harness, model, working directory picker through the desktop service);
- Settings › Agents: harness install and enable (from `ListHarnesses`, `InstallHarness`, `SetHarnessEnabled`), providers (the existing providers section, now backed by the daemon);
- reattach: reopening the window or the page subscribes again and shows everything that happened meanwhile; a session waiting for approval opens at its card.

**Tests:** `list_shows_all_harnesses_with_badges`, `new_session_calls_create_with_choice`, `reattach_shows_turns_finished_while_closed` (fake daemon advances while unsubscribed), `deeplink_opens_session`, `interrupted_session_offers_resume`.

**Steps:** tests first with a fake `desktop.agentd`; implement; register the page in the catalog and the module table; run the plugin tests, the console build and the desktop smoke; commit.

**Commit:** `web: agent sessions list, new sessions and reattach`.

### Task 31: Terminal and diffs

**Files:** `web/plugins/agent/src/{terminal/Terminal.tsx,diffs/{DiffView.tsx,ChangeRequests.tsx}}`, tests; `web/plugins/agent/package.json` (`@xterm/xterm`, `@xterm/addon-fit`).

**Consumes:** the terminals, diff and change-request RPCs kept by Task 0's list; Task 11's relay (terminal output batched every 16 ms).

**Produces:**
- a terminal tab per session (xterm.js, the fit addon, a token-coloured theme, paste confirmation for multi-line input, the session's working directory);
- the diff view of the session's working tree and of the latest turn, with hunks rendered as React text nodes, file list, discard (confirmed);
- the change-request view (status, checks, link opened in the system browser).

**Tests:** `terminal_writes_stream_and_sends_input`, `terminal_disposed_on_unmount`, `diff_renders_text_not_html` (a hunk containing `<img src=x onerror=…>` renders literally), `discard_requires_confirmation`.

**Steps:** pin and add the xterm packages (at least 14 days old); tests first; implement; run the plugin tests and the console build; commit.

**Commit:** `web: agent terminal and diff views`.

### Task 32: Slot contracts and Settings › Plugins

**Files:** `web/packages/slots/src/{contract.ts,index.ts}`, `web/plugins/agent/src/slots.ts`, `web/plugins/desktop-settings/src/plugins.tsx`, `web/apps/console/src/cordis/{desktop.ts,safe-mode.ts}`, `apps/desktop-electron/src/main/{index.ts,shell/menu-model.ts}` (Safe Mode), tests.

**Consumes:** dsh-desktop's `docs/patch-plugin-contract.md` (as a reference); the catalog and module table.

**Produces:**
- `defineSlot({ name, kind: 'single' | 'list', scope: 'app' | 'session', owner?: … })` in `@loams/slots`, and the four agent slots of §50 §14.4, with `agent.tool.renderer` keyed by tool name;
- Settings › Plugins: catalog plugins with tier, version, editions, slots, state, enable/disable (`plugins.disabled`, applied at the next boot), and an error card for a plugin that failed to load (isolated: the others still load);
- Safe Mode: the app menu item "Restart in Safe Mode" and the `--safe-mode` flag boot first-party plugins only; a banner says so and offers "Restart normally".

**Tests:** `slot_kind_single_rejects_second`, `session_scope_slots_reset_on_switch`, `disabled_plugin_not_started`, `failing_plugin_isolated`, `safe_mode_loads_first_party_only`, `tool_renderer_slot_keyed_by_tool`.

**Steps:** tests first; add the contract fields to `@loams/slots` without breaking existing registrations; declare the agent slots; build the Plugins page and Safe Mode; run the slots, plugin and console tests and the smoke; commit.

**Commit:** `web: typed agent slots and the Plugins settings page with Safe Mode`.

---

## DD1f — Switch, remove, review, release

### Task 33: The end-to-end durability gate

**Files:** `apps/desktop-electron/test/e2e/durability.spec.ts`, `crates/loams-agentd/tests/e2e_support.rs` (a scripted fake provider served on loopback), `.github/workflows/desktop-agentd-e2e.yml`.

**Consumes:** Tasks 12, 17, 25, 27, 29, 30.

**Produces:** the test of §50 §17 (steps 1–4), run on Linux in CI on every change to `crates/loams-agentd*`, `crates/loams-durable`, `apps/desktop-electron/src/main/agentd`, `web/plugins/agent`; with `agent.runtime = daemon` and service mode; a written manual checklist for macOS and Windows in `docs/guides/desktop/background-agents.md`.

**Tests:** `turn_continues_after_quit`, `reopen_reattaches_with_pending_approval`, `kill_daemon_resumes_from_checkpoint_write_once`, `service_restart_completes_turn`.

**Steps:** write the fake provider and the spec; make it pass locally on Linux; add the workflow; run it twice in CI to check stability; commit.

**Commit:** `desktop: end-to-end durability gate for background agents (D799)`.

### Task 34: The agent default moves to the daemon

**Files:** `apps/desktop-electron/src/main/settings.ts`, `web/plugins/agent/src/index.tsx` (the new UI is the panel), `web/plugins/approvals` (pending approvals from the daemon), tests.

**Consumes:** Task 33 green.

**Produces:** `agent.runtime` defaults to `daemon`; the right dock and `/agent` use the Task 29–31 components; the tray badge and the Approvals page include agent approvals from `WatchDaemonStatus`.

**Tests:** the e2e gate and the smoke stay green; `approvals_page_lists_agent_approvals`.

**Steps:** flip the defaults; switch the panel to the new components; update the Approvals page; run every suite and the e2e gate; commit.

**Commit:** `desktop: the daemon runs the agent by default`.

### Task 35: Remove the TypeScript path

**Files:** deleted: `apps/desktop-electron/src/main/agent/{loop,service,store,tools,builtin-tools,live-tools,sql-guard}.ts`, `src/main/agent/providers/*`, `src/main/agent-tools/`, `src/main/sql/{caps,pg,wesql,neon,tools}.ts`, `src/main/factory/{host,vault,ops,apps}.ts`, `src/main/engine/{supervisor,binary,ports,log-rotate}.ts`, `src/main/stacks/{stacks,runtime}.ts` and their tests; changed: the IPC files (now thin forwards), `src/main/index.ts`, `src/shared/sql-lex.ts` (kept only if a page still uses it for the write confirm), `package.json` (drop `pg`, `pg-cursor`, `mysql2`, `ajv`, the adapter dev dependencies if unused), `NOTICE`.

**Consumes:** Task 34 released in at least one tagged build (the owner confirms in the PR).

**Produces:** no agent, supervisor, stack, SQL or factory logic in Electron main; the flags `engine.owner` and `agent.runtime` removed (their stored values ignored); Electron main keeps the protocol, views, updater, tray, deep links, settings, the daemon client and the migrations (kept for one more release, then deleted by a ruling).

**Tests:** `no_agent_logic_in_main` (a source scan for `streamTurn`, `runTurn`, `EngineSupervisor`, `safeStorage.decryptString` outside `agentd/import-secrets.ts`); every remaining test, the smoke and the durability gate green.

**Steps:** write `no_agent_logic_in_main` (fails); delete file by file, replacing each IPC handler with a forward; drop unused dependencies; run every suite, the smoke and the e2e gate; commit.

**Commit:** `desktop: remove the TypeScript agent, supervisor and services (D799)`.

### Task 36: Security review and fuzzing

**Files:** `docs/security/agentd-threat-model.md`, `crates/loams-agentd-rpc/fuzz/{frame,upgrade,hello}`, `.github/workflows/ci.yml` (nightly fuzz), findings fixed in their own commits.

**Consumes:** everything; §50 §7, §18.

**Produces:** the threat model of §50 §18 with each threat's mitigation and test; a review of the merged code against it (including `cargo deny`, the npm audit of new packages, the renderer allowlist, every `renderer: true` method, the keyring fallback, the durable listener, notifications and deep links); fuzz targets running nightly; no open high or critical finding.

**Tests:** the fuzz targets; any finding's regression test.

**Steps:** write the threat model from §50 §18; review the code against it; add the fuzz targets and the nightly job; fix findings with regression tests; commit.

**Commit:** `docs: loams-agentd threat model`; `agentd: fixes from the security review` (as needed).

### Task 37: Documentation

**Files:** `docs/guides/desktop/background-agents.md`, `crates/loams-agentd/README.md`, `docs/design/{50-loams-desktop-daemon.md,37-desktop-and-mobile-apps.md,13-decision-log.md,README.md}`, `docs/plans/README.md`, `apps/desktop-electron/README.md`, `CHANGELOG.md`.

**Produces:** the user guide (background mode and consent, linger, notifications, stopping, removing the service and the per-user files on each OS, where logs are, the manual macOS and Windows checklist); §50 updated to the as-built state; D780–D799 statuses; the AP1n plan and §37 §18 marked superseded; release notes.

**Steps:** write the guide and update the documents; check every link; commit.

**Commit:** `docs: Loams Desktop background agents`.

### Task 38: Exit report

**Files:** `docs/plans/dd1-exit-report.md`, `docs/plans/README.md` (status).

**Produces:** every exit criterion below with its evidence (CI run links, test names, the review), the rulings made during execution summarised, and the open questions with their current answers.

**Steps:** collect the evidence for each exit box; write the report; commit.

**Commit:** `docs: DD1 exit report`.

## Self-review

- Every §50 section maps to tasks:

  | §50 | Tasks |
  |---|---|
  | §4 | 1–5 |
  | §5 | 6, 8, 11, 12 |
  | §5.3, §6 | 12, 13, 25 |
  | §7 | 7, 11, 36 |
  | §8 | 9, 10 |
  | §9 | 14–17 |
  | §10 | 18–24, 26, 27 |
  | §11 | 25 |
  | §12 | 19, 22 |
  | §13 | 23 |
  | §14 | 28–32 |
  | §15 | 5, 11, 28, 37 |
  | §16 | 11, 16, 17, 27, 34, 35 |
  | §17 | 33 |
  | §18 | 36 |

- The owner's decisions: 1 → Tasks 18–27, 33; 2 → Tasks 2, 3; 3 → Tasks 1, 4; 4 → Tasks 28–32; 5 → Tasks 8, 12.
- Every task names its tests, and each test states what it asserts.
- The app works at every merge: Tasks 1–10 do not touch the shipping path; Task 11 adds a child process nothing depends on; Tasks 16 and 27 switch behind flags with fallbacks; Tasks 17 and 34 flip defaults only after their e2e tests; Task 35 deletes only after a released build ran the daemon path.
- External dependencies: a systemd user manager in CI for Tasks 8, 17 and 33 (T0-8 decides; the self-supervisor is the fallback); `LOAMS_TEST_PG` for Task 21's live-database cases; the owner's confirmation in Task 35's PR.

## Exit criteria

DD1 is done when every box is checked on a release candidate.

**One agent system**
- [ ] Every agent turn (the native loop, Claude Code, Codex, ACP) runs in `loams-agentd`; Electron main has no agent, supervisor, SQL or factory logic.
- [ ] The native loop matches the TypeScript scenarios; providers match the recorded SSE fixtures; prompt caching and the refusal-fallback opt-in are covered.
- [ ] Electron chats and secrets were imported, verified and the old files removed.

**Durability**
- [ ] The e2e gate passes in CI: a turn continues after quit, reattaches on reopen, resumes after SIGKILL with a write tool run once, and completes after a service restart.
- [ ] The durable store has no listener; approvals settle only through the owner RPC.

**Process model**
- [ ] Service install, uninstall and status work on Linux (CI), macOS and Windows (manual checklist); linger is off by default and opt-in; consent is asked once with no pre-selection; the tray stops background agents.
- [ ] An app update replaces the daemon at the next launch after a drain.

**Security**
- [ ] Token, Origin, Host and role tests pass; the renderer reaches only `renderer: true` methods; scoped run tokens cannot approve.
- [ ] No canary appears in logs, replies, journals, docs or child environments.
- [ ] The threat model is complete and the review closed with no high or critical finding; the fuzz targets run nightly.

**Removals**
- [ ] No edge, WorkOS, Cursor, update or push code remains (`no_edge_symbols`, `no_remote_feature_symbols`).
- [ ] `agentd-deps.sh` passes on every target: no gpui, wry, webkit, javascriptcore, gtk or cpal.
- [ ] `apps/desktop/native` is gone; its notices and provenance are in `crates/loams-agentd`.

**UI and packaging**
- [ ] The chat, sessions, terminal, diffs, approvals and Plugins pages ship, adapted from the DSH client UI with attribution; Markdown has no raw HTML or remote images; no credential reaches the renderer.
- [ ] deb, rpm, pacman, AppImage, Windows NSIS (SignPath, including `loams-agentd.exe`) and macOS dmg/zip carry the daemon; NOTICE lists zeron and the DSH client UI.

## Rulings made during execution

### Task 0 findings (2026-10-09, `dev` at `ca12a14f`)

Checked against the code listed in Task 0. Nothing was built; `cargo metadata`/`cargo tree` were not needed because both lockfiles were compared directly. Crate versions and dates come from the crates.io and npm registries (2026-10-09). The owner gave no answers to Q700–Q714, so §50's defaults stand (T0-9).

| # | Finding (as built) | Resolution | Absorbed by |
|---|---|---|---|
| T0-1 | **RPC surface.** `rpc/src/lib.rs::methods` has 109 names. **Delete, edge (9):** `RelayCommand`, `RetryDelivery` (amended by T2-12: its local re-send stays), `FocusChat`, `ProbeSync`, `SyncStatus`, `WatchConnectivity`, `WatchTransfers`, `FetchToolBlob`, `WatchDevices`. **Delete, WorkOS (10):** `AuthStatus`, `SignIn`, `SignInHeadless`, `CompleteSignIn`, `SignOut`, `ListOrgs`, `CreateOrg`, `SelectOrg`, `LocalImportStatus`, `ImportLocalWorkspace`. **Delete, updates (3):** `UpdateStatus`, `ApplyUpdate`, `SetHarnessUpdatePolicy`. **Replaced (1):** the fork's `StopEngine` stops the fork's own headless runtime (`HeadlessRpc`), so it becomes the new `Shutdown`, and the name `StopEngine` is reused for the supervised `loams dev` (Shared contracts). **No GPUI-only deletions:** the UI-flavoured methods (`WatchSidebarPreferences`, `TakeProjectActionSetup`, `ResolveGitAvatars`, `ReadWorkspaceImage`) work headless and stay. **Keep (86):** `WatchPreviews`, `ListHarnesses`, `CancelInstall`, `InstallHarness`, `GetTitleSettings`, `SetTitleSettings`, `SetHarnessEnabled`, `ListModels`, `ListSkills`, `ListCommands`, `QueueCommand`, `TakeProjectActionSetup`, `ForkSideChat`, `WatchDocMessages`, `WatchQueue`, `QueueMessage`, `UpdateQueuedMessage`, `BeginQueuedMessageEdit`, `RenewQueuedMessageEdit`, `FinishQueuedMessageEdit`, `MoveQueuedMessage`, `RemoveQueuedMessage`, `SendQueuedMessageNow`, `SteerQueuedMessageNow`, `WatchChats`, `WatchSidebarPreferences`, `WatchSessions`, `WatchSpaces`, `Mutate` (without its `renameDevice` op), `LocalDevice`, `EngineInfo` (without `cursor_sdk_version`), `EngineReady`, `ListRepos`, `AddRepo`, `CloneRepo`, `CreateRepo`, `ListBranches`, `ListRefs`, `ListGitHistory`, `SearchGitHistory`, `ResolveGitAvatars`, `FetchAll`, `SwitchRef`, `ListFolders`, `ListDrives`, `SearchFiles`, `ListWorkspaceDirectory`, `SearchWorkspaceFiles`, `ReadWorkspaceImage`, `ReadWorkspaceFile`, `MoveWorkspaceEntry`, `DeleteWorkspaceEntry`, `WriteWorkspaceFile`, `WatchWorkspaceFiles`, `CreateWorktree`, `DeleteWorktree`, `ListProjectActions`, `UpsertProjectAction`, `DeleteProjectAction`, `RunProjectAction`, `OpenTerminal`, `SubscribeTerminal`, `WriteTerminal`, `ResizeTerminal`, `CloseTerminal`, `WatchCheckoutDiffs`, `WatchWorkspaceGitStatus`, `WatchCheckoutChangeRequest`, `GetCheckoutDiff`, `DiscardWorkingTree`, `GetCheckoutFileDiffText`, `ListAgentAccounts`, `ActivateAgentAccount`, `ForgetAgentAccount`, `StartAgentLogin`, `CompleteAgentLogin`, `PollAgentLogin`, `CancelAgentLogin`, `UploadChunk`, `UploadCommit`, `ReadAttachmentChunk`, `WatchHarnessUpdates`, `CheckHarnessUpdates`, `ApplyHarnessUpdate`, `CancelHarnessUpdate`, `DismissHarnessUpdate`. The `targetDeviceId` relay forwarding (`EngineRpc::forward`, `forwardable`, `forward_agent_login`) goes with the edge | This list is Task 9's input. Plan Task 3's "manual `UpdateHarness`" does not exist: the manual method is `ApplyHarnessUpdate` (Ruling 3, no rename). `HarnessUpdatePolicy::AutoWhenIdle` and `Notify` go with the polling; `Off` is the only behaviour left | 2, 3, 9 |
| T0-2 | **Edge and WorkOS reach.** `engine/src/lib.rs` (`EngineConfig.{edge_url,edge_token,org_id,workos_client_id}`, `EngineCore.{auth,links,updater,updater_wake}`, `set_links`, `dial_device`, `start_host_relay`, `disconnect_edge`, `Engine::{build_auth,initial_workspace_scope,resolve_profile,assemble_runtime_inner}` edge branch, `terminal_sign_in`, `wait_for_signed_out`); `auth.rs` (WorkOS, `impl preview::signaling::TokenSource for Auth`); `doc_host.rs` (`EdgeConfig`, `activate_sync`, `spawn_sync_scheduler`, `spawn_chat2_join`/`seed_chat2`/`chat2_maintenance`/`spawn_chat2_checkpoint`, `probe_edge_reachability`, `watch_connectivity`, `nudge_remote_host`, `remote_host_for`, `relay_command`, `retry_delivery`, `ingest_relayed_command`, `deliver_attachments`/`push_attachments`, `upload_tool_sidecar` (`PUT {edge}/blob/…`), `fetch_tool_blob`); `chat2_host.rs`; `workspace_host.rs` (`join_room`, `connect_registry_url`, presence, `peer_liveness`, `WsDerivedRegistryTransport`); `diff_sync.rs` (sidecar upload); `local_import.rs`; `rpc.rs` (forwarding, auth methods); `agent_accounts.rs` (takes `preview::login::CallbackRoutes` for logins started on another device); `rpc/src/device_room.rs` (`HostRelay`, `LinkCache`, `NudgeHandler`, `PeerLiveness`); `preview/src/signaling.rs` (`/preview/{org}/ws`), `peer.rs` (the only `webrtc` user), `login.rs`; `apps/loams-desktop/src/{main,auth_cli,update_cli,daemon}.rs`. **Local behaviour that changes:** (a) `WorkspaceHost` keeps the local registry doc (chats, spaces, sessions, sidebar pins) and loses rooms and presence, so the device list is this device only; (b) a large tool output keeps only its in-doc summary, as the `Local` profile does today (the sidecar was never uploaded without an edge), so "show full output" is gone; (c) queued attachments no longer travel to another host (`pending://` refs resolve locally only); (d) agent sign-ins started from another device are gone; local OAuth callbacks are unchanged | Strip in Task 2 as listed. (b) is accepted for DD1; keeping full outputs in a local blob directory is a possible follow-up, not in DD1 | 2 |
| T0-3 | **"Push" as built.** There is no mobile push (no APNs, FCM or web push anywhere in the fork). "Push" is: (1) the device-room **nudge**: a sender's engine nudges the host device's relay (`doc_host::nudge_remote_host`, `HostRelay` + `NudgeHandler` → `doc_host.enqueue_wakeup`); (2) **command relay**: `RelayCommand`, `RetryDelivery`, and the delivery escort in `doc_host`; (3) **room pushes**: chat2 and registry clients pushing Loro updates (`sync/src/{chat_client,registry,socket,dial,wake,net_path,sync_jobs,budget}.rs`, macOS `NWPathMonitor` through `block2`); (4) **liveness**: `ProbeSync`, `FocusChat`, `PeerLiveness`, `WatchConnectivity`, `WatchTransfers`. Local OS banners lived in `ui/src/notify.rs` (deleted with `ui`) | Task 3 removes (1), (2) and (4); Task 2 removes (3). §50 §5.5's daemon notifications are new code | 2, 3, 12 |
| T0-4 | **Dependency merge.** The headless closure (without `ui`, `update`, `webrtc`, `loro-protocol`, `block2`) has 498 packages: 317 at the root lock's exact version, 71 at another version, and 110 new to the root lock. Same versions: `rusqlite` 0.32.1, `libsqlite3-sys` 0.30.1, `connectrpc`/`buffa` 0.9.x, `reqwest` 0.12.28, `tokio-tungstenite` 0.24.0, `tokio` 1.53.1, `serde_json`, `similar`, `zbus` 5.19.0, `windows-sys` 0.61.2. Compatible bumps that unify on merge: `hyper` 1.11.0→1.11.1, `uuid` 1.24→1.26.1, `thiserror` 2.0.20→2.0.21. A second major stays: `pulldown-cmark` 0.12.2 (root has 0.13.4). New: `loro` 1.13.9, `keyring` 4.2.0 (zbus Secret Service store, Rust crypto, no libdbus), `portable-pty` 0.8.1 (pulls `serial*`/`termios`), `notify` 7.0.0, `ignore`, `nucleo-matcher` 0.3.1, `deser-hjson`, `serde_yaml_ng`, `zip` 7.2, `webbrowser`, `gethostname`. **`links`:** `libsqlite3-sys` (`sqlite3`), `ring`, `zstd-sys` and `defmt` match the root; `wasm-bindgen-shared` 0.2.127 vs 0.2.128 (`links = "wasm_bindgen"`) is semver-compatible and unifies to 0.2.128. No `links` conflict. **`deny.toml`:** every new licence is on the allow list (MIT, Apache-2.0, ISC, Zlib, BSD-3-Clause, CC0-1.0 for `notify`, MPL-2.0 for `nucleo-matcher`, Unlicense OR MIT for `ignore`). `im`, `bitmaps` and `sized-chunks` (through `loro`) declare `MPL-2.0+`, which the allowed `MPL-2.0` should satisfy. 19 macOS- and Windows-only crates (`objc2*`, `*-keyring-store`, `kqueue*`, `fsevent-sys`, `system-configuration*`, `windows-registry`, `winreg`, `serial-windows`, …) are not in the local registry cache; they are MIT and/or Apache-2.0 by their manifests upstream (from memory, unverified). No new git source. **New crates, at least 14 days old:** `ts-rs` **12.0.1** (2026-01-31, MIT), features `serde-compat`, `serde-json-impl`, `uuid-impl` **and `chrono-impl`** (proto carries `DateTime<Utc>`); `jsonschema` **0.58.0** (2026-09-25, MIT, MSRV 1.85) with `default-features = false` (its default resolves remote `$ref`s through `reqwest` 0.13); `notify-rust` **4.18.0** (2026-06-16, MIT OR Apache-2.0; 4.18.1 and 4.18.2 are too new) with `default-features = false, features = ["z"]` (the default `d` links libdbus; `z` reuses the root's zbus 5); `tokio-postgres` **0.7.18** and `mysql_async` **0.37.1** (`minimal-rust`) are already in the root lock (`loams`, `loams-compat`, `loams-sqldb`). **`tokio-postgres-rustls` is dropped:** the SQL tools reach the local stacks on loopback without TLS (`sql/pg.ts` `PG_DEV` 127.0.0.1:55433), as today. npm: `ws` **8.21.3** (2026-08-07; 8.22.0 is too new), `@xterm/xterm` **6.0.0** and `@xterm/addon-fit` **0.11.0** (2025-12-22), all MIT. §50 §14.2's "xterm.js 5" becomes 6 | Task 1 merges dependencies with these versions and lets the lock unify (`cargo update -p <name>` only, never a blanket update). Task 4 runs `cargo deny check licenses` and adds a `[[licenses.clarify]]` only if `MPL-2.0+` fails. `portable-pty` 0.9 (without serial) is optional, not required | 1, 4, 9, 10, 12, 21, 31 |
| T0-5 | **`loams-durable`'s embed.** (a) **A listener-free server is reachable today without code changes, but only by accident:** `DurableServer::start` always calls `listen::probe(config.listen)` and serves `gateways.gateway_http` on `config.listen`, but `gateways.gateway_http.enabled` and `workers.transport_http_poll.enabled` are not in `PROTECTED`, and `Mode::Migrate` already sets both to `false` and starts fine. `serve_http = false` is therefore: set those two keys, skip `probe`, skip the "unauthenticated API" log line, and skip `wait_free` in `stop`. (b) **SDK API** (`resonate-sdk` at `e3606698`): `#[resonate_sdk::function(name = "loams.agentd.turn")] async fn turn(ctx: &Context, …) -> Result<T>`; `DurableRuntime::start_with(&server, node_id, RuntimeOptions { ttl }, |sdk| sdk.register(turn))`; steps `ctx.run(step_fn, args).await` (local, checkpointed), `ctx.rpc::<T>(name, args)`, `ctx.sleep(d)`; a latent promise `ctx.promise::<T>().create()?` → `handle.id().await?`, settled from outside with `sdk.promises.resolve(id, value)` (or `reject`, `cancel`); a run started with `sdk.run(id, func, args).spawn().await?` and found again with `sdk.get::<T>(id)`. (c) **Child ids are minted by the SDK**, `<root>:<n>` then `<root>:<n>.<m>`, in call order (`ids.rs`); a caller cannot name a step's promise, and `:` is reserved in a root id. (d) **The group is fixed:** `inproc::GROUP = "loams"`; the process id is the runtime's `node_id`. (e) **Redelivery:** the SDK heartbeats every `ttl / 2`; when a runtime dies, its task is handed out again after the lease (60 s by default) plus up to `retry_timeout` (30 s by default); a new runtime with the same group and pid picks it up and replays completed steps from their stored results (`tests/inproc.rs` `crash_between_steps_resumes_without_rerunning_step_1`) | **Contract changes.** Resonate ids: function `loams.agentd.turn`, root promise `t<turnId>` (turn ids are UUIDs, no `:`), group **`loams`** (the fixed `inproc::GROUP`), process `agentd`. The logical names `llm:<n>`, `approval:<callId>`, `tool:<callId>:intent` and `tool:<callId>` are no longer promise ids: each step records its logical name in its result, and the store keeps a `turn_steps(turn_id, logical, promise_id)` row so `ResolveApproval` finds the approval promise and settles it in process with `promises.resolve`. Because ids follow call order, the turn function must issue steps in a deterministic order on replay. The daemon uses `RuntimeOptions { ttl: 15 s }` and `retry_timeout = 5 s`, so a restarted daemon resumes a turn within about 20 s rather than 90 s. Ruling 9 (`serve_http`) stands and is implemented as in (a) | 25 |
| T0-6 | **Harness resume as built.** The engine already resumes harness runs after a crash: `SessionsEngine::recover_stale` (at assembly) finds stale journals, recovers the harness session id from the journal (`journal_harness_session`), and, if the run is under 12 h old and fewer than 3 attempts were made (`MAX_AUTO_RESUME`), re-dispatches the last user message under its original id, so the transcript does not duplicate it. Each dispatch injects the remembered session id through `resume_for` (cwd-gated; an empty id is the "do not resume" tombstone). Per driver: **Claude Code** `--resume=<id>`, with the spawn history restored by `Normalizer::for_resume`, and a fresh start when the CLI rejects it; **Codex** app-server `thread/resume {threadId}`, with a fresh start on error; **opencode** reuses the server-side session id after `session_info`, with a fresh start on error; **Pi** `--session <file>` from its session store; **ACP** (Devin, Grok, Hermes, Antigravity) `session/load {sessionId}` whenever a resume id exists, **not gated on `agentCapabilities.loadSession`**, with a fresh session and a notice on error; **Loams Bot** advertises `loadSession: false`, so its load fails and it starts fresh | Task 25 builds harness resume **on the journal and `recover_stale`, not on a Resonate function**: the journal is already the durable record of the native session id. It adds the per-session `resumeOnRestart` gate (default on, Q706) and replaces the re-sent prompt with §50 §11.5's steer text followed by the original message. Where resume is unsupported or exhausted, the session ends `Done { status: Interrupted }` (an existing status) with the Resume action. ACP keeps trying `session/load` without the capability check, because the fallback already handles a refusal | 25 |
| T0-7 | **Electron paths** (`app-paths.ts` → `app.getPath`; `setAppLogsPath` is never called, so `logs` takes Electron's default). Packaged builds take the app name from `productName` "Loams Desktop": **Linux** userData `${XDG_CONFIG_HOME:-~/.config}/Loams Desktop`, logs `<userData>/logs`; **macOS** `~/Library/Application Support/Loams Desktop`, logs `~/Library/Logs/Loams Desktop`; **Windows** `%APPDATA%\Loams Desktop` (**Roaming**), logs `<userData>\logs`. Unpackaged (`electron-vite dev`) runs have no `productName`, so the name is the package's `@loams/desktop` and userData is `~/.config/@loams/desktop` (a nested directory). `LOAMS_DESKTOP_USER_DATA` overrides userData (`index.ts`, used by the e2e tests). **Escaping:** every path contains a space. A systemd `ExecStart` needs the path in double quotes, with `%` written `%%`, `$` written `$$`, and `\` and `"` backslash-escaped (the fork's unit leaves `ExecStart` unquoted and would break). A launchd plist needs only XML text escaping of `&`, `<` and `>` inside `<string>` (the fork's `xml_escape` suffices). A Windows Run value needs the executable quoted and must be `REG_SZ`, so `%` is not expanded | **Windows: the runtime directory moves to `%LOCALAPPDATA%\Loams Desktop\agentd\runtime`**, because binaries under Roaming AppData travel with roaming profiles; config, token, discovery, store and logs stay under userData. `config.toml` gains `[paths] runtime`. Task 8's golden files cover `%`, `$` and a space | 6, 8, 11 |
| T0-8 | **Platform facts flagged (verify).** (1) **SMAppService with an unsigned app:** not checked against a primary source (Apple's documentation is not readable offline here). It does not matter for DD1: the default is the LaunchAgents plist, which needs no signature. It stays (verify) for Q701. (2) **A logon-triggered scheduled task created by a standard user:** not checked against a primary source; widely reported as "Access is denied" through `schtasks /sc onlogon` for non-admins. The Run entry needs no elevation, so Q702's default stands; it stays (verify). (3) **polkit `set-self-linger`:** systemd's shipped policy (`/usr/share/polkit-1/actions/org.freedesktop.login1.policy`, systemd 261.3) gives `org.freedesktop.login1.set-self-linger` `allow_any`/`allow_inactive`/`allow_active` = **yes**, while `set-user-linger` (another user) is `auth_admin_keep`. Older systemd without the self action, and distributions that override it, need admin, which is §50 §5.6's "otherwise the error is shown". (4) **`notify-rust` on macOS from an unbundled binary:** its macOS backend is `mac-notification-sys`, which posts as an existing app's bundle id (`set_application(get_bundle_identifier_or_default(…))`, per its README). The fork's `ui/src/notify.rs` records that an unbundled process has no `NSUserNotificationCenter` (it is nil), so it borrowed the installed app's identity or fell back to `osascript`. Result: banners are attributed to another app and clicks do not route, so macOS waits for a signed bundle, as §50 says. **Windows** toasts need an AppUserModelID; electron-builder's NSIS shortcut registers `appId` `dev.loams.desktop`, so the daemon passes `.app_id("dev.loams.desktop")` (verify in Task 12). (5) **A systemd user manager in CI:** a `container:` job has no systemd as PID 1, so it has no user manager. GitHub-hosted `ubuntu-24.04` runners are VMs with systemd and passwordless sudo, and the runner process has no logind session, so the gate runs `sudo loginctl enable-linger "$USER"` and exports `XDG_RUNTIME_DIR=/run/user/$(id -u)` before `systemctl --user` (verify on the runner); the self-supervisor stays the fallback | Task 33 runs on the `ubuntu-24.04` VM runner, never in a container. Task 12's notifications set the Windows app id. Q701 and Q702 keep their defaults | 8, 12, 33 |
| T0-9 | **Owner answers to Q700–Q714:** none given by 2026-10-09 | §50 §20's defaults stand | — |
| T0-10 | **§50 §2 rows that differ from `dev`.** (a) `loams-desktop-link` and `loams-desktop-brand` are **Apache-2.0** (Loams-authored), not MIT; the other 15 fork packages inherit MIT. (b) The fork's binary is version 0.2.101; the root workspace is 0.0.1, and `loams-agentd` would report 0.0.1 for every desktop release. (c) Electron reserves **4 ports, or 5 with Live** (`reservePorts(wantLive ? 5 : 4)`), not always five. (d) The agent's SQL tools have a **third fence** §50 §10.4 omits: they log in as the SELECT-only `loams_ro` role (Postgres, with re-grants at most every 10 s) or a SELECT-only MySQL user, besides the READ ONLY transaction (`sql/pg.ts`, `sql/wesql.ts`). (e) `EngineProfile::local` already exists (scope `Local`, store `profiles/local`), separate from `development` (`orgs/<org>/…`); a plain fork start today is `Local` because the placeholder WorkOS client id is non-empty. (f) `@loams/slots` already has `kind` with three values, `single`, `list` and **`keyed`**, and an `approval.renderer` keyed slot. (g) dsh-desktop's lock lists **316** `@deepseek-ai/*` packages (304 MIT, 1 `MIT OR CC0-1.0`, 6 MPL-2.0, 5 BSD-3-Clause), all at `0.2.0-rc.2`, including a `dsh-client-ui-sidebar-terminal`. **`node_modules` is not installed**, so the tarballs were not read; nothing was installed | (a) Link stays Apache-2.0; the 42 Loams-authored brand lines move into MIT `loams-agentd-proto` under the Loams Authors' own grant, noted in the NOTICE; Task 5's licence table lists both. (b) `Hello.version` and `resources/bin/VERSION` come from build-time `LOAMS_AGENTD_VERSION` (the Electron app version, set by `fetch-agentd.mjs`), falling back to `CARGO_PKG_VERSION`; `buildSha` comes from `LOAMS_BUILD_SHA`; `loams_agentd_version_runs` expects the fallback. (c) The supervisor port keeps 4 or 5. (d) Task 21 ports the `loams_ro` logins and re-grants: three fences, not two. (e) Task 2 **deletes** `EngineProfile::{development,synced}` and keeps `local` as it is, rather than renaming; tests that use `EngineCore::assemble` move to `local`. (f) Task 32 adds only `scope` and lifecycle notes; `agent.tool.renderer` is kind `keyed` (key: tool name); Task 29 renders approvals through the existing `approval.renderer`. (g) Task 28 fetches the tarballs by lockfile integrity, as planned, and may add the terminal package to the port map | 1, 5, 11, 13, 14, 21, 28, 29, 32 |
| T0-11 | **Types and names.** `HarnessId` is `Copy`, so Task 3's `Unknown(String)` variant would ripple through the engine. The fork's chat id (`chatId`) is the session id. `WatchDocMessages` with `openingTail: true` already streams a snapshot and then deltas (`opening_doc_messages_stream`) | Task 3 adds a **unit variant `HarnessId::Unsupported`** with a hand-written `Deserialize` that maps any unknown wire name (including `cursor`) to it, so `HarnessId` stays `Copy`. New methods take `sessionId`, which equals the fork's `chatId`; the scoped-token check (Task 7) compares it with `chatId` on fork methods. `WatchSession` is built on `opening_doc_messages_stream`, plus pending approvals and running state | 3, 7, 9, 24, 29 |
| T0-12 | **The strip greps would hit kept code.** `chat2` names fields of the Loro doc schema that stored docs carry (`doc/src/{schema,parts,registry,rebuild}.rs`, `proto/src/entities.rs`); `/blob/` appears in GitHub URLs in `harness/src/install.rs` and `process/windows/command.rs` | `no_edge_symbols` matches `chat2_host`, `ChatClient`, `chat_client` and `chat2_live` instead of a bare `chat2`, and checks `/blob/` outside `loams-agentd-harness`. `no_remote_feature_symbols` matches `\bUpdater\b` as a whole word | 2, 3 |
| T0-13 | **Preview is not just discovery.** Local routing is a reverse proxy on fixed port **7331** on both `127.0.0.1` and `[::1]`, routed by `*.localhost` host name, and it reaches local servers through `mux::local`. `proxy::Router` also holds `peers` | Task 2 keeps `proxy.rs` and the local half of `mux.rs` (`Connector`, `BoxIo`, `local`), deletes `peer.rs`, `signaling.rs` and `login.rs` (and `webrtc` with them), and takes `peers` out of the router. Task 36's threat model lists the 7331 listener (IPv4 and IPv6, host-name routed, unauthenticated) beside the RPC port | 2, 36 |
| T0-14 | **Network reach beyond the Global Constraint.** The kept engine calls harness-provider auth and usage endpoints for agent accounts (`auth.openai.com`, `claude.ai`, `auth.x.ai`, `app.devin.ai`, `portal.nousresearch.com`, `server.codeium.com`, `github.com/login/device`), `api.github.com` (avatars, Copilot usage, change requests through `gh`, harness release lookups in `harness_updates.rs`), and git remotes (`CloneRepo`, `FetchAll`). Change-request polling is demand-driven: it runs only while a stream is subscribed | The constraint "No network calls" also allows: agent-account sign-in and usage endpoints of the harness providers, GitHub (API and `gh`) for avatars, change requests and harness releases, and git remotes, each only on a user action or an open subscription. Task 3 confirms that no timer remains (`harness_updates_has_no_timer`); Task 36 lists these destinations | 3, 36 |
| T0-15 | **Existing exceptions and CI gaps.** (a) `PgRevealPassword` and `WesqlRevealPassword` (`renderer: true` in Shared contracts) return the local stacks' passwords to the renderer, an AP1e exception to "credentials never reach the renderer". (b) The Windows and macOS rows of `desktop-electron.yml` are `experimental: true` (`continue-on-error`), so the `cfg(windows)` and `cfg(macos)` daemon code can fail without failing CI. (c) `monorepo.yml`'s `desktop` job, the `apps/desktop/**` path filter and the `loams-desktop` Nx project (`--target-dir target`) build the fork | (a) Kept; it covers only the local stacks' development passwords; Task 36 records it. (b) Task 11 adds a required (not `continue-on-error`) step `cargo build -p loams-agentd` to the Windows and macOS rows; the packaging steps stay experimental. (c) Task 1 deletes the Nx project; Task 4 removes the job and the filter | 1, 4, 11, 36 |

| T0-16 | **Controller ruling (2026-10-09) on T0-2(b).** Full tool outputs are kept locally | The daemon writes a large tool result's full output and diff to its own store, replacing the edge `/blob/` sidecar; `FetchToolBlob` (or its Task 9 successor) reads them from the store. This replaces T0-2(b)'s "summary only" | 2, 9 |

### Task 1 rulings (2026-10-09)

| # | Ruling | Why | Absorbed by |
|---|---|---|---|
| T1-1 | **The `update` crate is deleted now, not in Task 3.** The engine's `Updater` wiring (`EngineCore::{set_updater,updater,set_updater_wake}`, the release poller in `assemble_runtime_inner`, `restart_when_superseded`, the shutdown join) and the `UpdateStatus`/`ApplyUpdate` handlers are removed. Their method constants stay until Task 3. `local_first.rs`'s `local_runtime_checks_public_releases_without_starting_edge_links` became `local_runtime_starts_no_edge_links_and_sends_no_requests`, and two updater assertions were dropped | The crate's tests read `dist/`, which Task 1 deletes; keeping it would mean renaming a `loams-desktop-update` package only to delete it one task later, and `no_crate_named_loams_desktop` forbids the old name | 3 checks `no_remote_feature_symbols` |
| T1-2 | **Two hidden entry points stay** beside `run`, `mcp`, `status` and `version`: `loams bot-acp` (the Loams Bot harness launches the running executable with these arguments; Q708 keeps Loams Bot) and `--noop-browser` (the Antigravity driver points `BROWSER` at it on Windows, and on Unix when `true` is missing). The brief's "no `--noop-browser`" assumed it was GUI-only; it is a harness dependency. `brand::BINARY_NAME` is now `loams-agentd`, so Loams Bot finds the daemon binary. **Fix round 1 (controller ruling):** the hidden `loams` subcommand accepts only `bot-acp`; the link CLI's `login`, `logout`, `bot`, `status` and `mock` (and a bare `loams`) are clap usage errors with exit 2, checked by `removed_subcommands_are_rejected` | Removing them breaks two kept harnesses; the rest of the link CLI is WorkOS-era or binds ports | 3 (Antigravity unchanged), 7 |
| T1-3 | **Removed from the binary:** the headed default, `headless` (now `run`), `login`/`logout` and the WorkOS status lines (`auth_cli.rs`, 4 tests), `sync` (`sync_cli`), `appshot`, `daemon` (`daemon.rs`, 4 tests including the edge installer test; Task 8 reads it from history at `513dd67b:apps/desktop/native/apps/loams-desktop/src/daemon.rs`), `update` (`update_cli.rs`), and the Windows icon resource (`build.rs`, `embed-resource`, `dist/windows`; GPUI loaded it). `#![windows_subsystem = "windows"]` stays so a Run entry opens no console. `status` is rewritten without WorkOS (data dir, lock holder, IPC port). Logs are `<data>/logs/loams-agentd-run.log` | Tests of deleted features | 6, 8 |
| T1-4 | **All of `apps/desktop` is deleted**, including `edge/` and `LOAMS.md` (its facts are §50 §2.2). `tools/monorepo/check.py` now checks `crates/loams-agentd/{LICENSE,NOTICE,THIRD_PARTY_NOTICES.md}` and that `apps/desktop` stays deleted, and no longer requires the `loams-desktop` Nx project. The root `NOTICE`, `tools/monorepo/imports.json`, `README.md`, `apps/README.md` and `tools/README.md` point at `crates/loams-agentd*`. **Fix round 1:** `monorepo.yml`'s `desktop` job, its `apps/desktop/**` filter, the `outputs.desktop` entry, its place in `summary` and the two `workflow_dispatch` inputs are deleted now, not in Task 4: dorny's filter counts deleted files, so the job would have run on this PR and failed on the missing `apps/desktop/native` | The tree is gone | — |
| T1-5 | **Dependencies.** New root workspace keys: the nine `loams-agentd-*` paths, `chrono` (serde), `deser-hjson`, `gethostname`, `ignore`, `libc`, `loro`, `loro-protocol`, `mimalloc` (v2), `notify` 7, `nucleo-matcher`, `portable-pty` 0.8, `pulldown-cmark` 0.12, `rusqlite` 0.32 (bundled), `similar`, `tokio-tungstenite` 0.24 (rustls webpki roots), `zip` 7.2. `reqwest` 0.12 (rustls, json, stream, system-proxy) and `sha2` 0.10 are named in the member manifests, because the root keys are 0.13 and 0.11. `uuid` adds `serde`. The lock gains 170 packages and changes no existing version (the `webrtc` closure is among them until Task 2). `serde_json` inherits the workspace's `preserve_order`, so JSON objects keep field order; one fixture was updated. **Fix round 1:** the root keys `pulldown-cmark` 0.12 and `tokio-tungstenite` 0.24 are dropped (the rest of the workspace is on 0.13 and 0.30, so the fork's versions should not be the workspace default); the members name them, like `reqwest` and `sha2`. `buffa`, `connectrpc`, `url`, `http`, `toml` and `bytes` now use the existing workspace keys. `base64` stays at 0.22 in the members (the workspace key is 0.23). The lock does not change | One lock, one dependency set (D783) | 2, 4 |
| T1-6 | **Licences.** Every moved crate sets `license = "MIT"` and `publish = false` explicitly (the root default is Apache-2.0); `loams-agentd-link` keeps `Apache-2.0`. The brand strings are `loams-agentd-proto::brand` (T0-10a) | §50 §4.1 | 5 |
| T1-7 | **Bugs the fork shipped, fixed so its suites pass** (the fork's CI never ran the engine, harness or store suites): (a) `worktree_branch_from_title` produced `loams_desktop/…` while every other site, `titles.rs` included, uses `loams-desktop/…` (a rename slip from `zeron/`), so a titled worktree's branch was never renamed; (b) the opencode MCP fixture read `config.mcp.loams-desktop`, which is invalid JavaScript; (c) the Pi MCP fixture expected tool names `loams_desktop_*` where the extension registers `loams-desktop_*`; (d) the Cursor shim fixture matched sorted JSON keys (T1-5) | Found by running the moved suites | — |
| T1-8 | **Six `harness_updates` tests are `#[ignore]`d with a reason**: `command_update_check_offers_the_newer_release`, `shutdown_does_not_wait_for_a_slow_periodic_check`, `hermes_commit_updates_can_install_without_changing_the_cli_version`, `notify_cancels_waiting_automatic_but_preserves_explicit_updates`, `cancelling_versionless_available_update_preserves_its_notice`, `cancelling_a_busy_host_does_not_touch_another_device_or_its_installation`. They already failed in the fork: its import made `HarnessUpdatePolicy::Off` the default, so checks stay `Dormant` | Task 3 removes the policies and polling (T0-1) and rewrites these tests | 3 |
| T1-9 | **`[profile.dev.package.loams-agentd-sessions] opt-level = 1`** in the root `Cargo.toml` | Unoptimized, `EngineRpc::handle` overflows a tokio worker's 2 MiB stack (`checkout_file_diff_text_rpc_fits_the_default_worker_stack` aborted). The fork built dependencies at opt-level 2; optimizing only this crate is enough and rebuilds nothing else | — |
| T1-10 | **Test environment.** `loams-agentd-store`'s `registry_client` and `transport_reliability` tests need `--features mock-server` (the fork enabled it through the engine's dev-dependency in workspace builds). **Fix round 1:** a self dev-dependency (`loams-agentd-store = { path = ".", features = ["mock-server"] }`) turns it on for the crate's own tests, so plain `cargo test -p loams-agentd-store` works. Terminal and project-action tests spawn `$SHELL -l` in a PTY; on a machine whose interactive zsh profile waits for input they hang (`project_action_commands`, `worktree_on_run`, three `device_routing` relay terminal tests), so local runs use `SHELL=/bin/bash`, as CI's runners do. **Fix round 1:** `Terminals::set_shell(Option<TerminalShell>)`, set from the new `EngineConfig::terminal_shell`, replaces `$SHELL` for every terminal and project action that names no shell; the engine-level test helpers (`device_routing`, `m5_repos_diffs_terminals`, `worktree_on_run`, `project_action_commands`) use `TerminalShell::isolated("/bin/sh", <temp home>)`, which sets `HOME` and `ZDOTDIR`, so the suites no longer depend on the developer's shell (no `set_var`, no `#[ignore]`). `.config/nextest.toml` terminates a `loams-agentd*` test after 3 × 60 s, and `rust-tests` has `timeout-minutes: 90`. The harness's node and python3 fixtures are recorded in `ci.yml`, which sets up Node 22 for `rust-tests` | The fork's test assumptions | 4 (no `--features` needed for the store) |
| T1-11 | **Citations.** The link crate's citations of decisions 465 to 468 (Software Factory decisions from design 39, never declared in the log; the log's 465 and 466 rows are unrelated ES decisions) now cite design 37 section 18. `scripts/docs/check-decision-ids.sh` reports nothing under `crates/loams-agentd*`. **Fix round 1:** the last one (`link/src/a2a/mod.rs`) is rewritten too, and this plan names the numbers without the `D` prefix so the checker does not flag its own text | Brief | — |
| T1-12 | **CI exposure.** The moved crates now fall under `ci.yml`'s `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -D warnings` and `cargo nextest run --workspace`. They are formatted. `cargo clippy --fix` applied the machine-applicable lints (about 35 sites); the rest (`collapsible_if` on let-chains, `large_enum_variant`, `too_many_arguments`, `type_complexity` and a few more, mostly in code Tasks 2 and 3 delete) are allowed by a commented `#![allow(...)]` list in `loams-agentd-{doc,harness,preview,sessions,store}` and five integration-test roots. Two `from_mode(0)` literals became `0o0` (a deny-level lint). Clippy on all ten crates with `--all-targets` is clean | One CI (D783) | 4 (drops the allow lists once the code is gone or fixed) |
| T1-13 | **Controller ruling (fix round 1): the workspace lint policy applies now.** Every `loams-agentd*` crate sets `rust-version.workspace = true`. `proto`, `doc`, `rpc`, `mcp` and `link` take `[lints] workspace = true`. `loams-agentd`, `-harness`, `-preview`, `-sessions` and `-store` contain FFI and process code, so they carry their own `[lints.rust]`/`[lints.clippy]` tables: the workspace policy with `unsafe_code = "deny"` instead of `forbid` (Cargo cannot combine `workspace = true` with an override, so the other entries are repeated and must be kept in step). Each of the 132 unsafe sites has `#[allow(unsafe_code)]` on its statement or item, or on the enclosing `fn` where the block sits inside an expression, plus a `// SAFETY:` comment; the Windows and macOS sites were annotated by reading them, because they do not compile on Linux. `missing_debug_implementations` and `clippy::unwrap_used` are fixed in the five small crates (Debug derives or `finish_non_exhaustive` impls; `expect` with the invariant), except `rpc/src/device_room.rs`, which Task 2 deletes. In `harness` and `preview` both lints join the commented T1-12 allow lists; in `sessions` and `store` only `missing_debug_implementations` does (about 70 types and 50 unwraps in all). Integration tests and examples take `#![allow(clippy::unwrap_used)]`, as other workspace crates' tests do | Workspace policy (D783); the clippy fallout in the four large crates is too big for this round | 4 (removes the remaining allow-list entries) |

### Task 2 rulings (2026-10-09)

| # | Ruling | Why | Absorbed by |
|---|---|---|---|
| T2-1 | **WorkOS goes with the edge, in Task 2.** Deleted: `sessions/src/{auth.rs,local_import.rs}` and `tests/{auth.rs,local_import.rs}`, `EngineConfig.workos_client_id`, `Engine::{build_auth,initial_workspace_scope,resolve_profile}`, `terminal_sign_in`, `wait_for_signed_out`, the `AuthRpc` surface, the ten WorkOS methods of T0-1 (`AuthStatus` … `ImportLocalWorkspace`) and the `TokenSource`/`TokenError` seam. `Engine::run` opens the local profile and builds no `Auth`. Task 3 keeps the Cursor shim, the harness-update polling and policies, the `UpdateStatus`/`ApplyUpdate`/`SetHarnessUpdatePolicy` constants, `HarnessId::Unsupported` and `no_remote_feature_symbols` | Every WorkOS call (code exchange, refresh, org list, create and select) was an edge route (`{edge}/auth/*`), so it cannot work without the edge; T0-2 lists `auth.rs`, `local_import.rs` and the sign-in loop for Task 2, and T0-10(e) deletes the synced profile in Task 2. `no_edge_symbols` could not pass with `AuthConfig.edge_url` in place | 3 (less to delete) |
| T2-2 | **The local profile only.** `EngineProfile::{development,synced}`, `account_scoped` and the legacy uploads claim are deleted (T0-10e). `EngineCore::assemble(data_dir, registry, harness)` opens `EngineProfile::local` (store `profiles/local`); `assemble_with_identity` is gone; `assemble_with_profile{,_locked}` and `Engine::assemble_runtime{,_with_lock}` lose the edge and `Auth` arguments. Tests that read `orgs/dev-org/dev-user` now read `profiles/local`. `WorkspaceScope::{Development,Synced}` stay in the wire enum; the engine reports `Local` only | Brief: "the local profile only" | 9 (wire enum) |
| T2-3 | **RPC surface.** Removed from `methods` and `EngineRpc`: the edge methods of T0-1 (`RelayCommand`, `FocusChat`, `ProbeSync`, `SyncStatus`, `WatchConnectivity`, `WatchTransfers`, `WatchDevices`) and the ten WorkOS methods (T2-1). `RetryDelivery` stays (T2-12). `FetchToolBlob` stays and reads the local store (T0-16, T2-4). `targetDeviceId` forwarding (`forward`, `forwardable`, `forward_deadline`, `forward_agent_login`), `QueueCommand`'s `transfers` and `StartAgentLogin`'s `requesterDeviceId` are gone. New test `removed_edge_and_sign_in_methods_are_unknown` (`tests/local_first.rs`). The MCP shim's `list_devices` tool (it read `WatchDevices`) is removed; its `device` arguments accept only this device's id, and `list_projects` drops `deviceName` | T0-1; the device list is this device (T0-2a) | 9 |
| T2-4 | **T0-16 as built.** `sessions/src/tool_outputs.rs` keeps each tool result's full output and diff (`loams_agentd_doc::sidecar_payload`) and each frozen subagent transcript at `<store>/tool-outputs/<chatId>/<percent-encoded partId>[.diff]`, written off the run's hot path; `FetchToolBlob` reads `{chatId}/{partId}[.diff]`; `purge_chat` deletes the chat's outputs. Refs naming `.` or `..`, or outside the chat and part alphabets, are refused. The doc rows do not change: the fork parked ref stamping on 2026-08-10, and `apply_sidecar_refs`/`outputBytes` are left for the UI that offers "show full output" | T0-16; `local_session_roundtrip` requires the same doc rows | 9, 29 |
| T2-5 | **The doc host keeps the fork's edge-less path exactly.** The lineage logic (`room_gen`, the epoch-2 thin doc, `ChatPersistence`, the deferred adoption of an older stored doc) runs as the fork ran it with `edge: None`; `chat2_host.rs` is deleted and `CHAT2_DOC_EPOCH` moves to `chat_persistence.rs`. Deleted: the sync scheduler (replaced by a 100 ms LRU eviction tick, its only local duty), the chat2 join, seed, checkpoint and maintenance, the migration sweep, the cutover watcher, connectivity and transfer watches, the nudge, relay and delivery escort, the attachment push, `retry_delivery`'s redial, nudge and escort (its re-send stays, T2-12), `ingest_relayed_command`, the transcript salvage (it recovered only `.pre-chat2` rollback copies, which the edge adopt path created) and its sync-job retry queue. `ChatPersistence` loses the server-row cursor API (`applied`, `reset_cursor`, `cursor`, `is_clean`); the stored cursor is preserved on save. The queued-attachment gate for local `pending://` bytes stays | Constant-folding `edge = None` keeps local behaviour and the doc rows unchanged | — |
| T2-6 | **The workspace host is local.** No registry room, presence, dial gate or relay probe; `WorkspaceHostConfig` loses `edge`. `publish` passes `registry_synced = false`, as the fork's local profile did, so sidebar pins are never reconciled against a server and `SidebarPreferencesState.synced` stays `false`; the "Pins are still syncing" guard (edge only) is gone. The spaces orphan sweep runs, as it did in the fork's local profile (`registry_synced()` was `true` without an edge). `DiffSidecar` and its `POST {edge}/diff/{chatId}` are gone; checkout diffs are the local `WatchCheckoutDiffs` stream | T0-2(a) | — |
| T2-7 | **Store.** Deleted: `chat_client*`, `registry*` (with the mock server), `socket*`, `dial.rs`, `wake.rs`, `net_path.rs`, `sync_jobs.rs`, `budget.rs`, `chat_frames.rs`, the `mock-server` feature and self dev-dependency (T1-10 no longer applies), `tests/*` and the `chat2_live`/`transport_live` examples. **Deviation from the brief:** `types.rs` is deleted too; it held only the room clients' `SyncError`, `UrlProvider`, `StaticUrl` and `RoomStatsSnapshot`, and nothing references them. `store.rs` is unchanged: the outbox and sync-job tables stay for existing databases, and outbox rows an older build left are still imported on open. The root `loro-protocol` key is dropped (no user) | Brief, plus dead code | 4 |
| T2-8 | **Preview.** `signaling.rs`, `peer.rs` and `login.rs` are deleted (`webrtc` leaves the lock); `Router` has no `peers`; the catalog has no remote services or peer labels; `Connector` loses `connect_from` and `PeerScoped`; `PreviewService::start(projects)` takes no signaling config and has no login tunnels. `tests/{coordinator,leak}.rs` and the `peer-probe` example are deleted; `tests/{discovery,proxy,transport}.rs` stay (with `preview_watch_follows_the_session_checkout` in sessions, they are `preview_local_routing_still_works`). `tokio-tungstenite` and `reqwest` become dev-dependencies | T0-13 | 36 (threat model: the 7331 listener) |
| T2-9 | **Agent accounts.** The `requester` plumbing for sign-ins started on another device is removed (`start_login_for`, `with_callback_routes`, `tunnel_port_allowed`, the callback routes); local loopback callbacks are unchanged | T0-2(d) | — |
| T2-10 | **Deleted edge tests and examples.** Sessions: `tests/{device_routing,relay_delivery,session_publication,sync_resources,transcript_salvage}.rs`, `src/doc_host_sync_tests.rs`, the `EngineChatSink` replay tests in `rpc.rs`, the two-engine cases of `workspace_sync.rs` (it now covers one engine), the synced and edge cases of `local_first.rs` (rewritten: four local tests) and `local_profiles.rs` (two kept), and the `sidecar_probe` example (`e2e::retry_reissues_a_swallowed_send` was restored by T2-12). RPC: `tests/device_room.rs` and the two-device `e2e_driver` example. `previews.rs` now drives local services | They test removed behaviour | — |
| T2-11 | **`no_edge_symbols`** is `crates/loams-agentd/tests/no_edge_symbols.rs`. It scans `.rs` files and `Cargo.toml` under `crates/loams-agentd*` (skipping `tests/fixtures` and itself) for the T0-12 patterns: `edge_url`, `EdgeConfig`, `device_room`, `chat2_host`, `ChatClient`, `chat_client`, `chat2_live`, `HostRelay`, `LinkCache`, `/blob/` (outside `loams-agentd-harness`) and `edge.loams.invalid`. `local_session_roundtrip` is the fork's mock-run suite (`tests/e2e.rs`), kept green with the new data path | Brief | — |

### Task 2 fix round 1 (2026-10-09)

| # | Ruling | Why | Absorbed by |
|---|---|---|---|
| T2-12 | **`RetryDelivery` stays, local re-send only** (controller ruling, amends T0-1, T2-3, T2-5 and T2-10). `DocHost::retry_delivery` is restored from `41005d39` without the chat2 redial, the host nudge and the delivery escort: every Run/Steer attempt whose user message never landed and that was Rejected, Expired, or consumed by the ledger with no result (and is not executing) is re-sent under a new id, one per message (the latest attempt), skipping a message that still has a live pending attempt; a consumed original is resolved Rejected; then a drain pass runs. `methods::RETRY_DELIVERY` (`{chatId}` → `{}`) is back in `EngineRpc`, and `e2e::retry_reissues_a_swallowed_send` is restored. `removed_edge_and_sign_in_methods_are_unknown` now also checks that `RetryDelivery` is served | The failed-send affordance recovers a crash between mark and resolve, which happens without the edge too | 9 (keeps `RetryDelivery` in the method table) |
| T2-13 | **The tool-output store is bounded** (controller ruling, amends T2-4). `ToolOutputLimits` (`EngineConfig::tool_outputs`, set on the doc host at assembly): **per file** 4 MiB of output, cut at a UTF-8 boundary, the file's header byte flagging it truncated; **per profile** 1 GiB, and a write that goes over evicts the oldest files (by mtime) down to 90%; **archived chats** lose outputs older than 30 days, swept hourly on the blocking pool (`DocHost::sweep_tool_outputs`, from the workspace's `archived` rows). `FetchToolBlob` is paged: `{blobRef, offset?, len?}` → `{text, offset, len, total, truncated, eof}`, at most 256 KiB per page, never splitting a character. The writes stay; **Task 9 or 29 adds the reader** (stamping refs into the doc and the "show full output" UI, which pages until `eof`). Task 6's `config.toml` should expose the budget and retention | Unbounded files and one-`String` reads could exhaust disk and memory | 6, 9, 29 |
| T2-14 | **Minor review fixes.** (M1) On Unix the tool-output root and chat directories are 0700 (tightened if an older build made them wider) and the files 0600. (M2) A part id ending in `.diff` is refused, so `{chat}/{part}.diff` always names `part`'s diff. (M3) `ToolOutputs::purge_chat` tombstones the chat for the process's life, so a write still queued from the chat's last run is dropped instead of recreating its directory. (M4) `no_edge_symbols` also fails on `webrtc` in a manifest, `RelayCommand` and `WatchDevices` (allowed only in the test that checks they are unknown), WorkOS in code (a string-aware `//` strip leaves comments free to say what was removed), and a `connect_async`/`client_async` without a loopback marker in the code of its line or the eight before, outside `agent_accounts*`, `harness_updates`, `repos`, `source_control` and `loams-agentd-link`; `loams_agentd_rpc::connect_ws` now refuses any URL but `ws://` to `localhost`, `127.0.0.0/8` or `[::1]`, so its callers need no marker. `proto`'s `development_and_local_never_use_the_workos_gate` test is renamed `…_sign_in_gate`. (M5) An eviction pass checks the budget (handle map lock only) before it takes `opening`; the 100 ms tick and the chat task's post-quiesce pass run an over-budget pass on the blocking pool (inline only without a runtime), because its final snapshot saves are blocking I/O; `open` still evicts inline, so an open never leaves the warm set over its cap; the comment over the handle's publication names the change subscription. (M6) `proto::view::gate_phase` still maps a missing scope to the synced sign-in gate (`missing_scope_falls_back_to_a_synced_gate`); **Task 9 makes the default `Local`** when it trims `WorkspaceScope`. (M7) Noted, no action | Task 2 review | 9 (M6) |
| T2-15 | **Interim exposure until Task 7.** The RPC port is served on loopback without authentication (any local process, and any local user on a shared machine, can call every method); the browser `Origin` refusal is the only check. The preview listener on port 7331 (T0-13: `127.0.0.1` and `[::1]`, routed by `*.localhost` host name, unauthenticated) stays as it was. Task 7 adds the token and roles; Task 36's threat model lists both listeners | Task 2 review | 7, 36 |

### Task 3 rulings (2026-10-10)

| # | Ruling | Why | Absorbed by |
|---|---|---|---|
| T3-1 | **Already gone before Task 3.** `sessions/src/{auth.rs,local_import.rs}`, `EngineConfig.workos_client_id`, the ten WorkOS methods and the synced profile (T2-1, T2-2); `loams-agentd/src/{auth_cli.rs,update_cli.rs}` (T1-3); the `update` crate and the engine's `Updater` wiring (T1-1). **Task 3 removed the WorkOS remnants:** `proto`'s `AuthState` and `UserProfile`, and the sign-in boot gate in `proto::view` (`GatePhase`, `gate_phase`, `ConnectionStatus`, `parse_auth_state`, three tests), which only the deleted GPUI shell used. With no sign-in gate left, T2-14's M6 (`missing_scope_falls_back_to_a_synced_gate`) is gone too; Task 9 still trims `WorkspaceScope`. The link CLI keeps only `bot-acp` (its `status`, `login`, `logout`, `bot` and `mock` verbs were unreachable since T1-2); the link crate's Authentik `auth`, `client` and `mock` modules stay (Loams' own sign-in, not WorkOS, Q703) | Brief: "no WorkOS client id, no `Auth`" | 9 |
| T3-2 | **Cursor.** Deleted: `harness/src/cursor/` (the `@cursor/sdk` shim), `tests/{cursor,cursor_shim}.rs`, five Cursor fixtures, three Cursor examples, `sessions/tests/cursor_live.rs`, the registry slot, the startup transcript bridge (`cursor_unstarted_history`), Cursor's agent account (SDK credential detection, swap, login through the shim, the `api2.cursor.sh` usage probe and `NoCredentials::KeyExpired`), the npm-shim installer it alone used (`installed_shim`, `ensure_installed_shim`, `materialize_shim` and its stress test), `EngineInfo.cursor_sdk_version`, and the `cursorSdkVersion`/`cursorSdkEngineVersion` fields of device rows (rows an older build wrote still read). **`HarnessId::Unsupported`** is a unit variant with `#[serde(other)]` rather than a hand-written `Deserialize` (T0-11): the derive maps any unknown name, `"cursor"` included, to it, as a value and as a map key, and it serializes as `"unsupported"`. **Read-only** is enforced in `DocHost`: `queue_command` (Run, including a Run that picks the harness, and Steer), `queue_message`, the drain's `execute` (a command an older build left in the doc resolves `Rejected`) and `dispatch_with_source_context` (crash recovery and steer fallbacks) answer `EngineError::HarnessUnsupported`, whose text is `harness_unsupported: This agent is no longer supported` (`proto::{HARNESS_UNSUPPORTED, UNSUPPORTED_HARNESS_NOTICE}`). The MCP shim's `list_models` and `create_chat` refuse a harness name that reads as `Unsupported` (`unknown harness`), as they refused any unknown name before. A chat row whose config names an unknown harness now keeps its config (harness `Unsupported`) instead of losing it (`registry::tests::future_harness_chat_rows_stay_visible_as_unsupported`). Tests that used Cursor as "some other harness" use Pi, Devin, Grok or `Unsupported`; `catalog`'s subprocess stress test drove the Cursor shim and is deleted (the cache keeps its other tests) | T0-11; the UI that renders the notice is Task 29 | 29 |
| T3-3 | **Harness updates on request only.** `HarnessUpdateCoordinator::start` (the six-hour poller), `HarnessUpdatePolicy` (`Notify`, `AutoWhenIdle`, `Off`), `HarnessUpdateStatus.policy`, the stored `policies` preference (an old file's key is ignored), `set_policy`, `SetHarnessUpdatePolicy` and automatic installs are gone. Every agent starts `Dormant`; `CheckHarnessUpdates` probes, `ApplyHarnessUpdate` installs (the manual method T0-1 named, not "`UpdateHarness`"), `CancelHarnessUpdate` and `DismissHarnessUpdate` stay. `refresh_enabled` (after `SetHarnessEnabled`) now only cancels a disabled agent's update and settles its row; it no longer probes. Every task the coordinator starts goes through one `spawn` helper with a counter, the hook of `harness_updates_has_no_timer` (it assembles a full `EngineCore`, then calls `refresh_enabled`, and expects zero spawns). The six T1-8 tests: `command_update_check_offers_the_newer_release`, `cancelling_versionless_available_update_preserves_its_notice` and `cancelling_a_busy_host_does_not_touch_another_device_or_its_installation` pass unchanged except that a restarted coordinator is `Dormant`, not `Checking`; `shutdown_does_not_wait_for_a_slow_periodic_check` became `…_slow_check` (a client's check); `hermes_commit_updates_can_install_without_changing_the_cli_version` applies explicitly; `notify_cancels_waiting_automatic_but_preserves_explicit_updates` became `an_explicit_update_waits_for_the_running_agent_then_installs`. Of the other policy tests, `automatic_install_boundary_rechecks_policy_for_commands_and_downloads` and `enabling_auto_updates_installs_release_discovered_by_its_check` are deleted, `disabling_an_agent_cancels_its_waiting_update` uses an explicit update, `retrying_one_failed_check_reports_the_release` installs nothing, and `unknown_claude_channel_clears_stale_release` loses its auto-install assertion | T0-1, T0-14 | 9 |
| T3-4 | **Self-update and push.** `methods::{UPDATE_STATUS, APPLY_UPDATE, SET_HARNESS_UPDATE_POLICY}` are deleted (their handlers went in T1-1); `removed_edge_and_sign_in_methods_are_unknown` also checks `UpdateStatus`, `ApplyUpdate` and `SetHarnessUpdatePolicy`. The push liveness and transfer types in `proto` (`TransferProgress`, `Connectivity`, `ConnectivityState`, `ChatSyncState`, `ChatConnectivity`) had no user left and are deleted. `RetryDelivery` keeps its local re-send (T2-12) | T0-3 | 9 |
| T3-5 | **`no_remote_feature_symbols`** (`crates/loams-agentd/tests/no_remote_feature_symbols.rs`) scans `.rs`, `.mjs`, `.js`, `.ts`, `.py`, `.sh` and `Cargo.toml` under `crates/loams-agentd*`, fixtures included, for `workos`, `WORKOS`, `cursor_sdk`, `@cursor/sdk`, `CursorHarness`, `HarnessId::Cursor`, `ApplyUpdate`, `SetHarnessUpdatePolicy`, `HarnessUpdatePolicy`, `Nudge`, `RelayCommand`, `FocusChat`, `ProbeSync`, `WatchConnectivity`, `WatchTransfers`, and the whole words `Updater` and `UpdateStatus`; matching is case-sensitive, so comments may still say "WorkOS". It shares the walk with `no_edge_symbols` (`tests/source_scan/mod.rs`), and both skip the two strip tests | Brief, T0-12 | 4 |
| T3-6 | **§50 §4.2 follows the rulings:** a stored `cursor` reads as `Unsupported` (read-only) rather than being rejected, `RETRY_DELIVERY` keeps its local re-send, and the manual check and apply of harness updates stay | Design text matched the code | — |
| T3-7 | **The flaky `message_queue` tests (ledger, Task 2 fix round 1) had two causes.** (1) **Queue mutations raced in the doc.** Every `SessionDoc` queue write is a read-modify-write of the shared `LoroDoc` (a push reads the length then inserts; a take finds an index then deletes at it; a new row is created then filled field by field). Loro makes each op atomic, not the sequence, and the composer, the drain and the queue RPCs run on different threads, so a push raced a take into `OutOfBound` (the reported `concurrent_drains_release_one_message` failure) or `ContainerDeleted`, and a take could delete the neighbour of the row it found. `SessionDoc::queue_writes` now serializes every queue mutation from its first read to its commit; reads stay lock-free. `loams-agentd-doc/tests/queue_concurrency.rs` (one pusher, one taker, one exporter) fails 3 of 3 runs without the lock and passes 3 of 3 with it. (2) **A test-harness race.** The fake harness in `message_queue.rs` published the prompt before subscribing to `finish`, and one test waited for the transcript entry (written before the run starts), so `finish.send` could find no receiver (`SendError`, or a turn that never ends). The harness now subscribes first, and the test waits for the agent to receive the prompt. Six copies of the `message_queue` binary in parallel: 6 failed runs of 60 before, 0 of 120 after | Asked in the Task 3 dispatch | — |

### Task 4 rulings (2026-10-10)

| # | Ruling | Why | Absorbed by |
|---|---|---|---|
| T4-1 | **`scripts/ci/agentd-deps.sh` is stricter than the §50 §4.3 sketch.** It uses the sketch's pattern and the same `cargo tree -p loams-agentd -e normal,build --target all` call, adding `--locked`. It saves the tree first, so a failed `cargo tree` fails the step: the sketch's `grep … && exit 1 \|\| exit 0` passes on empty output. Its exit codes are 0 (headless), 1 (GUI crates; each `name vX` is listed, then its `cargo tree -i name@X` path) and 2 (no tree to check, or a `grep` error). `--filter` reads `{p}` lines from stdin, and the self-test uses it. `agentd-deps.test.sh` checks that `wry v0.50.0` gives exit 1 and a clean list gives exit 0, as the brief asks. It also checks one package from each family the pattern names (`gpui`, `zed-*`, `wry`, `webkit2gtk*`, `javascriptcore*`, `soup*`, `gtk*`, `gdk*`, `cpal`, `alsa*`), look-alikes that must pass (`salsa`, `zeroize`, `cpal-free`, `webpki-roots`), and that empty input gives exit 2. **PR #393 review:** the pattern also rejects `egui*`, `eframe` and `iced*` (other Rust GUI toolkits D782 did not list), with self-test cases, and the job's checkout sets `persist-credentials: false`. §50 §4.3 has a note on this | A guard that passes when `cargo tree` breaks guards nothing | — |
| T4-2 | **The `ci.yml` `agentd` job.** Its filter covers the brief's paths (`crates/loams-agentd*/**`, `Cargo.lock`, `scripts/ci/agentd-*`), plus the shared `*toolchain` anchor (root `Cargo.toml` with the workspace lints and dependency keys, `rust-toolchain*`, `.cargo/**`, workflows) and `.config/nextest.toml`. Steps: the self-test, then the guard, then `cargo clippy -p 'loams-agentd*' --all-targets --locked -- -D warnings`, then `cargo test -p <crate> --locked` for each `loams-agentd*` package named by `cargo metadata`, one at a time with no fail-fast. Each crate therefore builds with only its own features; the workspace `check` and `rust-tests` jobs still cover the crates with unified features. It sets up Node 22 for the harness fixtures, needs no protoc (no agentd crate has a build script), has `timeout-minutes: 60`, and is in `required`. **Repository CI is disabled (owner, 2026-10-10)**, so "green on the PR" was checked by running the same commands locally (the test step was extracted from `ci.yml` and run as-is) | Brief; CI disabled | — |
| T4-3 | **`deny.toml` needs no new allowance.** `cargo deny check` (advisories, bans, licences and sources) passes on the merged tree. MIT (the zeron crates), Apache-2.0 (`loams-agentd-link`), CC0-1.0 (`notify`), MPL-2.0 (`nucleo-matcher`) and MPL-2.0+ (`im`, `bitmaps` and `sized-chunks`, through `loro`) are all on the allow list. No `[[licenses.clarify]]` is needed (T0-4). A comment records this | T0-4 | — |
| T4-4 | **The T1-12/T1-13 allow lists are gone.** The crate-level `#![allow(...)]` lists in `loams-agentd-{doc,harness,preview,sessions,store}` and the four integration-test roots (`harness/tests/real_quiet_survey.rs`, `preview/tests/transport.rs`, `sessions/tests/{side_chats,rich_composer_delivery}.rs`) were removed and the code fixed. **Debug:** 44 public types gained `Debug` (26 in sessions, 10 in harness, 7 in preview, 1 in store). Data types derive it (`SteerMessage`, `MockHarness`, `VerifiedRelease`, `LocalRoute`, `Router`, `SocketTransport`, `mux::Stream`, Windows `Attributes`). Service handles, harnesses and other types holding closures, locks or channels get a `finish_non_exhaustive` impl, as in T1-13. **`unwrap()` outside tests:** preview's mutex locks use the crate's `lock()` helper (`unwrap_or_else(PoisonError::into_inner)`, the sessions convention). The credit frame decodes with `<[u8; 4]>::try_from` and an error, and a missing browser upgrade is an error. Harness `model_catalog`s go through `model_context::required`, so a `None` context is `HarnessError::Protocol` rather than a panic. Pi's serialisations map to errors or fall back. The Pi session dir is checked with `let … else`. Invariants already proven by the surrounding code use `expect` with the invariant. **Design lints**, as elsewhere in the workspace (`loams-query`, `loams-kv`, `loams-safekeeper`): `large_enum_variant` on `SessionCommandPayload` and `MessagePart`, and `too_many_arguments` on `opencode::post_prompt`, `DocHost::finish_queued_message_edit_with_attachments` and one opencode test fixture, each as an item-level `#[allow]` with its reason. **Rewritten:** `explicit_counter_loop` (`summarize_tool_output` uses `char_indices().nth`) and `while_let_loop`; `type_complexity` uses type aliases (`devin_models::Probe`, test-only `HarnessCtor` and `Runs`); the test-only `Delivery::Run` is boxed. `field_reassign_with_default`, `question_mark` and `doc_lazy_continuation` no longer fired: the code that tripped them went in Tasks 2–3. What remains at file level is generated code (`loams-agentd-link/src/gen`), `#![allow(clippy::unwrap_used)]` in integration tests and examples (workspace convention), `dead_code` in the shared `tests/source_scan` module, and `refining_impl_trait` in the link mock. **Windows and macOS** code does not compile on Linux, so it was audited by reading it: every `unwrap()` there is inside a `cfg(all(test, …))` module, and `Attributes` (public `windows_process`) was the only exported type without `Debug` | Brief, T1-13 | — |

### Task 5 rulings (2026-10-10)

| # | Ruling | Why | Absorbed by |
|---|---|---|---|
| T5-1 | **The README gives the contract's run command and the one that works today.** `cargo run -p loams-agentd -- run --child --config <file>` is the documented form, as the brief asks. `run` takes no flags until Task 6 adds `--child`, `--service` and `--config`, so the README also shows the environment form (`LOAMS_DESKTOP_DATA_DIR`, `LOAMS_DESKTOP_IPC_PORT`, `cargo run -p loams-agentd -- run`) and says when it goes | A README whose only command fails is worse than one that names the gap | 6 (drops the interim form) |
| T5-2 | **The licence table and `licence_fields_are_set`.** The table is under `## Licences` in `crates/loams-agentd/README.md`, with columns crate, licence, origin and notes. Origin is `zeron` (MIT), `Loams Desktop` (Apache-2.0: `loams-agentd-link`) or `new` (Apache-2.0). The five new crates of §50 §4.1 (`llm`, `loop`, `tools`, `factory`, `supervisor`) are listed already. The test is in `crates/loams-agentd/tests/workspace.rs` and reads `cargo metadata --no-deps`. Every `loams-agentd*` package must have a row with the same `license`. A `zeron` row must be MIT and a `Loams Desktop` or `new` row Apache-2.0. Only a `new` row may name a crate that does not exist yet. A `new` crate's manifest must set `license.workspace = true`, which is read with `toml` (a new dev-dependency of `loams-agentd`), because `cargo metadata` resolves inheritance. This is how "every new crate declares the workspace licence" is enforced for Tasks 14–23 | The brief's "checks each package's `license` against the table", made strict enough to catch a missing row or a new crate copying `license = "MIT"` | 14, 15, 18–23 (add the crate; the row exists) |
| T5-3 | **The provenance commit is the fork's.** `import-provenance.json`'s `source_commit` (`c9205f8f…`) is a commit of `ostrium-labs/loams-desktop`, not of `zeronsh/zeron`. The fork keeps zeron's history, so that commit defines the zeron code, but no zeron-side commit was recorded at import and none is invented. The NOTICE says the MIT crates derive from zeron "through the Loams Desktop fork, at the commit recorded as `source_commit`". The file gains `moved_to_root_workspace`: zeron's and the fork's repositories, licence and copyright, and, for each of the 17 packages in `desktop_dependency_closure`, where it went and its licence, or why it was deleted. A note says the existing hashes describe the import, not today's files | The brief's "the commit in `import-provenance.json`" | — |
| T5-4 | **The daemon's `NOTICE` is rewritten. `LICENSE` and `THIRD_PARTY_NOTICES.md` are unchanged.** The old `NOTICE` was the fork's own (Loams-authored; it named `crates/loams-brand` and `dist/loams`), not zeron's, so it is replaced. The new one has: zeron's MIT notice; which crates are MIT and which Apache-2.0; the T0-10a grant for the brand lines; the fork's added third-party components (`connectrpc`, `buffa`, `keyring` and `webbrowser`, all still in `loams-agentd-link`); and the status of the other notice files. `THIRD_PARTY_NOTICES.md` stays as imported. Its components (tree-sitter grammars, GPUI and gpui-component, Symbols icons, fonts, the voice model) are no longer in `cargo tree -p loams-agentd --target all`, and the NOTICE says so. `SCOPED_NOTICE.md` gains a "kept for history" preface, because its "preserved byte-for-byte" claim no longer covers `NOTICE`; the old text is at `6e470a96^:apps/desktop/native/NOTICE`. The root `NOTICE` paragraph now names zeron's URL, the provenance commit and the README table | The fork's NOTICE described a layout that no longer exists | — |
| T5-5 | **§18's status line already pointed at §50** (written with the plan). Task 5 adds where the licence, notices and import record live, and the MIT/Apache split | Brief | — |
