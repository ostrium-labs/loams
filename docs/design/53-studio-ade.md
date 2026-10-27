# 53 — Loams Studio: an Agentic Development Environment in the cordis Console

> **Amendment (approved 2026-10-23) by [§78](78-loams-cloud-on-tidb-x.md)** (Loams Cloud on TiDB X; D1700–D1735). Studio's remote-agentd host registry (D909) becomes Loams Cloud's device registry; a run's workspace may be a TiDB Cloud Filesystem layer; runs are pinned to a device under a lease and hand off between a user's devices explicitly (drain, checkpoint, fork) (D1713–D1716).

Status: **Proposed; mostly built** · 2026-10-13, updated 2026-10-15. SD1 Tasks 0–28 and 30 are on `dev`; how each decision was built, and where it differs from this text, is in the [decision log](13-decision-log.md) (D900–D922's status column) and the plan's "Rulings made during execution". The user guides are in [docs/guides/studio](../guides/studio/index.md). Not built yet: the e2e suite (Task 29) and the browser default (Task 32). The security review is [docs/security/studio-threat-model.md](../security/studio-threat-model.md). This document is otherwise as written on 2026-10-13. Source: the owner's direction of 2026-10-13 (§1.1). Decisions **D900–D922** and open questions **Q800–Q819** are proposed numbers. They sit above the highest numbers the documents on `dev` use (D891 and Q795, §52), with a gap left for work in flight. They go into the [decision log](13-decision-log.md) when the owner confirms them. The plan is [SD1](../plans/2026-10-13-sd1-studio-ade.md). The vendoring inventory is [studio-vendor-inventory.md](studio-vendor-inventory.md). **This document writes no code.**

This document builds on, and amends where §17 says:
- [§37](37-desktop-and-mobile-apps.md) §5: the cordis host, the manifest, slots, trust tiers and editions (D422–D428). §19 covers the Electron desktop and its cloud-console layout (D652–D679).
- [§50](50-loams-desktop-daemon.md): `loams-agentd`, its ndjson RPC, roles, the renderer allowlist and the DSH UI port (D780–D799).
- [§39](39-software-factory-and-loams-bot.md): the software factory apps and the factory loop (D-SF-1–D-SF-20).
- [§44](44-unified-api-and-sdks.md): one Connect API (D600–D619).
- The console UI stack rulings of 2026-10-13 ([console-ui-stack.md](console-ui-stack.md), being recorded on the branch `console/ui-stack`).

It was checked against the code on `dev` at `6c8481e8` (2026-10-13). The T3 Code source it inventories is described in §16.

Markers:
- **(verify)** means not checked against a primary source; the plan task that depends on it checks it first.
- **(estimate)** means computed or guessed, not measured.
- **[P]** marks a fact read from a primary source on 2026-10-13: this repository, or the T3 Code source tree.

---

## 1. Summary

### 1.1 The owner's direction (2026-10-13, binding)

1. **One shell, two modes.** The cordis console host runs in the browser and in Electron. Its parts are `web/apps/console` `cordis.html`, `web/packages/console-host`, `@loams/slots`, `@loams/ui` and `web/plugins/*`; `apps/desktop-electron` loads `cordis.html`. The host gets a mode switcher with two modes:
   - **Console** is the cloud console: orgs, projects, environments, data, compute, connectors, access and audit.
   - **Studio** is the ADE: agent threads, a worktree per agent, diff review, parallel agents compared side by side with the best result merged (Orca's idea), approvals and terminals.
2. **The software factory apps belong to Studio.** Zulip, Plane, Forgejo, OpenPanel, GlitchTip, Langfuse and SigNoz move there. Plugin manifests declare which modes a plugin appears in.
3. **Vendor, don't embed.** Over the long term, we vendor UI components and other useful code from T3 Code (MIT) into this repository and rebuild them natively on our stack, backed by `loams-agentd`. We do not embed T3 Code's app or its server.
4. **The UI stack rulings apply:**
   - Tailwind 4 with `@loams/ui` tokens only.
   - Base UI primitives, wrapped once in `@loams/ui`.
   - No Effect.
   - `@connectrpc/connect-query` with TanStack Query for server state.
   - TanStack Table and Virtual.
   - One version of each shared dependency, through the pnpm catalog.
5. **Where Studio runs:**
   - The web edition runs against a **remote** `loams-agentd`.
   - The desktop edition uses local repositories and agent CLIs through the **local** `loams-agentd`.

### 1.2 Decisions (proposed)

| # | Decision | § |
|---|---|---|
| D900 | **Studio is a mode of the one cordis console, not a second app.** The host, the transport, identity, the query cache, the right dock, approvals, notifications and the palette are shared between the modes. A plugin is loaded once and appears in the modes its manifest names | §4 |
| D901 | **`modes` is a new manifest field.** Its value is `["console"]`, `["studio"]` or both; omitting it means `["console"]`. Core plugins appear in every mode. The host gains a `mode` service (current mode, `set`, `onChange`). Switching modes filters nav, pages and palette commands, and never reloads the window | §4.1–§4.2 |
| D902 | **Default mode per edition.** The browser edition opens Console; the desktop edition opens Studio. After that, the last mode used on the device wins. Studio routes live under `/studio`, so a deep link selects its mode. The desktop deep links are `loams://open/studio/…`, and `loams://open/agent/<id>` keeps working | §4.3–§4.4 |
| D903 | **Studio's model is project → thread → worktree.** A Studio project is an agentd *space* plus its repository. A thread is exactly one agentd *chat*. By default each thread runs in its own git worktree on its own branch; "local" (run in the main checkout) is a per-thread choice | §5.2–§5.4 |
| D904 | **Parallel runs are run groups.** One prompt fans out to N threads, each with its own worktree and its own harness or model. A compare view puts their diffs, checks, time and tokens side by side. The person picks one result and merges it or opens a PR from it; the rest are archived and their worktrees removed. The daemon owns the group record, so a group survives the window closing | §5.5 |
| D905 | **Diff review with comments.** Comments are anchored to (checkout, path, side, line, the diff checksum). The daemon stores them. They can be sent to the thread's agent as one turn, or posted to the Forgejo PR | §5.6 |
| D906 | **`loams-agentd` is Studio's only backend for code work.** Studio never talks to a T3 Code server and never runs one. The Loams server is used only for Loams resources and for the factory's `loams.collab.v1` | §6 |
| D907 | **Studio reaches agentd through a Connect surface, `loams.agentd.v1`.** A new crate, `loams-agentd-connect`, serves it beside the ndjson RPC. Its handlers call the same engine functions as the ndjson dispatcher. The ndjson wire stays for Electron main, the MCP shim and the CLI (DD1 Ruling 2 stands). Studio reads it with connect-query | §6.1 |
| D908 | **Desktop transport.** The renderer calls `loams-app://console/loams.agentd.v1.*`. Electron main's protocol proxy forwards the call to the local Connect listener, adds the owner token, and refuses any method not marked `renderer: true`. The renderer never sees the token (D788 and D789 hold) | §6.2 |
| D909 | **Web transport: remote agentd.** `loams-agentd serve --remote` opens a TLS Connect listener, off by default. It accepts only IdP-issued access tokens (Zitadel, §76 D1600) with the audience `loams-agentd` whose subject is the one principal the daemon was enrolled for. It sends CORS headers only to configured console origins. This amends D788's "no browsers" rule for that listener only; the loopback ndjson listener keeps it | §6.3, §8.2 |
| D910 | **New agentd RPCs** fill the gaps of §6.5: git writes (commit, push, merge, change requests for Forgejo and GitHub), a worktree list, per-turn checkpoints and restore, review comments, run groups with compare and resolve, check runs, approvals as a list and a stream, notifications, and per-session usage | §6.5 |
| D911 | **Approvals are real in Studio.** Each thread has an approval mode: `full-access` (today's behaviour) or `supervised`. In `supervised` mode, the harness drivers stop auto-approving. They surface each permission request as `ApprovalRequested`, which only an `owner` resolves through `ResolveApproval` (DD1 Task 24's contract, extended to the harness drivers). Studio's approval UI and Console's approvals inbox read the same stream | §8.1 |
| D912 | **Vendored code lives in `web/packages/studio-ui` (`@loams/studio-ui`).** `provenance.json` records the upstream commit, the upstream path, the upstream SHA-256 and the local SHA-256 of every vendored file. An import tool copies files from a verified source archive without executing anything in it. Vendored files are ported to `@loams/ui` tokens, Base UI through `@loams/ui`, and connect-query; no Effect, Effect atoms or T3 contracts survive the port | §9 |
| D913 | **Licence and attribution.** Every vendored file keeps an MIT header that names T3 Code, the upstream path and the commit. T3 Code's LICENSE is kept verbatim as `web/packages/studio-ui/LICENSE.t3code`. The root NOTICE gets one entry (CONTRIBUTING rule 4). The package is Apache-2.0 with MIT portions, as `loams-agentd` is | §9.2 |
| D914 | **An npm licence gate.** `scripts/ci/npm-licences.mjs` checks every production dependency of `web/` against the same allow list as `deny.toml`, and fails on anything else. Today there is no npm licence check | §9.3 |
| D915 | **Re-sync policy.** Vendored code is a fork. We re-sync on demand and at most once per month, by a scripted three-way diff from the recorded commit to the new one, one component group per PR. Security fixes upstream are taken within 7 days | §9.4 |
| D916 | **The factory moves under Studio.** `@loams/plugin-factory` declares `modes: ["studio"]`. Studio's sidebar gets a Factory group, and threads link to factory objects (a Plane issue starts a thread; a thread's branch opens a Forgejo PR; a GlitchTip issue starts a "fix this" thread). This amends D666's console nav | §5.9 |
| D917 | **One session store, one transcript.** Studio's chat view and the docked agent panel render the same `@loams/agentd-client` reducer state. Where DD1e (Tasks 29–31) and this plan would build the same component, SD1's vendored component replaces DD1e's; DD1e keeps what T3 Code lacks (Q802) | §5.3, §17 |
| D918 | **Studio plugins are first-party only.** The `rpc.agentd.*` services can be injected only by `core` and `first-party` plugins. A third-party frame never reaches agentd, not even through the bridge | §8.3 |
| D919 | **Rendering rules from AP1e and §50 §14.5 hold for vendored code.** That means no raw HTML from models, tools or diffs; http(s) links only; no remote images. Syntax highlighting produces tokens that render as React text nodes, or HTML generated only from escaped text inside a shadow root. Every vendored renderer gets an XSS fixture test | §8.4 |
| D920 | **Terminals use xterm.js** at the version DD1 pins, through one `@loams/studio-ui` terminal component that Studio and the agent panel share. T3 Code's own terminal is a libghostty-vt wasm renderer; its drawer, tabs, link detection and selection-to-composer code are vendored on top of xterm instead (Q818) | §5.7 |
| D921 | **Remote agentd runs as one user's daemon.** A remote agentd serves one principal: a personal dev box, a VM or a §15 workspace. A multi-user hosted agentd is out of scope here (Q805) | §6.3 |
| D922 | **Factory apps on the web wait for SF1.** In the browser, Studio's Factory group shows native panels through `loams.collab.v1` and frames the app UIs through SF1's `embed` plugin. Until SF1 lands, the browser Factory group shows the links only | §5.9, §7 |

### 1.3 What it looks like

```
┌ header ─────────────────────────────────────────────────────────────────────────────────────┐
│ [Loams]  ( Console | Studio )   project ▾  branch loams/fix-login ▾   ⟳ 2 running  ⚠ 1   [◧] │
├ sidebar ─────────────┬ thread ─────────────────────────────────────┬ panel ──────────────────┤
│ ▾ loams              │  ▸ user: fix the login redirect …           │ [Diff] Files Term Prev  │
│   ● fix-login   2m   │  ▸ assistant: I'll look at auth.ts …        │ src/auth.ts  +12 −3     │
│   ◐ run: retry ×3    │    ⎿ read_file src/auth.ts                  │  @@ -40,7 +40,9 @@      │
│     ├ claude  ✓      │    ⎿ edit src/auth.ts            [diff]     │  − if (!next) …         │
│     ├ codex   ✓      │  ▸ approval: run `pnpm test`? [Once][Deny]  │  + if (!isSafe(next)) … │
│     └ opencode ✗     │                                             │  💬 "also cover //x"    │
│ ▾ web                │                                             │                         │
│ ─ Factory ─          │ ┌ composer ───────────────────────────────┐ │                         │
│   Forgejo  Plane     │ │ @file /command  …          claude ▾ ⏎   │ │                         │
│   Zulip  GlitchTip … │ └─────────────────────────────────────────┘ │                         │
├──────────────────────┴─────────────────────────────────────────────┴─────────────────────────┤
│ terminal ▸ zsh — ~/.loams-desktop/worktrees/loams/fix-login                                   │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

The header, the mode switcher, the approval badge and the dock toggle `[◧]` are the shell's. The docked agent panel (D666) stays available in both modes. In Studio it shows the selected thread's compact view or a quick-ask thread.

---

## 2. Goals and non-goals

### 2.1 Goals

1. **An ADE as good as T3 Code and Orca for daily work**, inside Loams' console:
   - threads with any supported harness (Claude Code, Codex, opencode, Pi, the ACP agents and Loams' own loop);
   - a worktree per thread;
   - reviewable diffs with comments;
   - terminals and a preview;
   - parallel runs with compare and merge.
2. **One shell.** A person moves between running their data platform (Console) and building software with agents (Studio) without switching apps or signing in twice. Approvals and notifications from both reach one inbox.
3. **One agent backend.** Everything Studio does goes through `loams-agentd`, so desktop and web behave the same way. The daemon's durability (§50 §11), its secrets handling (§50 §12) and its security model (§50 §7) cover Studio too.
4. **Own the code.** Vendored components become ours, written on our UI stack, tested and tokenised, with clear provenance and a re-sync path.
5. **The factory in context.** Issues, PRs, errors, traces and chat sit next to the threads that act on them.

### 2.2 Non-goals

- **No embedded T3 Code.** We don't run its web app, its server, its Effect runtime, its WebSocket protocol or its provider adapters (owner direction).
- **No new agent runtime.** Harnesses stay the daemon's. Studio adds no model calls of its own (titles already exist in the daemon).
- **No hosted multi-user agentd** in SD1 (D921, Q805). That is a cloud product question.
- **No IDE.** Studio has a file viewer, diffs, a terminal and a preview. It has no general code editor, language server or debugger. "Open in editor" hands off to the person's own editor on the desktop (Q810).
- **No cloud sync of threads between devices.** That is §50's Q700; it stays out of scope.
- **No mobile Studio.** The phones keep approvals and notifications (§37 §7).
- **No auto-merge.** A run group's merge always needs a person's click. Policy-driven merges stay the factory's (D-SF-14).

---

## 3. Where things are today (checked on `dev` at `6c8481e8`)

| Area | As built [P] | Consequence for Studio |
|---|---|---|
| Console host | `web/packages/console-host`: boot, catalog, manifest validator (`manifest.ts`, a closed `KEYS` set), tiers, sandbox frames, permissions, actions. Editions in manifests are `oss`, `desktop` and `cloud`; `FlagsService.edition` is `oss`, `cloud`, `byoc` or `unknown` | `modes` must be added to `KEYS`, to `plugin-manifest.schema.json` and to the validator (SD1 Task 3) |
| Shell | `web/plugins/shell`: a hash router; nav sections `Data`, `Compute`, `Integrate` and `Organisation`; the `shell.dock.right` and `shell.header.server` slots | The mode switcher goes in the header; nav filtering happens in `navSections` |
| UI stack | `@loams/ui` today has tokens, Button, Card, Table and a few others. Base UI, DataTable, VirtualList, `ServerStateProvider` and the pnpm catalog are on the unmerged branch `console/ui-stack` (PR #513: `82269206`, `13180ecd`, `9feea35a`, `670ea5c8`, `dbdf37ab`; checked by SD1 T0-1) | SD1 depends on them landing (Task 0 checks) |
| Agent panel | `@loams/plugin-agent` talks to Electron's in-process TypeScript loop (`desktop.chat.*`, D675), not to agentd | DD1 Task 27/34 moves it to agentd; Studio builds on the agentd path only |
| agentd | 90 ndjson methods (77 unary, 13 streams; 88 `renderer: true`, all but `Hello` and `Shutdown`) in `crates/loams-agentd-proto/src/rpc.rs:267-401` (SD1 T0-3). Covered: chats, spaces, repos, worktrees, diffs, git status, terminals, files, previews, harness and model catalogues, queue and steering. Missing: git writes, real approvals, checkpoints, review comments, run groups, notifications, per-session cost. No protobuf; no Connect; no browser access (any `Origin` gets a 403) | §6 |
| agentd client | `@loams/agentd-client`: ts-rs types, the method table, typed request builders and the transcript reducer. No transport yet (DD1 Task 10) | Studio adds the Connect transport (§6.2) |
| DD1 progress | Rulings recorded through Task 9. Tasks 10–38 are not done, including the approval loop (24) and the DSH chat UI (28–32) | §17 reconciles SD1 with DD1e |
| Factory | One plugin, `@loams/plugin-factory`, desktop edition only. It holds a hub, a configure form, embedded `WebContentsView` apps (D678) and native panels for Zulip, Plane, Forgejo, GlitchTip, OpenPanel, Langfuse and Matomo; SigNoz has the full UI only. SF1's browser `embed` plugin and per-app plugins are not built | The owner's "already exist as cordis plugins" holds for the desktop; the web needs SF1 (D922) |
| Licence gates | `cargo deny` for Rust. **No npm licence check** in CI | D914 |
| T3 Code (the source) | React 19 SPA, 608 tsx files (484 non-test, 173 457 lines). Server state is Effect atoms (`@effect/atom-react`) over Effect RPC on one WebSocket; client UI state is zustand 5; TanStack Router; Tailwind 4; Base UI primitives in `components/ui` (shadcn `base-mira`); Tiptap 3 composer; `@pierre/diffs` 1.5.2 **with a local patch** (find-in-diff) and `@pierre/trees` 1.0.0-beta.6; `@legendapp/list` (patched) for virtualisation; a libghostty-vt wasm terminal. Checkpoints are hidden commits under `refs/t3/checkpoints/<thread>/turn/<n>`, written through a temporary index. **No parallel-run compare UI exists** | Every container component needs a new data layer; leaf components, the Tiptap modules, pure logic and keybindings port well (inventory). The compare view is ours to build |
| Naming | A desktop page called **Data Studio** exists (`@loams/plugin-data-studio`, D653) | Q812 |

---

## 4. The mode switcher

### 4.1 The manifest field (D901)

A plugin's `loams.plugin` block gains:

```json
"modes": ["studio"]
```

| Value | Meaning |
|---|---|
| absent | `["console"]`, so every existing plugin keeps its place |
| `["console"]` | Console only |
| `["studio"]` | Studio only |
| `["console", "studio"]` | Both. Each nav entry says which mode it belongs to (below) |

- Core plugins (`shell`, `rpc`, `identity`, the platforms) ignore `modes`. They are present in every mode.
- `editions` and `modes` are independent. `@loams/plugin-factory` today is `editions: ["desktop"]`; after D916 it is `editions: ["desktop", "oss", "cloud"], modes: ["studio"]` once its web half exists (D922).
- The validator rejects unknown modes. The host rejects a `shell.nav.section` entry, a page or a palette command whose plugin's `modes` does not include the mode it is shown in.
- A plugin in both modes can give its nav entry a `meta.mode` (`"console"` or `"studio"`). Without one, the entry shows in each of the plugin's modes. Example: approvals is in both, with one nav entry per mode.

### 4.2 The `mode` service and switching

```ts
interface ModeService {
  readonly current: 'console' | 'studio';
  readonly available: ReadonlyArray<'console' | 'studio'>;  // studio absent if no plugin declares it
  set(mode: 'console' | 'studio', opts?: { navigate?: string }): void;
  onChange(cb: (mode: 'console' | 'studio') => void): () => void;
}
```

- The host provides `mode`. The shell renders the switcher (a segmented control in the header, keyboard `Mod+Shift+M`) only when `available` has both modes.
- **Loading.** The plugins of the boot mode load at boot. The other mode's plugins load the first time that mode is entered, and then stay loaded: their fibers, streams and caches survive later switches. Studio's agentd streams keep running while the person is in Console, so the approval badge stays live.
- **What switching changes:**
  - the nav model (`navSections` filters by mode);
  - the home route of the mode (`/` for Console, `/studio` for Studio);
  - the palette's default scope;
  - the header's context controls. Console shows the environment and server switcher. Studio shows the project and branch, plus the agentd host on the web.
- **What it doesn't change:** the transport, the session, the query cache, the dock, overlays, toasts, approvals and notifications.
- The current mode is stored per device under `loams.shell.mode`. Reads and writes are wrapped in try/catch; when storage fails, the edition default applies.

### 4.3 Default mode per edition (D902)

| Edition | Default | Why |
|---|---|---|
| Browser (`web.yml`, hosted, BYOC) | Console | The hosted console is first a cloud console. Studio needs a remote agentd that most visitors won't have yet; without one, Studio shows "connect an agentd host" (§6.3) |
| Desktop (`desktop.yml`) | Studio | The owner places local repositories and agent CLIs in the desktop. The local engine, data pages and factory stay one click away |

The last-used mode overrides the default after the first switch. Q801 asks whether the desktop default should be Studio from the first run, or Console until the first project is added.

### 4.4 Routes and deep links

- Studio routes are prefixed `/studio`:
  - `/studio` (home: recent threads, projects, run groups);
  - `/studio/p/:projectId`;
  - `/studio/t/:threadId`, with the panels `?panel=diff|files|terminal|preview`;
  - `/studio/g/:groupId` (compare);
  - `/studio/factory/:app`.
- Opening a route of the other mode switches the mode first. A Console route never starts with `/studio`, and every existing route keeps its path.
- **Desktop.** `loams://open/studio/t/<threadId>` and `loams://open/studio/g/<groupId>` join §19.5's deep-link allowlist. The existing `loams://open/agent/<sessionId>` opens the same thread (an agentd chat id is the thread id).
- **Web.** `https://<console>/ui/#/studio/t/<threadId>?host=<agentd-host-id>`. The `host` parameter picks a registered remote agentd (§6.3). It is a registry id, never a URL, so a link can't point the console at an attacker's host.
- **OS notifications** from the daemon (§50 §5.5) open thread links.

### 4.5 What both modes share (D900)

| Shared piece | Owner | In Console | In Studio |
|---|---|---|---|
| Header (logo, mode switcher, user menu) | shell | environment and server switcher | project, branch and host |
| Right dock `shell.dock.right` | agent plugin | the agent panel (D675, then DD1's daemon panel) | a compact view of the selected thread, or a quick-ask thread |
| Approvals | `@loams/plugin-approvals`, both modes | the inbox page, and `approval.renderer` per kind | an inline card in the thread; the same inbox with an `agent` filter |
| Notifications | the `shell.overlay` toasts; on the desktop, the OS via the daemon | engine, jobs and operations | turn finished, approval waiting, check failed, group ready |
| Command palette `palette.command` | shell | the mode's commands first | the other mode's commands under "Console" or "Studio" |
| Identity, transport and the query cache | core | — | — |
| Settings | `console.settings.section` | — | Studio adds harnesses, approval defaults, worktree location, keybindings |

The approval badge counts both the Loams server's `loams.approvals.v1` approvals and agentd's pending approvals. Each source is labelled.

---

## 5. Studio's information architecture

### 5.1 Layout

The layout has four regions:
- **Sidebar.** Projects, each with its threads and run groups, then the Factory group. It is resizable and collapsible.
- **Thread.** The chat timeline and the composer.
- **Panel.** A right-hand tabbed panel: Diff, Files, Terminal, Preview and Plan.
- **Terminal drawer.** At the bottom.

It also has:
- **Toolbar.** Over the thread: the branch, the worktree, the git actions and the harness and model.
- **Dock.** The shell's right dock, outside Studio's own panel.

On narrow windows, the panel and the dock become overlays, and the sidebar becomes a drawer.

### 5.2 Projects and repositories

- **A Studio project is an agentd space**: a folder on the daemon's host (`WatchSpaces`, `Mutate createSpace`), which is usually a git repository (`ListRepos`, `AddRepo`, `CloneRepo`, `CreateRepo`).
- **Desktop.** "Add project" opens the OS folder picker through the desktop platform service, then calls `AddRepo`.
- **Web.** "Add project" clones a URL on the remote host (`CloneRepo`). A Forgejo repository of the factory or a Loams Git URL (§36) is offered first.
- **Optional binding.** A project can be bound to a Loams project and environment (§19). The binding is used for factory links and for the agent's Loams tools. Stored as space metadata (Q807).

### 5.3 Threads

- **A thread is one agentd chat.** It has a title, a harness, a model, a reasoning level, a sandbox level, an approval mode, a working directory (worktree or main checkout), a branch, a status (`Idle`, `Working`, `AwaitingInput`, `Errored`) and a parent (side chats and run-group members).
- **Lifecycle:**
  - create: `Mutate createChat`, plus `setChatConfig`;
  - send: `QueueCommand run` with `RunRequest.worktree`;
  - steer: `QueueCommand steer`;
  - queue while busy: `QueueMessage` and the queue methods;
  - interrupt: `QueueCommand interrupt`;
  - answer a question: `QueueCommand respondInput`;
  - rename, archive or delete: `Mutate`;
  - fork: `ForkSideChat`.
- **Timeline.** `WatchDocMessages` streams `TranscriptUpdate`, folded by `@loams/agentd-client`'s reducer. One store serves the Studio thread view and the dock (D917).
- **Sidebar badges.** Running, waiting for approval, unread, errored, and part of a run group. Threads are sorted by last activity, can be pinned (the existing sidebar pin doc) and searched.

### 5.4 Worktrees

- **One worktree per thread by default.** The first run sends `RunRequest.worktree = {repoPath, base, spaceId}`. The daemon creates `~/.loams-desktop/worktrees/<repo>/<name>` on the branch `loams-desktop/<name>` [P] (`repos.rs`). Studio shows the branch and the path.
  - The prefix `loams-desktop/` predates Studio. Q809 asks whether to rename it to `loams/`.
- **Local mode.** The thread runs in the project's main checkout. Studio warns when another thread is already working there.
- **A worktree's life:**
  - It is kept after the thread is archived.
  - The sidebar's "Clean up" lists stale worktrees (`ListWorktrees`, new) with their diff status and removes them (`DeleteWorktree`).
  - A worktree with uncommitted changes needs a confirmation before removal.
- **Setup.** A project action marked as setup (`ListProjectActions`, `TakeProjectActionSetup`, for example `pnpm install`) runs in a new worktree before the first turn. Its output goes to the terminal drawer.

### 5.5 Parallel runs: compare and merge (D904)

A **run group** is one prompt, run N times in parallel. It works in four steps:

1. **Start.** "Run in parallel" opens a dialog in the composer. It takes the prompt, attachments and the base ref. It also takes N variants (harness, model, reasoning); the default is the same model three times, or one each of the enabled harnesses. An optional **check** is a project action, for example `pnpm test`. `CreateRunGroup` makes N chats, each in its own worktree from the same base, and queues the same run in each.
2. **Watch.** `/studio/g/:id` shows the group as a grid of columns, one per variant. Each column shows the status, the last message, the files changed, `+/−`, elapsed time, tokens (when the harness reports them) and the check status.
3. **Compare.**
   - **Diff matrix.** Files on one axis and variants on the other, coloured by change size. It shows which variants touched which files.
   - **Side by side.** Pick two variants and see their diffs for one file side by side, or the diff between their results (`GetRunGroupComparison`, which computes `git diff A..B` across the two worktrees).
   - **Check results.** Each variant's check exit code and log tail (`RunGroupCheck`).
   - An optional **judge** thread can be started on the comparison. It is an ordinary thread with a read-only sandbox, so it is just another agent and gets no special trust (Q811).
4. **Resolve.** `ResolveRunGroup` takes the winner and an action:
   - `merge`: merge into the base with the chosen strategy;
   - `pr`: push and open a change request;
   - `keep`: leave it as a normal thread.

   The losers are archived, and their worktrees are removed after a confirmation. A person can also continue a variant as a normal thread before resolving.

The daemon keeps the group record, so a group keeps running and stays resolvable after the window closes. Runs go to a harness's own queue, so N is bounded by the daemon's concurrency limit (default 4 running turns per group, 8 per daemon (estimate), configurable).

### 5.6 Diff review with comments (D905)

- **Diff sources** (all exist):
  - the working tree against the base (`GetCheckoutDiff mode: ""`, or `branch` with `baseRef`);
  - a commit;
  - one turn (`mode: "turn"`);
  - per file: `GetCheckoutFileDiffText`, live via `WatchCheckoutDiffs` and `WatchWorkspaceGitStatus`.
- **Display.** Unified or split view; file tree and file list; collapse; "viewed" marks (kept on the client); whitespace toggle; word diff.
- **Comments.** Each is anchored to `{checkoutId, path, side: old|new, line, endLine?, diffChecksum}`. When the diff changes, a comment whose anchor no longer matches is shown as outdated, as on a code host.
  - The daemon stores them: `UpsertReviewComment`, `DeleteReviewComment`, `ResolveReviewComment`, `WatchReviewComments`.
- **Send to agent.** "Ask the agent to address N comments" queues a turn whose prompt quotes each comment with its file and line, in a fixed template (`SendReviewToThread`).
- **Post to PR.** When the thread's branch has a Forgejo or GitHub change request, comments can be posted as a review (`PostReviewToChangeRequest`, through the provider's API with the factory credential, §6.5).
- **Discard.** Discard all, or discard one file, with a confirmation and the expected checksum (`DiscardWorkingTree`, plus a per-path variant).
- **Restore a turn.** `RestoreCheckpoint` resets the worktree to the state before a turn. Checkpoints are refs that the daemon writes at each turn start (§6.5).

### 5.7 Terminals (D920)

- **Per thread.** Terminals open in the thread's worktree: `OpenTerminal {chatId, cols, rows}`, then `SubscribeTerminal` (with a 1 MB replay), `WriteTerminal`, `ResizeTerminal` and `CloseTerminal`.
- **Tabs.** A thread can have several. Shells survive detach, so reopening a thread reattaches.
- **Rendering.** xterm.js with the fit addon and a token-coloured theme. Pasting more than one line asks for confirmation (§50 §14.5).
- **Project actions** (dev server, tests, lint) run as named terminals (`RunProjectAction`) and can be pinned to the toolbar.

### 5.8 Preview

- **Source.** `WatchPreviews {chatId}` lists the dev servers that the daemon discovered under the project, with stable URLs `http://<name>.<project>.localhost:7331`.
- **Desktop.** The Preview tab shows the URL in a `WebContentsView` with D678's isolation: its own partition, no preload, sandboxed, and the frame and permission policy. "Open in browser" opens the URL with `openExternal`.
- **Web (remote agentd).** The `*.localhost:7331` URLs aren't reachable from the browser. The remote agentd serves previews through its TLS listener at `https://<host>/preview/<name>.<project>/…`, behind the same token check as the API (Q806). Until that exists, the Preview tab on the web shows the port and the command to forward it (`ssh -L`).

### 5.9 The factory apps in Studio (D916, D922)

| App | In Studio's sidebar (Factory group) | In context |
|---|---|---|
| Forgejo | Repositories, PRs, CI | A thread's toolbar shows the branch's PR and its checks. "Open PR" uses `CreateChangeRequest` (provider `forgejo`). Review comments can be posted to the PR |
| Plane | Issues, cycles | "Start thread from issue" puts the issue's title, description and link in the composer, and names the branch after the issue key |
| Zulip | Streams and topics | A run group's or thread's result can be posted to a topic (a factory write, with approval per §39 §8) |
| GlitchTip | Issues | "Fix this error" starts a thread with the stack trace and the release |
| OpenPanel | Dashboards | Read-only panel |
| Langfuse | Traces | A thread links to its traces when the harness exports OTel (§39 §9) |
| SigNoz | Logs, metrics, traces | Full UI only, as today |
| Matomo | (present in the factory plugin today; not in the owner's list) | Kept where it is, in Studio (Q813) |

- **Desktop.** The existing plugin keeps its pages (hub, configure, embedded app, panels) under `/studio/factory/…`. The old `/factory/…` paths redirect. Its overview card stays on Console's overview (the plugin is in both modes for that card only, through `meta.mode`).
- **Web.** Native panels come from `loams.collab.v1` (SF1 Task 3), and the full UIs from SF1's `embed` plugin (iframes behind the edge, D-SF-3).
- **The factory loop.** A run's "fix" stage (§39 §10, stage 4) can appear in Studio as a thread on the run's branch. SF4 decides whether `propose_patch` runs through agentd (Q814).

### 5.10 Command palette, keybindings and settings

- **Palette** (`Mod+K`). It covers threads, projects, run groups, files in the current worktree (`SearchWorkspaceFiles`), git actions, project actions, harness and model switches, and the other mode's commands.
- **Keybindings.** T3 Code's keybinding model (`packages/shared` parsing, a when-clause scope) is vendored and stored on the client per device. A daemon-side store comes later if threads sync (Q700).
- **Settings › Studio:**
  - default harness, model and approval mode;
  - worktree location;
  - setup action;
  - parallel-run defaults;
  - notification preferences;
  - keybindings.

---

## 6. The agentd mapping

### 6.1 One Connect surface for Studio (D907)

The daemon speaks ndjson over a loopback WebSocket, and it refuses browsers by design (§50 §7). The owner's UI stack ruling makes connect-query the way pages read server state. A connect-query hook needs a protobuf method descriptor, so a client that only speaks ndjson can't use it. The options:

| Option | For | Against |
|---|---|---|
| **A. `loams.agentd.v1` protos, served by agentd through `connectrpc` (proposed)** | connect-query and TanStack Query as the ruling says; one wire for web and desktop; the §44 "one Connect API" direction; Connect's server streaming fits `Watch*`; Swift and Kotlin clients become possible | A second schema next to ts-rs for the methods Studio uses; a mapping layer; the work of keeping two contracts equal |
| B. Keep ndjson; write a typed ndjson client and use plain TanStack Query | No new schema | Breaks the ruling's connect-query half; a browser WebSocket can't send `Authorization`, so the remote path needs another auth scheme anyway |
| C. Generate protos from the ts-rs types | One source | ts-rs types are internally tagged serde enums; mapping them mechanically to proto `oneof`s is fragile (§50 §8.1 rejected typeshare for the same reason) |

**Proposal: A.**
- The proto files go in `proto/loams/agentd/v1/{studio,threads,git,files,terminals,review,groups,approvals}.proto`.
- `loams-agentd-connect` implements them by calling the same `loams-agentd-sessions` functions as the ndjson dispatcher, so behaviour can't drift. A conformance test runs each Studio scenario through both wires against the Mock harness and compares the results (SD1 Task 6).
- The ndjson wire keeps everything it serves today.
- Only the methods Studio uses are exposed. That is about 60 of the 95, plus the new ones of §6.5.
- Q800 asks the owner to confirm A over B.

### 6.2 Desktop transport (D908)

```
renderer (Studio)  ──connect-web, fetch──►  loams-app://console/loams.agentd.v1.ThreadService/WatchThreads
Electron main protocol handler (route.ts): prefix /loams.agentd.v1.  → allowlist check (generated, renderer:true)
                                          → forward to 127.0.0.1:<connect-port>, add Authorization: Bearer <token>, drop Origin
loams-agentd-connect (loopback listener, same token, same Host check, no Origin)
```

- The local Connect listener has the same rules as the ndjson listener (§50 §7):
  - loopback only, on an ephemeral port;
  - the owner token is checked;
  - `Host` must be `127.0.0.1:<port>`;
  - any `Origin` gets a 403.
- `agentd.json` gains `connectPort`.
- Electron main is still the only client that holds the token. Server streaming passes through the protocol handler as a streamed response body.
- On the desktop, a page that uses the agentd transport and a page that uses the active Loams server's transport are both Connect, through two `transport` instances. Studio's `rpc.agentd.*` services use the agentd one.

### 6.3 Web transport: a remote agentd (D909, D921)

- **Process.** `loams-agentd serve --remote --listen 0.0.0.0:8443 --tls-cert … --tls-key … --issuer https://auth.<domain> --audience loams-agentd --subject <sub> --allow-origin https://console.<domain>`. Every flag is required; there are no defaults that open it up.
- **What it serves.**
  - It serves only `loams.agentd.v1` (Connect over HTTP/1.1 and HTTP/2), plus `/preview/…` (Q806).
  - It never serves the ndjson wire: the loopback ndjson listener stays loopback-only for the CLI and the MCP shim.
- **Auth.**
  - `Authorization: Bearer <access token>`, verified against the issuer's JWKS (Zitadel, §76 D1600; D404 is superseded). It checks `aud`, `exp`, `nbf` and `iss`, and requires `sub` to equal the enrolled subject.
  - The token maps to a role `studio`, which may call the Studio method set and nothing else. In particular, not `Shutdown`, the key and secret methods, or the account logins.
  - The console gets the token through its own Zitadel sign-in (IA1, formerly MT1), as an audience-specific access token (an RFC 8693 exchange or a second client).
- **CORS.** Only the configured origins, with `Access-Control-Allow-Headers` limited to the Connect headers and `Authorization`. No cookies, so there is no CSRF surface.
- **Finding the host.** Studio settings keep a list of agentd hosts per user, each with a name, a URL and a verified certificate fingerprint for self-signed setups. The list lives in the Loams server's user settings when one exists, and otherwise in local storage. A deep link names a host by id only (§4.4).
- **Where it runs.** A dev box, a VM, a container, or a §15 workspace sandbox. One remote agentd serves one person (D921). It is a per-user service, as on the desktop (D784).
- **Reachability.** An agentd behind NAT can join the tailnet (§43) instead of opening a port. An outbound tunnel through the Loams gateway is Q804.

### 6.4 The capability map (existing methods)

`U` is unary and `S` is a server stream. Names are the ndjson methods [P] (`crates/loams-agentd-proto/src/rpc.rs`). The Connect service column is the proposed `loams.agentd.v1` home.

| Studio capability | Existing agentd method(s) | `loams.agentd.v1` service |
|---|---|---|
| Daemon identity, version, features | `Hello` (ndjson only); `EngineInfo`, `LocalDevice` | `StudioService.GetHost` |
| Projects (list, add, rename, delete) | `WatchSpaces` S; `Mutate createSpace / renameSpace / deleteSpace` | `ProjectService` |
| Repositories (list, add, clone, create) | `ListRepos`, `AddRepo`, `CloneRepo`, `CreateRepo` | `ProjectService` |
| Folder picker (desktop) and drives | `ListFolders`, `ListDrives` | `FileService` |
| Threads (list, create, rename, archive, delete, pin, mark seen, config) | `WatchChats` S; `WatchSessions` S; `Mutate createChat / renameChat / setChatArchived / deleteChat / changeSidebarPin / markChatSeen / setChatConfig / setChatBranch / setChatCwd`; `WatchSidebarPreferences` S | `ThreadService` |
| Fork / side thread | `ForkSideChat` | `ThreadService.ForkThread` |
| Send a turn, steer, interrupt, answer a question | `QueueCommand {run, steer, interrupt, respondInput}` | `ThreadService.Send / Steer / Interrupt / AnswerQuestion` |
| Queue while busy | `WatchQueue` S; `QueueMessage`, `UpdateQueuedMessage`, `Begin/Renew/FinishQueuedMessageEdit`, `MoveQueuedMessage`, `RemoveQueuedMessage`, `SendQueuedMessageNow`, `SteerQueuedMessageNow`, `RetryDelivery` | `QueueService` |
| Transcript stream | `WatchDocMessages` S (reset, then deltas) | `ThreadService.WatchTranscript` |
| Harnesses, models, skills, slash commands | `ListHarnesses`, `ListModels`, `ListSkills`, `ListCommands`, `SetHarnessEnabled`, `InstallHarness`, `CancelInstall` | `HarnessService` |
| Harness updates | `WatchHarnessUpdates` S; `Check/Apply/Cancel/DismissHarnessUpdate` | `HarnessService` |
| Agent accounts and quota windows | `ListAgentAccounts`, `Activate/ForgetAgentAccount`, `Start/Complete/Poll/CancelAgentLogin` | `HarnessService` (desktop only for logins, Q808) |
| Branches, refs, history, switch, fetch | `ListBranches`, `ListRefs`, `ListGitHistory`, `SearchGitHistory`, `ResolveGitAvatars`, `SwitchRef`, `FetchAll` | `GitService` |
| Worktree create and delete | `CreateWorktree`, `DeleteWorktree`; `RunRequest.worktree` | `GitService` |
| Git status | `WatchWorkspaceGitStatus` S | `GitService.WatchStatus` |
| Diffs (working tree, branch, commit, turn) | `WatchCheckoutDiffs` S, `GetCheckoutDiff`, `GetCheckoutFileDiffText` | `DiffService` |
| Discard all | `DiscardWorkingTree` | `DiffService.Discard` |
| PR status (GitHub, read-only) | `WatchCheckoutChangeRequest` S | `GitService.WatchChangeRequest` |
| File tree, read, write, move, delete, watch | `ListWorkspaceDirectory`, `ReadWorkspaceFile`, `ReadWorkspaceImage`, `WriteWorkspaceFile`, `MoveWorkspaceEntry`, `DeleteWorkspaceEntry`, `WatchWorkspaceFiles` S | `FileService` |
| File search, @-mentions | `SearchWorkspaceFiles`, `SearchFiles` | `FileService.Search` |
| Terminals | `OpenTerminal`, `SubscribeTerminal` S, `WriteTerminal`, `ResizeTerminal`, `CloseTerminal` | `TerminalService` |
| Project actions (setup, dev, test) | `ListProjectActions`, `UpsertProjectAction`, `DeleteProjectAction`, `RunProjectAction`, `TakeProjectActionSetup` | `ProjectService` |
| Preview | `WatchPreviews` S | `PreviewService.WatchPreviews` |
| Attachments | `UploadChunk`, `UploadCommit`, `ReadAttachmentChunk`, `FetchToolBlob` | `AttachmentService` (Connect client streaming isn't available in browsers, so uploads stay chunked unary calls) |
| Title settings | `GetTitleSettings`, `SetTitleSettings` | `StudioService` |

### 6.5 Gaps: new RPCs (D910)

Each one is added to the ndjson method table (with role and `renderer`) and to `loams.agentd.v1`. Shapes are given in proto style; field names follow the existing wire's camelCase in JSON.

| # | New RPC | Kind | Request → response (key fields) | Why |
|---|---|---|---|---|
| G1 | `ListWorktrees` | U | `{repoPath}` → `{worktrees: [{path, branch, head, checkoutId?, chatIds[], dirty, ahead, behind, lastActivityMs}]}` | Clean-up and the worktree picker. Only `ListRefs.worktreePath` exists today |
| G2 | `CommitWorktree` | U | `{checkoutId, cwd, message, paths?[], all: bool, amend: bool, signoff: bool, expectedChecksum}` → `{sha, summary}` | Commit from Studio's git toolbar. Refuses when the checksum is stale (`stale_diff`) |
| G3 | `PushBranch` | U | `{cwd, remote?: "origin", branch, setUpstream: bool, forceWithLease: bool}` → `{remote, ref, url?}` | Push for a PR. Never a plain `--force` |
| G4 | `CreateChangeRequest` | U | `{cwd, provider: "forgejo"\|"github", base, head, title, body, draft: bool, linkIssue?}` → `{provider, number, url, state}` | Opens a PR. Forgejo goes through the factory credential in the daemon's keyring (§50 §12, D795); GitHub through `gh` as today |
| G5 | `MergeBranch` | U | `{repoPath, source, target, strategy: "merge"\|"squash"\|"rebase"\|"ff", message?, deleteSource: bool, removeWorktree: bool, expectedSourceHead}` → `{sha, conflicts?: [path]}` | Merge a run group's winner, or any thread, into the base. On conflicts it stops, leaves the tree clean, and reports the paths |
| G6 | `ListCheckpoints` | U | `{chatId}` → `{checkpoints: [{turnId, ref, createdAtMs, filesChanged}]}` | Per-turn restore points. At each turn's start and end, the daemon writes a hidden commit `refs/loams/checkpoints/<chatId>/turn/<n>/{pre,post}` through a temporary index (`GIT_INDEX_FILE`, `write-tree`, `commit-tree -p HEAD`), never touching the user's index or HEAD. This is T3 Code's scheme (`apps/server/src/checkpointing`), reimplemented in Rust |
| G7 | `RestoreCheckpoint` | U | `{chatId, turnId, expectedChecksum, scope: "worktree"\|"worktree+transcript"}` → `{restoredTo}` | Undo an agent's turn. `worktree+transcript` also forks the thread at that turn (it never deletes history) |
| G8 | `WatchReviewComments` | S | `{checkoutId}` → `{comments: [ReviewComment]}` (reset, then upserts and deletes) | Review comments (§5.6) |
| G9 | `UpsertReviewComment` / `DeleteReviewComment` / `ResolveReviewComment` | U | `ReviewComment {id, checkoutId, path, side, line, endLine?, body, diffChecksum, createdAtMs, resolved}` → `IdReply` | Stored in the daemon's SQLite store, keyed by checkout |
| G10 | `SendReviewToThread` | U | `{chatId, commentIds[]}` → `{commandId}` | Queues one turn built from a fixed template; comment bodies are quoted as data |
| G11 | `PostReviewToChangeRequest` | U | `{checkoutId, commentIds[], event: "comment"\|"request_changes"}` → `{url}` | Forgejo and GitHub review APIs |
| G12 | `CreateRunGroup` | U | `{spaceId, repoPath, base, prompt, attachments[], variants: [{harness, model?, reasoning?, modelOptions?}], checkActionId?, approvalMode}` → `{groupId, chatIds[]}` | Parallel runs (§5.5) |
| G13 | `WatchRunGroups` | S | `{spaceId?}` → `{groups: [RunGroup {id, prompt, base, createdAtMs, state: running\|ready\|resolved, members: [{chatId, harness, model, status, filesChanged, additions, deletions, elapsedMs, tokens?, check?: {state, exitCode?}}], resolution?}]}` | The group grid and the sidebar |
| G14 | `GetRunGroupComparison` | U | `{groupId, a: chatId, b?: chatId, path?}` → `{matrix: [{path, perMember: [{chatId, additions, deletions}]}], pairDiff?: CheckoutDiff}` | The diff matrix and the side-by-side view |
| G15 | `RunGroupCheck` | U | `{groupId, chatId?}` → `{runs: [{chatId, terminalId}]}` | Runs the check action in each worktree; the results land in `WatchRunGroups` |
| G16 | `ResolveRunGroup` | U | `{groupId, winnerChatId, action: "merge"\|"pr"\|"keep", merge?: MergeBranch params, pr?: CreateChangeRequest params, archiveOthers: bool, removeOtherWorktrees: bool}` → `{result}` | Pick and merge |
| G17 | `WatchApprovals` | S | `{chatId?}` → `{pending: [Approval {id, chatId, turnId, tool, args (redacted), risk: read\|write\|destructive, createdAtMs}]}` | The inbox and badges. `ApprovalRequested` / `ResolveApproval` come from DD1 Task 24, extended to the harness drivers (§8.1) |
| G18 | `ResolveApproval` | U | DD1's contract: `{callId, decision: once\|always\|deny}` (plus `chatId`) | Role `owner` and `studio` only; never `mcp` |
| G19 | `WatchNotifications` | S | `{}` → `Notification {id, kind: turn_done\|approval\|question\|error\|group_ready\|check_failed, chatId?, groupId?, title, createdAtMs}` | In-app notifications on the web. On the desktop, they also drive OS notifications |
| G20 | `GetThreadUsage` | U | `{chatId}` → `{turns: [{turnId, inputTokens?, outputTokens?, cacheTokens?, costUsd?, durationMs}], totals}` | Per-session usage. The `usage` events are persisted from now on. No billing meaning (CONTRIBUTING open-core rule; the name avoids `meter`) |
| G21 | `DiscardPaths` | U | `{checkoutId, paths[], expectedChecksum}` → `{}` | Discard one file |
| G22 | `SetThreadApprovalMode` | U | `{chatId, mode: "full-access"\|"supervised"}` → `{}` | §8.1. Could be a `Mutate setChatConfig` field instead (SD1 Task 0 decides) |
| G23 | `SearchWorkspaceContent` | S | `{spaceId?, chatId?, query, regex: bool, caseSensitive: bool, globs?[], maxResults}` → `{path, line, column, preview}` items | Content search for the palette and the search dialog. `SearchWorkspaceFiles` matches names only. Built on the `ignore` crate the daemon already uses, with a result cap |

`WatchSession` (DD1 §14.3), `ApprovalRequested` and `ResolveApproval` (DD1 Task 24) are DD1's. SD1 consumes them and doesn't redefine them.

### 6.6 What Studio does not need from agentd

- Engine and stack supervision (DD1c): Console uses them.
- SQL and factory read ops (DD1 Tasks 21 and 23): Console and the factory panels use them.
- The native loop (DD1d): it is one more harness in Studio's picker (`loams-agent`).

---

## 7. Web and desktop capability matrix

| Capability | Desktop (local agentd) | Web (remote agentd) | Notes |
|---|---|---|---|
| Mode switcher, shared shell | Yes | Yes | |
| Add project | Folder picker + `AddRepo`; also clone | Clone only (`CloneRepo`), or a path on the host typed in | |
| Harnesses | Every installed CLI on the machine; install from Settings | Every CLI installed on the host; install from Settings (runs on the host) | Accounts and logins: §6.4, Q808 |
| Threads, transcript, queue, steer | Yes | Yes | |
| Worktree per thread | Yes | Yes | |
| Diff review, comments | Yes | Yes | |
| Commit, push, PR, merge | Yes | Yes | Credentials stay on the daemon's host |
| Parallel runs | Yes (bounded by the machine) | Yes (bounded by the host) | |
| Terminals | Yes | Yes (over Connect streaming) | Latency is the network's |
| Preview | `WebContentsView` on `*.localhost:7331` | Through the remote listener's `/preview/` (Q806); until then port-forward instructions | |
| Open in editor | Yes (`openExternal` with an editor URL scheme, Q810) | No | |
| Factory apps | All eight, embedded views (D678) | Native panels and frames once SF1 lands (D922) | |
| OS notifications | Yes, from the daemon | Browser notifications only with permission, from `WatchNotifications` | |
| Background turns | Yes, with consent (D785) | Yes (the host's daemon is a service) | |
| Durable turns | Native loop (DD1 Task 25); harnesses resume (§50 §11.5) | Same | |
| Without an agentd | Studio shows "the daemon is not running" with Start | Studio shows "connect an agentd host" with the setup guide | |

---

## 8. Security

### 8.1 Agent approvals (D911)

- **Today.** Every harness driver auto-approves tool calls [P]:
  - Claude Code: `can_use_tool` always allows.
  - Codex: `approval_policy = "never"`, with the sandbox forced to `danger-full-access`.
  - opencode: answers "once".
  - ACP: auto-accepts.
  
  Only genuine questions reach the person.
- **Studio's approval modes:**
  - `full-access` keeps today's behaviour and labels it plainly in the thread header.
  - `supervised` makes each driver ask instead:
    - Claude: `can_use_tool` becomes a durable `ApprovalRequested`.
    - Codex: an approval policy of `on-request`, with the sandbox at the thread's level.
    - opencode: forwards its permission event.
    - ACP: forwards `session/request_permission`.
  
  The answer comes only from `ResolveApproval` by an `owner` (desktop) or `studio` (remote) client, never from the `mcp` role (DD1 Ruling 8).
- **Default.** `supervised` for the web (a remote daemon is reachable over the network), and the person's choice on the desktop, with `supervised` pre-selected (Q803). The daemon's own default, for a thread nobody chose a mode for, is `supervised` locally too (owner, 2026-10-15; SD1 T30-19): full access is always asked for.
- **Run groups** use one approval mode for all members. "Approve for all variants" answers the same tool and arguments in each member, as one click with one record per member.
- **Destructive git actions need an explicit confirmation**, whatever the mode: discard, restore, merge, push and delete worktree. They are never offered to the agent through the MCP shim.

### 8.2 Sandboxing and confinement

- **The sandbox level** (`read-only`, `workspace-write`, `danger-full-access`) is passed to the harnesses that support it.
  - Codex's forced full access is lifted in `supervised` mode (SD1 Task 13).
  - The mode a harness actually runs in is shown in the thread header.
  - Where a harness can't honour a level, Studio says so instead of pretending.
- **Worktrees** keep an agent's edits apart from the main checkout and from other agents. They do not contain processes: an agent with a shell can still read the whole home directory. §50 §7 records this residual risk, and Studio repeats it in its guide.
- **The remote agentd** should run in an isolated environment: a VM, a container with only the project directories mounted, or a §15 sandbox runtime (microsandbox by default). The SD1 guide ships a container recipe. OS-level confinement inside the daemon (landlock, seatbelt, bubblewrap) is Q815.
- **The remote listener's surface:**
  - It serves only the `studio` method set. Methods that reveal or set secrets are excluded, and so are `Shutdown`, logins and the harness installers (installers run with a confirmation from the desktop only; on a remote host, Q808).
  - A fuzz target covers the Connect decoder (SD1 Task 30).
  - Rate limits match §50 §7.
- **Credentials** for Forgejo and GitHub stay in the daemon's keyring (D794). Studio never receives them. `CreateChangeRequest` and `PostReviewToChangeRequest` use them on the daemon.

### 8.3 Plugin trust tiers (D918)

- §37 §5.6's tiers stand:
  - `core` and `first-party` run in the console realm behind the guard proxy;
  - `third-party` runs in a sandboxed opaque-origin frame behind the bridge, with vended tokens.
- **Studio's rules:**
  - `rpc.agentd.*` and the agentd `transport` are provided only to `core` and `first-party` plugins. The bridge has no agentd permission, so a third-party frame can't call agentd at all.
  - Vendored code in `@loams/studio-ui` is a library, not a plugin. It runs inside first-party plugins and has their reach, so it gets the same review as our own code (§9.1).
  - Studio adds two permissions to `permissions.ts`: `agentd:use` (threads, files, terminals) and `agentd:git-write` (commit, push, merge, PR). The guard checks them for first-party plugins too, as hygiene.
- **Safe Mode** (DD1 Task 32) loads Studio, which is first-party.

### 8.4 Rendering untrusted content (D919)

Model output, tool output, file contents, diffs and terminal output are untrusted.
- **Markdown.** The existing safe renderer: no raw HTML, no images, http(s) links only, DOMPurify as a second fence. Vendored chat markdown must keep these rules; T3 Code's renderer is adapted to them, never the other way round.
- **Syntax highlighting.** Shiki runs on escaped text, and its token output renders as React spans; or inside `@pierre/diffs`' shadow root, where every token came from escaped text. Each renderer gets the fixture test `diff_renders_text_not_html` (a hunk containing `<img src=x onerror=…>` renders literally) and `code_block_renders_text_not_html`. Whether `@pierre/diffs` renders through `innerHTML` is checked in SD1 Task 0 (verify). If it does, its HTML must be produced only from Shiki's escaped token stream, or the diff panel falls back to React rendering.
- **Terminal output** goes to xterm.js only. **File previews** are text. Images are shown only for local files the daemon serves (`ReadWorkspaceImage`), as blob URLs.

---

## 9. Vendoring, licence and attribution

### 9.1 The package and the import tool (D912)

```
web/packages/studio-ui/
  package.json            "license": "Apache-2.0 AND MIT"   (as loams-agentd mixes MIT and Apache-2.0)
  LICENSE                 Apache-2.0 (the repository's)
  LICENSE.t3code          T3 Code's MIT licence, verbatim (§16)
  NOTICE                  what is from T3 Code, at which commit
  provenance.json         {upstream, commit, archiveSha256, files: [{path, upstreamPath, upstreamSha256, localSha256, verdict, group}]}
  src/<group>/…           thread-list, chat, markdown, composer, diff, file-tree, terminal, git-toolbar, palette, keybindings, ui
```

- **The import tool** is `scripts/studio/vendor.mjs`. It never executes anything from the upstream tree.
  1. It takes a source archive whose SHA-256 is recorded, and a commit id.
  2. It extracts the archive into a scratch directory.
  3. It copies the named files.
  4. It writes the header (§9.2).
  5. It records both hashes in `provenance.json`.
  
  `--check` mode verifies that every vendored file has a header and a provenance row. It also checks that an `as-is` file's body still hashes to its recorded `localSha256`.
- **Verdicts** (inventory): `vendor as-is` (header only, plus import-path rewrites), `vendor and adapt` (port the state layer to connect-query and the agentd store, styles to tokens, primitives to `@loams/ui`), `rewrite` (ours, informed by upstream, no header needed but credited in NOTICE when it follows the upstream's structure closely), `skip`.
- **The port rules:**
  - No import from `effect`, `@effect/*`, `@effect-atom/*`, T3's `contracts`, `client-runtime`, or its RPC or WebSocket clients. A test (`no_effect_imports`) scans `web/packages/studio-ui` and `web/plugins/studio`.
  - Colours, radii and shadows come from tokens only (the `colours.test.ts` scan covers the package).
  - Primitives come from `@loams/ui` (Base UI wrapped once); no direct `@base-ui/react` import.
  - Server state goes through connect-query hooks on `rpc.agentd.*`. T3 Code's `useEnvironmentQuery` already has TanStack Query's shape (`{data, error, isPending, refresh}`), so it is the seam. Client-only state stays in per-group stores (zustand, Q819), never a global atom graph; `effect/Schema` persistence decoding becomes zod.
  - Virtualised lists use TanStack Virtual through `@loams/ui`'s `VirtualList`, not `@legendapp/list` (ruling 5).
  - T3 Code's `components/ui` primitives are merged into `@loams/ui` only where `@loams/ui` lacks one (ruling 2: Base UI wrapped once). They never live in `@loams/studio-ui`.
  - Routing is the shell's hash router; TanStack Router file routes are rewritten as `router.page` registrations.
  - `@pierre/diffs`' find-in-diff calls depend on T3 Code's local patch (`patches/@pierre%2Fdiffs@1.5.2.patch`). We drop them at first and add find-in-diff later, either upstream or as our own patch with its own NOTICE line (Task 20).
  - Third-party dependencies are added to the pnpm catalog, pinned, and at least 14 days old (console-ui-stack ruling 7).

### 9.2 Licence and attribution (D913)

T3 Code is MIT, "Copyright (c) 2026 T3 Tools Inc." (§16). MIT lets us copy, modify and relicense combined works, provided the copyright notice and the permission notice are kept in copies or substantial portions.

**The file header** of every vendored or adapted file:

```ts
// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// <upstream path> at commit <sha>, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// <one line: what changed>. Loams' changes are Apache-2.0.
```

**The root NOTICE entry** (CONTRIBUTING rule 4):

```
T3 Code (https://github.com/pingdotgg/t3code), MIT License, Copyright (c)
2026 T3 Tools Inc. Files under web/packages/studio-ui/ marked in their
headers are derived from T3 Code at the commit recorded in
web/packages/studio-ui/provenance.json; the MIT licence text is in
web/packages/studio-ui/LICENSE.t3code and must accompany distributions of
those files.
```

- **The desktop.** The desktop bundle's NOTICE (`apps/desktop-electron/NOTICE`) gains the same entry, and the web console's built `/ui/NOTICE` lists it.
- **PR descriptions.** Every PR that vendors files says so and lists the files (rule 4).

### 9.3 Third-party dependencies (D914)

- **Licences.** The inventory lists the npm packages the vendored code pulls in and their licences. The rule is CONTRIBUTING rule 3: Apache-2.0, MIT, BSD, ISC, Zlib, Unicode, MPL-2.0 (unmodified); no AGPL, GPL, LGPL, BUSL, SSPL, ELv2 or proprietary code.
- **The gate.** `scripts/ci/npm-licences.mjs` reads `pnpm licenses list --prod --json` for the console and desktop projects. It fails on any licence outside the allow list or on `UNKNOWN`, and takes an explicit exception file with reasons. It runs in `monorepo.yml`.
- **Licences checked on the npm registry (2026-10-13, `npm view <pkg> license`)** [P]:
  - `@pierre/diffs` 1.5.2, `@pierre/trees`, `@pierre/theming`: **Apache-2.0**. Allowed. They are dependencies, not copied source; if a package ships a NOTICE file, it goes into the bundle's third-party notices.
  - `@tiptap/core`, `@tiptap/pm`, `@tiptap/react`, `@tiptap/starter-kit`: MIT. Tiptap Pro and Cloud extensions are proprietary and must never be added; the gate refuses them by licence.
  - MIT: `@legendapp/list`, `@base-ui/react`, `zustand`, `shiki`, `@dnd-kit/*`, `@xterm/xterm`, `preact` (pulled in by `@pierre/trees`), `react-markdown`, `remark-gfm`, `unified`, `rehype-sanitize`, `hast-util-to-html`, `mermaid`, `culori`, `morphicons`.
  - Other allowed licences: `lucide-react` (ISC), `class-variance-authority` (Apache-2.0), `diff` (BSD-3-Clause), `dompurify` (MPL-2.0 OR Apache-2.0; taken under Apache-2.0).
  - **`heic-to`: LGPL-3.0. Not allowed** (rule 3). T3 Code's HEIC conversion in `lib/imageCompression.ts` is dropped: Studio accepts PNG, JPEG, GIF and WebP attachments, and a HEIC file is refused with a message.
  - **`jszip`: MIT OR GPL-3.0-or-later.** Taken under MIT if it is needed at all (T3 Code uses it for zip downloads, which Studio does not vendor). The gate accepts an `OR` expression when one branch is allowed, and records the choice.
- **`@pierre/trees` is a beta** (1.0.0-beta.6) that bundles a Preact 11 beta. The file tree keeps a fallback: a TanStack Virtual tree over `ListWorkspaceDirectory` (inventory group e).

### 9.4 Upstream drift and the re-sync policy (D915)

- **A fork, not a mirror.** After the port, vendored files are ours, and upstream is a source of ideas and fixes.
- **Watch.** `scripts/studio/upstream-report.mjs` compares the recorded commit with a newer archive, per vendored file. It reports `unchanged`, `changed upstream` (with the upstream diff) and `gone upstream`. It runs on demand. A monthly reminder issue is opened by a scheduled workflow that downloads nothing and only notes that a check is due.
- **Re-sync.** One component group per PR:
  1. Take the upstream diff between the two commits for that group's files.
  2. Apply it by hand to our port.
  3. Run the group's tests.
  4. Update `provenance.json` (the new commit for the touched files only; a group can sit at a different commit from the rest).
- **Security fixes upstream** that touch vendored code (an XSS in markdown, say) are applied within 7 days of becoming known.
- **Licence change upstream.** A file stays at the last MIT commit unless the new licence is on the allow list. Code already vendored under MIT stays MIT.

---

## 10. Testing

- **Unit (Vitest).**
  - Each component group has its own tests, named in SD1.
  - The mode service and manifest validator.
  - The connect transports.
  - The reducer fixtures, shared with `@loams/agentd-client`.
  - `no_effect_imports`, the colour scan, and `vendored_headers_present`.
- **Rust.**
  - Each new RPC has tests in `loams-agentd-sessions` against a temporary git repository.
  - Connect parity: the same scenario through ndjson and through Connect gives the same result.
  - Remote-listener security: wrong audience, wrong subject, a disallowed origin, an `mcp` token.
  - Fuzzing the Connect decoder.
- **End to end (Playwright).** A test fixture starts `loams-agentd run --child` with `HOME=<tmp>` and `LOAMS_DESKTOP_HARNESS=mock` (the existing pattern in `crates/loams-agentd/tests/support/mod.rs`). Two variants:
  - **Web:** the console against the remote listener on loopback with a test issuer (a local JWKS).
  - **Electron:** `_electron.launch` with the bundled daemon.
  
  The Mock harness's knobs drive questions, errors and long runs. A new knob makes it write files, so diffs exist (`LOAMS_DESKTOP_MOCK_EDIT=<path>`, SD1 Task 8). Scenarios: create a project from a fixture repository; run a thread; see the diff; comment; send the review to the agent; commit; a run group of 3 with compare and merge; a terminal round trip; an approval in supervised mode; mode switching with live badges.

---

## 11. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | **Upstream drift.** T3 Code is at v0.0.x and moves fast; our port diverges within weeks | High | Medium | Treat it as a fork (D915); port by component group; `upstream-report.mjs`; take ideas, not churn |
| 2 | **The port is a rewrite in practice.** T3's web app is built on Effect atoms and its own contracts; little survives unchanged | High | Medium | The inventory marks most files `vendor and adapt` or `rewrite`, with honest sizes; leaf presentational components first |
| 3 | **Two contracts for agentd** (ts-rs ndjson and proto Connect) drift apart | Medium | Medium | Handlers call the same functions; a parity test per Studio method; Studio reads only Connect |
| 4 | **A remote agentd exposes code execution to the network** | Medium | High | Off by default; TLS; one subject; audience-bound tokens; origin allowlist; method allowlist; supervised mode by default; isolation recipe; security review (SD1 Task 30) |
| 5 | **Third-party licences.** One dependency is already out (`heic-to`, LGPL-3.0); `@pierre/trees` is a beta; a dependency's licence may change | Medium | Medium | The npm gate (D914); the HEIC path dropped; `@pierre/*` checked as Apache-2.0; fallbacks on Shiki and TanStack Virtual |
| 6 | **XSS through the diff, markdown or highlighting renderers** | Medium | High | D919; fixture tests; DOMPurify; shadow root; review |
| 7 | **DD1 and SD1 build the same UI twice** (DSH port vs T3 port) | High if unmanaged | Medium | D917; Q802; SD1 Task 0 agrees the split with DD1's owner |
| 8 | **Harness approval hooks differ per CLI and change with CLI releases** | Medium | Medium | Per-driver tests with the fake CLIs; the thread header always shows the effective mode; `full-access` stays available |
| 9 | **Parallel runs exhaust the machine** (N harness processes, N `node_modules`) | Medium | Low | Concurrency limits; setup actions can share a package store; the group dialog estimates disk use (estimate) |
| 10 | **Name confusion** between "Loams Studio" and "Data Studio" | Medium | Low | Q812 |
| 11 | **Base UI, the pnpm catalog and connect-query aren't on `dev` yet** | Low | Medium | SD1 Task 0 waits for or rebases onto `console/ui-stack` |
| 12 | **Connect streaming through Electron's protocol handler** buffers or breaks backpressure | Low | Medium | A Task 5 test streams 10 000 terminal frames through the proxy and checks the latency bound |

---

## 12. Rollout

1. **SD1a.** The foundations: vendoring, the licence gate, modes, the Connect surface, transports, remote access. Nothing is visible except an empty Studio with a project list.
2. **SD1b.** The daemon gaps (G1–G22), behind the daemon's feature list, so a client can ask for them in `GetHost.features`.
3. **SD1c.** The component groups: Studio usable on the desktop.
4. **SD1d.** The factory, the desktop integration and the web path.
5. **SD1e.** End to end, security review, docs, exit.

Studio ships behind `flags.features["studio"]` in the browser edition until SD1e, and is on by default in the desktop edition from SD1c.

---

## 13. What this does not change

- The ndjson wire, the roles and the renderer allowlist of §50 §7–§8.
- The trust tiers of §37 §5.6.
- The factory's open-core split (D-SF-16).
- The engine's Connect API (§44). Studio's `loams.agentd.v1` is the daemon's, is not served by `loams`, and is not in the SDK catalogue (Q816).

---

## 14. Glossary

| Term | Meaning |
|---|---|
| **Mode** | Console or Studio: a view of the one console that selects nav, pages and palette (D901) |
| **Studio project** | An agentd space: a folder (normally a git repository) on the daemon's host |
| **Thread** | One agentd chat with its harness, model, worktree and transcript |
| **Run group** | N threads started from one prompt, compared and resolved together (D904) |
| **Checkpoint** | A ref written at a turn's start, restorable (G6, G7) |
| **Remote agentd** | `loams-agentd serve --remote`: one person's daemon reachable from the browser console (D909) |

---

## 15. Open questions

| # | Question | Default if unanswered | Needed by |
|---|---|---|---|
| Q800 | Connect facade `loams.agentd.v1` (option A) or a typed ndjson client with plain TanStack Query (option B)? | A | SD1 Task 4 |
| Q801 | Desktop default mode: Studio from the first run, or Console until a project is added? | Studio from the first run | Task 3 |
| Q802 | DD1e (Tasks 28–31) adapts the DSH client UI for the chat, sessions, terminal and diffs; SD1 vendors T3 Code for the same surfaces. Which wins per component? | T3 Code's port for the thread list, chat timeline, composer, diff panel, file tree, terminal and git toolbar; DD1e keeps the session store and reducer, `ApprovalCard`, `QuestionCard`, `SubagentCard`, `TurnOutline`, Settings › Plugins and Safe Mode. DD1 Tasks 29–31 are narrowed accordingly | Task 0 |
| Q803 | Default approval mode on the desktop | `supervised`, pre-selected, changeable per thread | Task 13 |
| Q804 | Reaching a remote agentd behind NAT: tailnet only (§43), or an outbound tunnel through the Loams gateway? | Tailnet or a direct TLS port in SD1; a tunnel is a later design | Task 7 |
| Q805 | A hosted, multi-user agentd (Studio in Loams Cloud)? | Out of scope; a cloud product decision (open-core review) | — |
| Q806 | Serve previews through the remote listener (`/preview/…`)? | Yes, behind the same token, with response headers that forbid framing by other origins | Task 26 |
| Q807 | Store the Loams project binding in agentd space metadata or in the Loams server? | agentd space metadata | Task 15 |
| Q808 | Harness installs and account logins on a remote host from the browser? | Not in SD1; the remote guide uses the host's shell | Task 7 |
| Q809 | Rename the worktree branch prefix `loams-desktop/` to `loams/`, and `~/.loams-desktop/worktrees` to the daemon data directory? | Keep both in SD1 (existing users); revisit with the device-sync design | Task 9 |
| Q810 | "Open in editor" on the desktop: which editors, through URL schemes (`vscode://`, `zed://`, `cursor://`) or a configured command? | URL schemes for VS Code, Zed and Cursor plus a configured command | Task 28 |
| Q811 | Offer a "judge" agent in the compare view? | Yes, as an ordinary read-only thread started on demand; no automatic picking | Task 25 |
| Q812 | Rename the desktop's **Data Studio** page to avoid confusion with Loams Studio? | Rename it "Data explorer" in SD1 Task 28 | Task 28 |
| Q813 | Matomo is a factory app in the desktop plugin but not in the owner's list: keep it under Studio? | Keep, under Studio's Factory group | Task 27 |
| Q814 | Should the factory loop's coding stage (`propose_patch`, D-SF-18) run as an agentd thread that Studio shows? | Yes when the run is local or on a remote agentd; SF4 decides for hosted runs | SF4 |
| Q815 | OS-level confinement in the daemon (landlock, seatbelt, bubblewrap) for `workspace-write` runs? | Not in SD1; a separate design | — |
| Q816 | Should `loams.agentd.v1` be in the SDK catalogue and the 13-language generation? | No; TypeScript only, generated for the console | Task 4 |
| Q817 | Vendor from `main` at `6524a8d4` (2026-10-13), or from the tagged release v0.0.45? | `main` at `6524a8d4`, the archive inventoried here; Task 1 confirms the id against the repository | Task 1 |
| Q818 | Terminal: keep xterm.js (DD1 Task 31's pin), or vendor T3 Code's libghostty-vt wasm terminal (MIT, a 631 KB wasm binary built with zig 0.15.2)? | xterm.js. Vendoring a prebuilt wasm needs a reproducible build of it in our CI first; revisit after SD1 | Task 22 |
| Q819 | T3 Code keeps client-only UI state in zustand 5 (MIT) stores. Allow zustand in `@loams/studio-ui`, or port each store to `useSyncExternalStore`? | Allow zustand, through the pnpm catalog, for client-only UI state in `@loams/studio-ui` and `@loams/plugin-studio` only; server state stays in connect-query | Task 1 |

---

## 16. Provenance of the source

| | |
|---|---|
| Repository | `pingdotgg/t3code` (GitHub) |
| Branch | `main`, pushed 2026-10-13 |
| Snapshot | The GitHub zip archive of `main` (top directory `t3code-main`), downloaded 2026-10-13 (`main` as of 2026-10-13T07:16:38Z), SHA-256 `b9ca31efad706d6828f3d9fb6b411027d5837901b800790d241eb672a74b7cbd`, unzipped read-only into a scratch directory outside this repository |
| Commit | **`6524a8d43795dc543f7cfb572388cdea3662ad48`**, from the archive's zip comment (`unzip -z`; GitHub writes the commit id there). The tree itself has no `.git`, CHANGELOG or build-info file. The other 40-hex strings in it are pinned action SHAs, the GhosttyKit revision `cf8edc23…` (`third-party-licenses.config.json`) and libghostty-vt `9f62873b…` (`native/libghostty-vt/VERSION`), none of them the source commit |
| Versions | `apps/web` is `@t3tools/web` 0.0.45; `apps/server` is `t3` 0.0.45; the root `@t3tools/monorepo` has no version; `pnpm@11.10.0`, Node `^24.13.1` |
| Latest release | v0.0.45 |
| Licence | MIT, Copyright (c) 2026 T3 Tools Inc. (text below) |
| How it was read | Read only: no script, install or build was run from it |

The licence text, verbatim from the archive's `LICENSE`:

```
MIT License

Copyright (c) 2026 T3 Tools Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## 17. Contradictions with earlier decisions, and how they are resolved

| Earlier | What it says | This document | Resolution |
|---|---|---|---|
| D788 (§50 §7) | No browsers: any `Origin` is a 403; loopback only | The web edition reaches a remote agentd from the browser | Amended for a **separate** remote Connect listener only (D909), off by default, with OIDC and an origin allowlist. The loopback ndjson listener is unchanged |
| DD1 Ruling 2 | The RPC stays ndjson, not Connect | A Connect surface for Studio | Not overridden: ndjson stays for its clients; Connect is added beside it (D907). Q800 lets the owner choose B instead |
| D797, §50 §14, DD1 Tasks 28–31 | The chat, sessions, terminal and diff UI are adapted from the DSH client UI | T3 Code components for the same surfaces | **Conflict.** Recorded as Q802 with a split as the default; not silently overridden |
| D666 (§37 §19.10) | "Software Factory" is a product section in the console nav | The factory belongs to Studio | Amended by the owner's direction (D916) |
| SF1 (D-SF-19) | One plugin per app (`@loams/plugin-zulip`, `-plane`, `-forgejo`) in the browser console | The as-built desktop factory is one plugin with per-app panels | SF1's per-app plugins, when built, declare `modes: ["studio"]`; the desktop plugin stays one plugin. SF1's plan isn't changed here |
| §50 §2.1 and D675 | The agent panel is the right dock | Studio has its own thread view | The dock stays in both modes (D900); both read one store (D917) |
| AP1e page-plugin conventions | `rounded-none`/`rounded-pill`, `@loams/ui` first, no new UI framework | Vendored components bring their own look | The port applies the conventions; T3's look isn't kept |
| §37 §5.3 manifest | A closed set of keys | `modes` | Added (D901); `requires.console` bumps to the next minor version |

---

## 18. Sources

- The owner's direction of 2026-10-13 (§1.1).
- This repository on `dev` at `6c8481e8` [P]: `web/packages/console-host/src/{manifest,services,boot,tiers}.ts`, `web/packages/slots/src/types.ts`, `web/plugins/shell/src/layout.tsx`, `web/plugins/factory/**`, `web/apps/console/catalog/*.yml`, `crates/loams-agentd*/**` (notably `loams-agentd-proto/src/{rpc,agent}.rs`, `loams-agentd-sessions/src/{rpc,repos,diff_sync,terminals}.rs`, `loams-agentd-harness/src/mock.rs`, `loams-agentd/tests/support/mod.rs`), `web/packages/agentd-client/**`, `apps/desktop-electron/src/main/{protocol/route.ts,agentd/*}`, `CONTRIBUTING.md`, `NOTICE`, `deny.toml`.
- The branch `console/ui-stack` at `dbdf37ab` (`docs/design/console-ui-stack.md`).
- T3 Code `main` as of 2026-10-13T07:16:38Z [P], read-only (§16).
- [§37](37-desktop-and-mobile-apps.md), [§39](39-software-factory-and-loams-bot.md), [§44](44-unified-api-and-sdks.md), [§50](50-loams-desktop-daemon.md); plans [AP1a](../plans/2026-10-01-ap1a-cordis-console.md), [AP1e](../plans/2026-10-08-ap1e-electron-desktop.md), [DD1](../plans/2026-10-09-dd1-desktop-daemon.md), [SF1](../plans/2026-10-02-sf1-collab-ui-plugins.md)–[SF5](../plans/2026-10-02-sf5-observability-ui.md), [CL1](../plans/2026-10-10-cl1-loams-cli.md).
- Orca: the parallel-agents-with-compare idea, as the owner describes it (not read from source here).

## Appendix A: the vendoring inventory

The inventory is in [studio-vendor-inventory.md](studio-vendor-inventory.md).


> **See also (2026-10-17):** [§60 Places and Harness Composition](60-places-and-harness-composition.md). Studio already has much of what Endo offers (harnesses on the user's own subscriptions, run groups, forks); §60 proposes Places, composition rules, what an agent left behind, and durable agent workflows (D1087–D1093).


> **See also (2026-10-17):** [§61](61-loams-jailer.md) proposes the Loams Jailer over agentd harnesses: deterministic policy and sensors, an overseer that may only tighten, `ToolRisk::Egress` (D1098) and a circuit breaker (D1099).
