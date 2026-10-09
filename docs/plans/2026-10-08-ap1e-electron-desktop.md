# AP1e — Loams Desktop on Electron Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Work task by task, test first. Each task lists the interfaces it must produce and the tests that must exist and pass before it is done. Where this plan gives exact code, use it. Where it gives a contract and named tests, write the code to that contract, and record any deviation in "Rulings made during execution" at the end of this file.
>
> **Status: In progress** (2026-10-08). Part 2 (Tasks 18–31) added the same day for the full cloud console, the agent panel and Linux-only releases (§37 §19.10, D666–D676). This plan replaces AP1n as the shipping desktop (D652). AP1n is paused.
>
> **Names after the hard fork (D823; NF1 Task 1b).** This document predates the rename and keeps the old names: read `ostrium-labs/neon` as `ostrium-labs/loams-postgres`, `crates/loams-neon` (package `loams-neon`) as `crates/loams-postgres` (package `loams-postgres`), and `deploy/neon` as `deploy/loams-postgres-dev` (the compose project `loams-neon` and the desktop's copied `stacks/neon` keep their names).

**Goal:** Loams Desktop v0.1, an Electron app built with electron-vite. It runs the cordis console as a real cloud console against any Loams server. It also supervises a local `loams dev` engine with a Data Studio, and shows every Software Factory app as native read-only panels plus its full web UI. It is packaged for Linux, macOS and Windows.

**Architecture** (§37 §19.4):
- **The main process owns everything privileged.** That covers the `loams-app://` protocol (console files, API proxy to the active server, and the local-edition shim), the engine supervisor, the server registry, the factory host (adapters and encrypted credentials), the factory views, the tray, deep links and the updater.
- **The renderer is the existing cordis console** started with `@loams/platform-electron` and the `catalog/desktop.yml` patch. It is sandboxed, and its only bridge is a typed `window.loamsDesktop`.
- **Every main-process module has a pure core with no `electron` import,** tested with Vitest. A thin `*.electron.ts` adapter wires it to Electron.

**Tech Stack:**
- Electron (exact pin, D662), electron-vite 5, electron-builder 26, electron-updater 6.
- TypeScript 5.9.3, Node 26 (the workspace's), pnpm 11.27.1, Nx 23.2.1, Biome 2.5.14.
- Vitest 5.0.3 and Playwright (`_electron`).
- `@noble/ed25519` for update manifests.
- React 19.3.0 and `@connectrpc/connect-web` (the console's) for the plugins.
- `cordis@4.0.0-rc.10` via `@loams/cordis` (renderer) and `cordis` directly (main, as `plugins/` does).

**Spec:**
- [§37 §19](../design/37-desktop-and-mobile-apps.md) (all of it) and D652–D665, Q621–Q624 in the [decision log](../design/13-decision-log.md).
- Kept rules: §37 §6.2 (restart policy), §6.5 (keychain fallback), D432 (deep links navigate only), D489 (credentials), D490 (signed updates), D498 (no telemetry).
- §39 §3–§4 (factory apps and SSO).
- Reference shell: `~/Documents/Ostriumlabs/dsh-desktop` (MIT): `src/main/runtime/harness-runtime.ts`, `src/main/security.ts`, `src/main/security-policy.ts`, `src/main/desktop-protocol.ts`, `src/main/state/window-state.ts`, `src/main/state/desktop-storage.ts`, `src/main/close-to-tray.ts`, `src/main/update/*`, `electron.vite.config.ts`, `package.json` `build`, `scripts/jsign-windows-hook.mjs`, `.github/workflows/release.yml`.

## Global Constraints

- **Worktree and branch.** Work in `~/Documents/Ostriumlabs/loams-wt/ap1e-electron-desktop`, on branch `feat/ap1e-electron-desktop`, based on `dev`. Use `git commit -s` (DCO). Commit areas: `desktop`, `console`, `plugins`, `docs`, `ci`.
- **Rust builds** use the shared target directory (`~/Documents/.cargo/config.toml`). Never set `CARGO_TARGET_DIR`, never build in `/tmp`. The engine binary is `/mnt/Projects/rust-cache/target/{debug,release}/loams`.
- **Pins.**
  - Exact versions only. No `^` or `~`.
  - Each new npm dependency must be at least 14 days old (`npm view <pkg> time`). Record the chosen versions in the Task 1 commit message.
  - Use `pnpm` (never `npm install`) from the repo root.
- **Electron security defaults** (D659):
  - `contextIsolation: true`, `sandbox: true`, `nodeIntegration: false`, `webSecurity: true`, `webviewTag: false`;
  - no `remote` module;
  - every `ipcMain.handle` validates `event.senderFrame.url` starts with `loams-app://console/`.
- **No secret crosses IPC or reaches a log.** Credential values are typed `Secret` (a class whose `toJSON`/`toString` return `"[redacted]"`).
- **No telemetry** (D663). No request leaves the machine except:
  - to a server the user added;
  - to a factory app the user configured;
  - to the configured update feed.
- **Attribution.** Every file adapted from dsh-desktop starts with `// Adapted from dataelement/dsh-desktop (MIT), <path>.`, and `NOTICE` gains dsh-desktop's copyright line (Task 1).
- **Names.** `Loams Desktop`, `Loams Software Factory`, app id `dev.loams.desktop`, deep-link scheme `loams`, internal scheme `loams-app`.

## Review Focus

1. **A crafted path escapes the console directory.** For example: `loams-app://console/ui/..%2f..%2fetc/passwd`, `\\..\\` on Windows, or a NUL byte. Expected: 404, never a file outside `dist`. Test: Task 2 `rejects_traversal_variants`.
2. **The active server redirects to another origin, or sets a cookie for another domain.** Expected: the redirect is returned to the renderer and not followed by the proxy, and the cookie `Domain` is stripped. Tests: Task 2 `proxy_does_not_follow_cross_origin_redirect`, `strips_cookie_domain`.
3. **The engine binary is missing, its port is taken, or it crashes in a loop.** Expected: the state goes to `failed` with a readable reason after 5 restarts in 10 minutes. The UI offers "Open logs", and the app is still usable for remote servers. Tests: Task 5 `missing_binary_is_failed_not_thrown`, `crash_loop_stops_after_five`.
4. **A factory credential leaks into an IPC reply, a log line, or an error message.** Expected: never. Tests: Task 10 `ipc_replies_never_contain_secret`, `adapter_error_is_redacted`.
5. **A factory app's page navigates the factory view to another origin or opens a popup.** Expected: the navigation is blocked and the URL opens in the system browser if it is http(s). Test: Task 12 `factory_view_navigation_policy`.

---

## File structure

```
apps/desktop-electron/
  package.json  project.json  tsconfig.json  tsconfig.node.json  electron.vite.config.ts
  electron-builder.config.cjs  biome.json(extends root)  vitest.config.ts  NOTICE  README.md
  build/            icons (png/icns/ico), entitlements.mac.plist, installer.nsh
  resources/bin/    (packaging only: the loams engine binary, git-ignored)
  scripts/          fetch-engine.mjs, jsign-windows-hook.mjs, sign-manifest.mjs
  src/shared/       contracts.ts (IPC channel names + types), deeplink.ts
  src/preload/      index.ts (contextBridge: window.loamsDesktop)
  src/main/
    index.ts                 app lifecycle, wiring only
    paths.ts                 userData/logs/resources resolution
    protocol/                static.ts, proxy.ts, local-shim.ts, handler.electron.ts
    security/                policy.ts, install.electron.ts
    servers/                 registry.ts, ipc.electron.ts
    engine/                  ports.ts, binary.ts, supervisor.ts, log-rotate.ts, ipc.electron.ts
    factory/                 apps.ts, ops.ts, vault.ts, host.ts, ipc.electron.ts, views.electron.ts
    shell/                   tray.electron.ts, menu.electron.ts, window-state.ts, single-instance.electron.ts
    update/                  manifest.ts, pubkey.ts, updater.electron.ts
  test/             *.test.ts (vitest), fixtures/fake-engine.mjs, e2e/smoke.spec.ts
web/packages/platform-electron/   @loams/platform-electron
web/plugins/desktop-servers/      @loams/plugin-desktop-servers
web/plugins/data-studio/          @loams/plugin-data-studio
web/plugins/factory/              @loams/plugin-factory
web/apps/console/catalog/desktop.yml
web/apps/console/src/cordis/desktop.ts   (desktop entry: startConsole with platform-electron + patch)
.github/workflows/desktop-electron.yml
```

## Shared contracts (all tasks use these names)

`apps/desktop-electron/src/shared/contracts.ts`:

```ts
export type ServerKind = 'local' | 'remote' | 'demo';
export interface ServerEntry { id: string; name: string; kind: ServerKind; url: string } // url = origin
export type EngineState =
  | { phase: 'stopped' }
  | { phase: 'starting'; attempt: number }
  | { phase: 'ready'; url: string; esUrl: string; flightUrl: string; pid: number }
  | { phase: 'failed'; reason: string; logPath: string };
export type FactoryAppId =
  | 'forgejo' | 'zulip' | 'plane' | 'glitchtip' | 'openpanel' | 'matomo' | 'langfuse' | 'openobserve';
export type FactoryHealth = 'unconfigured' | 'ok' | 'auth_failed' | 'unreachable';
export interface FactoryAppInfo {
  id: FactoryAppId; label: string; url?: string; health: FactoryHealth;
  hasPanels: boolean; credentialFields: { key: string; label: string; secret: boolean }[];
  persistent: boolean; // false when safeStorage has no backend (session-only)
}
export interface FactoryQuery { app: FactoryAppId; op: string; params: Record<string, unknown> }
export type IpcResult<T> = { ok: true; value: T } | { ok: false; code: string; message: string };

export interface LoamsDesktopApi {
  version: string; platform: NodeJS.Platform;
  servers: {
    list(): Promise<{ servers: ServerEntry[]; activeId: string }>;
    add(e: Omit<ServerEntry, 'id'>): Promise<IpcResult<ServerEntry>>;
    remove(id: string): Promise<IpcResult<void>>;
    activate(id: string): Promise<IpcResult<void>>; // reloads the window
  };
  engine: {
    state(): Promise<EngineState>;
    start(): Promise<void>; stop(): Promise<void>; openLogs(): Promise<void>;
    onState(cb: (s: EngineState) => void): () => void;
  };
  factory: {
    list(): Promise<FactoryAppInfo[]>;
    configure(app: FactoryAppId, url: string, fields: Record<string, string>): Promise<IpcResult<FactoryAppInfo>>;
    test(app: FactoryAppId): Promise<FactoryAppInfo>;
    remove(app: FactoryAppId): Promise<void>;
    query<T = unknown>(q: FactoryQuery): Promise<IpcResult<T>>;
    openApp(app: FactoryAppId): Promise<IpcResult<void>>; closeApp(app: FactoryAppId): Promise<void>;
  };
  shell: {
    openExternal(url: string): Promise<IpcResult<void>>;
    notify(title: string, body?: string): Promise<void>;
    clipboardWrite(text: string): Promise<void>;
    onNavigate(cb: (path: string) => void): () => void; // deep links
    setBadge(count: number): Promise<void>;           // pending approvals in tray
  };
  update: {
    state(): Promise<{ phase: 'disabled' | 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error'; version?: string; message?: string }>;
    check(): Promise<void>; download(): Promise<void>; installAndRestart(): Promise<void>;
  };
}
export const CH = {
  serversList: 'servers:list', serversAdd: 'servers:add', serversRemove: 'servers:remove', serversActivate: 'servers:activate',
  engineState: 'engine:state', engineStart: 'engine:start', engineStop: 'engine:stop', engineLogs: 'engine:logs', engineEvent: 'engine:event',
  factoryList: 'factory:list', factoryConfigure: 'factory:configure', factoryTest: 'factory:test', factoryRemove: 'factory:remove',
  factoryQuery: 'factory:query', factoryOpen: 'factory:open', factoryClose: 'factory:close',
  shellOpenExternal: 'shell:open-external', shellNotify: 'shell:notify', shellClipboard: 'shell:clipboard',
  shellNavigate: 'shell:navigate', shellBadge: 'shell:badge',
  updateState: 'update:state', updateCheck: 'update:check', updateDownload: 'update:download', updateInstall: 'update:install',
} as const;
```

---

### Task 0: Reconcile with the code as built

**Files:** this plan's "Rulings made during execution" section only.

Steps:
1. Answer each of the following and record the answer, with file paths, as a ruling:
   - What does `boot()` in `web/packages/console-host/src/boot.ts` need from `PlatformService`?
   - How does `info.features` reach plugins? (Plugins hide on `features.local`.)
   - Does `@loams/proto` generate `loams.collection.v1`? Check `buf.gen.apps.yaml` and `web/packages/proto/src/gen`. If not, Task 9 adds it to `buf.gen.apps.yaml` and runs `pnpm --filter @loams/proto generate`.
   - How do the classic console (`src/main.tsx`) and the cordis console (`cordis.html`) link to each other? Pick one: the desktop window opens `cordis.html`, and the shell nav links to `/ui/` for the classic pages.
   - What is each adapter class's exported name, constructor config and methods for the §37 §19.6 ops? Look under `plugins/packages/plugin-*-adapter/src/service.ts`.
   - What do the `loams-apps-mock` routes need? Is the `/api/v1/session` shape the local shim must mimic?
   - What is the newest Electron, electron-vite, electron-builder and electron-updater version that is at least 14 days old (`npm view electron time --json`)?
2. Commit: `docs(ap1e): task 0 rulings`.

### Task 1: Scaffold `apps/desktop-electron` (window boots, workspace wired)

**Files:**
- Create: `apps/desktop-electron/{package.json,project.json,tsconfig.json,tsconfig.node.json,electron.vite.config.ts,vitest.config.ts,NOTICE,README.md}`
- Create: `src/main/index.ts`, `src/main/paths.ts`, `src/preload/index.ts` (empty bridge for now), `src/shared/contracts.ts` (above)
- Modify: `pnpm-workspace.yaml` (add `apps/desktop-electron`), root `package.json` `build:desktop` → `nx run loams-desktop-electron:build`, root `NOTICE` (dsh-desktop line)
- Test: `test/paths.test.ts`

**Interfaces produced:**
- `paths.ts`: `resolvePaths(env: {userData: string; logs: string; resourcesPath: string; isPackaged: boolean; appRoot: string}) => {engineData: string; logs: string; consoleDist: string; engineBin: string[] /* candidates in order */; factoryDir: string; serversFile: string}`
- `consoleDist`: packaged → `<resources>/console`; dev → `<repo>/web/apps/console/dist`.

Steps:
- [ ] Write `test/paths.test.ts`:
  - `packaged_paths_use_resources`;
  - `dev_paths_use_repo_dist`;
  - `engine_bin_candidates_order`: `LOAMS_BIN` first, then resources, then the cargo target directories `release` and `debug`; `.exe` on win32.
- [ ] Run `pnpm --filter @loams/desktop test` and expect FAIL (module missing).
- [ ] Implement `paths.ts`. In `package.json`, set name `@loams/desktop`, `"main": "out/main/index.js"`, and the scripts:
  - `dev: electron-vite dev`
  - `build: electron-vite build`
  - `test: vitest run`
  - `typecheck: tsc --noEmit -p tsconfig.node.json`
  - `package: electron-builder --config electron-builder.config.cjs`
- [ ] Write `electron.vite.config.ts`: main and preload only (no renderer; the renderer is the console build); `externalizeDepsPlugin()`; preload output CJS `index.cjs`.
- [ ] Write the `project.json` targets: `build`, `test`, `typecheck`, `dev`, `package-linux`, `package-macos`, `package-windows` (`nx:run-commands`, `cwd: apps/desktop-electron`). `build` `dependsOn` `@loams/console:build`.
- [ ] Write `index.ts`: single-instance lock; `app.whenReady` → a `BrowserWindow` with the D659 prefs, loading `https://example.invalid` for now. Replace it in Task 2.
- [ ] Run `pnpm install`, `pnpm --filter @loams/desktop test` (PASS), `typecheck` and `build`, then `python3 tools/monorepo/check.py`.
- [ ] Commit `feat(desktop): scaffold electron-vite app` and list the pinned versions in the body.

### Task 2: The `loams-app://` protocol — static console, API proxy

**Files:** `src/main/protocol/static.ts`, `proxy.ts`, `handler.electron.ts`; tests `test/protocol-static.test.ts`, `test/protocol-proxy.test.ts`

**Interfaces:**
- `resolveStatic(distRoot: string, url: URL): {file: string; mime: string} | {spaFallback: string} | null`
  - Only host `console` and path prefix `/ui/` are served.
  - The path is decoded once, NUL is refused, and the path is normalized with `path.posix`.
  - The result must stay inside `distRoot` (checked with `path.relative`; it must not start with `..` and must not be absolute).
  - A path with no extension falls back to the SPA: `/ui/cordis.html` if it starts with `/ui/cordis`, otherwise `/ui/index.html`.
- `isProxied(pathname: string): boolean`: true for the prefixes `/api/`, `/v1/`, `/loams.`, `/grpc.health.`, `/.well-known/`, `/health`, `/ready`.
- `proxyRequest(req: Request, target: string /* origin */, fetchImpl: typeof fetch): Promise<Response>`
  - Rewrites the URL to `target + pathname + search`, using `redirect: 'manual'`.
  - Drops the `origin` and `referer` headers, then sets `origin` to `target` (so CSRF origin checks pass).
  - On the response: removes `Domain=...` from every `set-cookie`, removes `content-security-policy` from API responses, and leaves 3xx untouched.
  - When the active server is local, it consults `localShim` first (Task 6 provides it; until then it is a no-op `() => null`).
- `handler.electron.ts`: `registerAppScheme()` (before ready: `protocol.registerSchemesAsPrivileged([{scheme:'loams-app', privileges:{standard:true, secure:true, supportFetchAPI:true, corsEnabled:false, stream:true}}])`) and `installAppProtocol(ses, deps: {distRoot; activeServer: () => ServerEntry; localShim})` using `ses.protocol.handle('loams-app', ...)` and `ses.fetch` for the proxy.
- The console's production CSP has `connect-src 'self'`, and the proxy keeps all API traffic on `loams-app://console`, so the CSP stays as it is.

Tests (write first, then implement):
- `serves_index_and_assets_with_mime`
- `spa_fallback_for_routes` (`/ui/projects/x` → index.html; `/ui/cordis/data` → cordis.html)
- `rejects_traversal_variants`: `..%2f`, `%2e%2e/`, `..\\`, `%00`, absolute `/etc/passwd`, host `evil`
- `isProxied_prefixes`
- `proxy_rewrites_url_and_origin`
- `proxy_does_not_follow_cross_origin_redirect` (fake fetch returns 302 to `https://idp.example`; the response status is 302 and the fetch is called once)
- `strips_cookie_domain` (`a=1; Domain=.example.com; Path=/; HttpOnly` → `a=1; Path=/; HttpOnly`)

Then wire `index.ts`: `registerAppScheme()` at module top, `installAppProtocol(session.defaultSession, …)`, and load `loams-app://console/ui/cordis.html`. With `cargo run -p loams-apps-mock` running and a temporary hard-coded demo server (`http://127.0.0.1:8084`), run `pnpm --filter @loams/console build && pnpm --filter @loams/desktop dev`. The console must render. Commit `feat(desktop): loams-app protocol with API proxy`.

### Task 3: Security baseline and deep links

**Files:** `src/main/security/policy.ts`, `install.electron.ts`, `src/shared/deeplink.ts`, `src/main/shell/single-instance.electron.ts`; tests `test/security-policy.test.ts`, `test/deeplink.test.ts`

**Interfaces:**
- `navigationDecision(from: string, to: string): 'allow' | 'external' | 'deny'`: allow within `loams-app://console/`; `external` for http(s); deny everything else (`file:`, `javascript:`, `data:`, other schemes).
- `permissionDecision(permission: string, origin: string): boolean`: true only for `clipboard-sanitized-write` and `notifications`, and only from `loams-app://console`.
- `parseDeepLink(raw: string): {path: string} | null`. `loams://open/console/...` maps to `/ui/...`, `loams://open/data[/...]` to `/ui/cordis/data...`, `loams://open/factory[/app]` to `/ui/cordis/factory...`, and `loams://open/servers` to `/ui/cordis/servers`. Anything else returns null. Path segments must match `[A-Za-z0-9._-]+`, and there is no query passthrough.
- `install.electron.ts` (adapt dsh `security.ts`): `will-navigate`, `setWindowOpenHandler` (deny; external goes to `shell.openExternal`), `will-attach-webview` preventDefault, `setPermissionRequestHandler`/`CheckHandler`, and `ipcMain` sender validation helper `assertTrustedSender(event)`.
- `single-instance.electron.ts`:
  - `app.setAsDefaultProtocolClient('loams')`, the `second-instance` link forward, and macOS `open-url`;
  - it focuses the window and sends `CH.shellNavigate` with the parsed path; links that fail to parse are logged and dropped.

Tests:
- `navigation_matrix` (a table of from/to pairs)
- `permission_matrix`
- `deeplink_allowlist` (good cases plus `loams://open/../x`, `loams://run/rm`, `loams://open/data?x=javascript:` → null)
- `assert_trusted_sender_rejects_other_frames`

Commit `feat(desktop): security policy and loams:// deep links`.

### Task 4: Server registry, preload bridge, typed IPC

**Files:** `src/main/servers/registry.ts`, `ipc.electron.ts`, `src/main/shell/window-state.ts` (adapt dsh atomic JSON storage), `src/preload/index.ts`; tests `test/registry.test.ts`, `test/window-state.test.ts`

**Interfaces:**
- `class ServerRegistry { constructor(file: string, opts: {devDemo: boolean}); list(); add(e); remove(id); activate(id); active(): ServerEntry }`
  - The `local` entry is always present (id `local`, url filled by the engine once ready, otherwise `''`) and cannot be removed.
  - The `demo` entry (`http://127.0.0.1:8084`) is present only when `devDemo` (unpackaged).
  - `add` accepts only `http:`/`https:` and stores the origin. Plain `http:` is allowed only for loopback hosts and otherwise returns `{ok:false, code:'insecure_url'}`.
  - The file is written atomically (temp file plus rename), versioned `{v:1, servers, activeId}`. A corrupt file is renamed to `servers.json.corrupt-<ts>`, and the registry starts with defaults.
- `window-state.ts`: `loadWindowState(file)` and `saveWindowState(file, state)` with bounds clamped to a visible display (the display list is injected).
- Preload exposes `LoamsDesktopApi` via `contextBridge.exposeInMainWorld('loamsDesktop', api)`, implemented with `ipcRenderer.invoke(CH.*)` and listener wrappers that return unsubscribe functions. No other globals.
- `servers.activate` persists, then calls `win.webContents.reloadIgnoringCache()`.

Tests:
- `local_entry_always_present_and_not_removable`
- `rejects_http_non_loopback`
- `stores_origin_only`
- `corrupt_file_recovers`
- `atomic_write_survives_crash` (simulate: the temp file exists, the rename did not happen)
- `window_state_clamped_to_display`

Commit `feat(desktop): server registry and preload bridge`.

### Task 5: Engine supervisor

**Files:** `src/main/engine/{ports.ts,binary.ts,supervisor.ts,log-rotate.ts,ipc.electron.ts}`, `test/fixtures/fake-engine.mjs`, `test/engine.test.ts`

**Interfaces** (adapted from dsh `runtime/harness-runtime.ts`):
- `reservePorts(n: number): Promise<number[]>`: listen on `127.0.0.1:0`, read the port, close; all `n` must be distinct.
- `findEngineBinary(candidates: string[], exists: (p)=>boolean): string | null`
- `engineArgs(dataDir, ports: {http, flight, es}): string[]`, which returns:
  ```
  ['dev','--data-dir',dataDir,'--listen',`127.0.0.1:${http}`,'--flight-sql-listen',`127.0.0.1:${flight}`,'--es-listen',`127.0.0.1:${es}`,'--no-qdrant','--no-durable']
  ```
- `class EngineSupervisor extends EventEmitter` (event `'state'`):
  - `constructor(deps: {spawn: typeof child_process.spawn; binary: () => string | null; dataDir; logFile; fetch; now: () => number; sleep: (ms)=>Promise<void>})`
  - `start()`, `stop(): Promise<void>`, `state(): EngineState`
  - Readiness: poll `POST {url}/loams.instance.v1.InstanceService/GetInstance` with body `{}`, `content-type: application/json`, every 250 ms. Timeout: 60 s, or 180 s on win32.
  - Restart policy: backoff 1, 2, 4, 8, 16, 30 s; at most 5 restarts in a rolling 10 minutes, then `failed`.
  - `stop`: SIGTERM, then after 5 s SIGKILL (`taskkill /T /F` on win32).
  - The child's env inherits `process.env` minus any `LOAMS_*_TOKEN`, `*_SECRET`, `*_API_KEY` variables.
- `RotatingLog(file, maxBytes=10*1024*1024, keep=5)`: `write(chunk)`, rotating `engine.log` → `engine.log.1` … `.5`.
- `fake-engine.mjs`: a Node HTTP server parsing the same `--listen` flag. `GetInstance` returns `{}` with 200. The env `FAKE_ENGINE_MODE=crash|slow|ok` controls its behaviour.

Tests:
- `reserves_distinct_ports`
- `engine_args_exact`
- `ready_with_fake_engine`
- `missing_binary_is_failed_not_thrown`
- `crash_loop_stops_after_five` (inject `now` and `sleep`)
- `stop_kills_after_grace`
- `env_scrubbed_of_secrets`
- `log_rotates_at_limit`

Wire it into `index.ts`:
- Start the engine at launch (a setting `engine.autoStart`, default true).
- On `ready`, set the registry `local.url`.
- Forward the `state` event to the renderer (`CH.engineEvent`).
- Stop it in `before-quit` (await, with a 6 s cap).

Commit `feat(desktop): local engine supervisor`.

### Task 6: Local-edition shim

**Files:** `src/main/protocol/local-shim.ts`, `test/local-shim.test.ts`

**Interface:** `localShim(pathname: string, method: string): Response | null`. It runs only when `active().kind === 'local'`, and gives:
- `GET /api/v1/instance` → 200 JSON, shaped like the apps mock's instance response (Task 0 ruling), with `edition: 'oss'`, `features: { local: true, desktop: true }`, `version: app.getVersion()`.
- `GET /api/v1/session` → 200 with `{ user: { id: 'local', name: os.userInfo().username, email: null }, org: { id: 'local', name: 'This computer' }, role: 'owner', csrf: null }`. The shape follows the Task 0 ruling.
- Any other `/api/v1/*` → 404 `{code:'not_in_local_edition', message:'This needs a Loams control plane. Add a server in Servers.'}`.
- Everything else → `null` (proxied to the engine).

Tests:
- `instance_and_session_shapes_match_mock` (compare keys with a recorded apps-mock response fixture saved under `test/fixtures/`)
- `other_api_v1_404`
- `non_api_returns_null`
- `shim_inactive_for_remote`

Commit `feat(desktop): local edition shim`.

### Task 7: `@loams/platform-electron`, the desktop catalog patch and the console desktop entry

**Files:**
- Create: `web/packages/platform-electron/{package.json,src/index.ts,src/index.test.ts,tsconfig.json}`
- Create: `web/apps/console/catalog/desktop.yml`, `web/apps/console/src/cordis/desktop.ts`
- Modify: `web/apps/console/src/cordis/main.tsx` (choose desktop vs web), `src/cordis/modules.ts` (register the 3 desktop plugins), `vite.config.ts` (CSP unchanged; confirm)
- Modify: the classic `src/shell.tsx` (a "Desktop" link to `/ui/cordis.html` when `window.loamsDesktop` exists)

**Interfaces:**
- `createElectronPlatform(api: LoamsDesktopApi): PluginModule`. It provides:
  - `platform`: `kind: 'desktop'`; `baseUrl = location.origin` (`loams-app://console`); `fetch` = `globalThis.fetch` with `credentials:'include'`; `openExternal` → `api.shell.openExternal`; `notify` → `api.shell.notify`; `clipboardWrite` → `api.shell.clipboardWrite`.
  - `transport`: `createConnectTransport({ baseUrl, useBinaryFormat: true, fetch })`.
  - `desktop`: the `LoamsDesktopApi` itself, as a cordis service, so only desktop plugins (which inject `desktop`) can use it.
- `desktop.yml`:
  ```yaml
  - insert: { id: desktop-servers, name: '@loams/plugin-desktop-servers' }
  - insert: { id: data-studio, name: '@loams/plugin-data-studio' }
  - insert: { id: factory, name: '@loams/plugin-factory' }
  ```
  Use the patch syntax Task 0 confirms in `console-host/src/catalog.ts`.
- In `main.tsx`, if `'loamsDesktop' in window`, call `startConsole({ platform: createElectronPlatform(window.loamsDesktop), patches: [parseCatalog(desktopYml)] })`; otherwise keep the web path unchanged.
- `PlatformService.kind`: add `'desktop'` to the union in console-host if it is not there.
- Subscribe to `api.shell.onNavigate` and route it with the shell's `router` service.

Tests:
- `platform_electron_provides_platform_transport_desktop`
- `openExternal_delegates_to_bridge`
- `browser_build_never_loads_desktop_plugins` (boot with platform-web plus the desktop patch, and assert that the desktop plugins stay pending or unloaded because `desktop` is never provided)
- `desktop_catalog_parses`

Commit `feat(console): desktop edition and platform-electron`.

### Task 8: `@loams/plugin-desktop-servers` (server switcher and engine status)

**Files:** `web/plugins/desktop-servers/{package.json,src/index.tsx,src/servers-page.tsx,src/switcher.tsx,src/engine-card.tsx,src/*.test.tsx}`

**Behaviour:**
- `package.json` sets `loams.plugin` with `kind: 'console'`, `tier: 'core'`, `inject: ['desktop','router','shell']`, `editions: ['oss','cloud','byoc']`.
- **Header switcher** (a shell slot; use the slot Task 0 names): shows the active server's name plus a status dot (for local: engine phase). The menu lists servers and "Manage servers…".
- **Route `/servers`:**
  - lists servers and lets the user add one (name and URL, with validation errors from IPC shown inline), remove one, and activate one;
  - an **Engine** card shows the phase, URLs (HTTP/ES/Flight, each with a copy button), Start/Stop and Open logs;
  - when the phase is `failed`, the card shows the reason.
- `stack-status` slot `environment.overview.card`: contributes the engine card when the active server is local.

Tests (RTL plus a fake `desktop` service):
- `lists_and_activates_servers`
- `add_shows_insecure_url_error`
- `engine_card_renders_each_phase`
- `failed_shows_reason_and_logs_button`

Commit `feat(plugins): desktop servers and engine status`.

### Task 9: `@loams/plugin-data-studio`

**Files:** `web/plugins/data-studio/{package.json,src/index.tsx,src/client.ts,src/pages/{collections,collection,search,sql,ingest}.tsx,src/*.test.tsx}`; if Task 0 says so, `buf.gen.apps.yaml` gains `loams/collection/v1`, and `web/packages/proto` is regenerated.

**Interfaces:**
- `client.ts`: `createDataClient(transport, fetch)` returns:
  - `listCollections(ns)`, `getCollection(ns, name)`, `createNamespace(ns)`, `createCollection(ns, spec)`;
  - `scroll(ns, coll, {pageToken?, limit})`, `count(ns, coll, filter?)`;
  - `search(ns, coll, ir)` (`QueryService/Search`), `write(ns, coll, docs, idempotencyKey)` (`DocumentService/WriteDocuments`; key = `crypto.randomUUID()` per ingest batch);
  - `sql(ns, query)` (REST `POST /v1/namespaces/{ns}/sql`; the response shape is from Task 0).
  - Connect clients come from `@loams/proto`'s `loams.collection.v1` descriptors.
- Routes, where the namespace comes from `/data/:ns?` (namespace picker; default `default`):
  - `/data`: collections table (name, documents, versions, hot), plus "New collection" (a form of name, PK, fields, vector dim/metric).
  - `/data/:ns/:coll`: schema, plus a documents grid with `ScrollDocuments` paging. Selecting a row shows its JSON in a side panel.
  - `/data/:ns/:coll/search`: query builder with text, a vector (paste a JSON array), a filter (JSON IR), `limit` and a hybrid toggle. Results show scores.
  - `/data/:ns/sql`: editor (`<textarea>` with monospace and Ctrl/Cmd+Enter to run), result grid, error panel.
  - `/data/:ns/:coll/ingest`: file picker for `.json` (array) or `.ndjson`. It parses in the renderer in batches of 500, calls `write` per batch, shows a progress bar, stops on the first error, and shows the batch index.
- The plugin works against any active server that serves the data plane. If `GetInstance` lists no `loams.collection.v1`, it shows the empty state "This server has no data plane."

Tests, with Connect clients over `createRouterTransport` from `@connectrpc/connect`:
- `lists_collections`
- `pages_with_page_token`
- `search_sends_ir_and_renders_scores`
- `sql_error_shown`
- `ingest_batches_of_500_and_stops_on_error`
- `ndjson_parse_reports_line_number_on_bad_line`
- `no_data_plane_empty_state`

Commit `feat(plugins): data studio`.

### Task 10: Factory host — adapters, vault, op allowlist

**Files:** `apps/desktop-electron/src/main/factory/{apps.ts,ops.ts,vault.ts,host.ts,ipc.electron.ts}`, tests `test/factory-*.test.ts`. Add `@loams-plugins/plugin-{forgejo,zulip,itsaplan,glitchtip,openpanel,matomo,langfuse}-adapter` and `cordis` as `dependencies` (`workspace:*`). Bundle them into main (electron-vite `externalizeDepsPlugin({ exclude: [...] })`), so the packaged app does not need the workspace.

**Interfaces:**
- `apps.ts`: `FACTORY_APPS: Record<FactoryAppId, {label; credentialFields; hasPanels; adapter?: () => Promise<AdapterCtor>; configFrom(url, fields) => adapterConfig}>`. The labels:
  - `Forgejo`;
  - `Zulip`;
  - `Plane (ItsAPlan)` (Q622);
  - `GlitchTip`;
  - `OpenPanel`;
  - `Matomo`;
  - `Langfuse`;
  - `OpenObserve` (`hasPanels: false`).

  Credential fields follow each adapter's env names, mapped to config (from the plugins report: Forgejo `token`; Zulip `email` and `apiKey`; ItsAPlan `apiKey`; GlitchTip `token`; OpenPanel `clientId` and `clientSecret`; Matomo `apiToken` and `siteId`; Langfuse `publicKey` and `secretKey`). Task 0 confirms the exact config keys.
- `ops.ts`: `OPS: Record<FactoryAppId, Record<string, {params: ZodSchema; run(adapter, params) => Promise<unknown>}>>`, per §37 §19.6, plus `health` for every app with an adapter. Read-only methods only; a test asserts the method names against an allowlist regex `^(get|list|search|health|fetch|query)`.
- `vault.ts`:
  - `class Vault { constructor(file, crypto: {available(): boolean; encrypt(s): Buffer; decrypt(b): string}); get(app): {url; fields: Record<string, Secret>} | undefined; set(app, url, fields); remove(app); persistent: boolean }`
  - The file is a JSON map of base64 ciphertexts.
  - When `available()` is false, data is kept in memory only, and `persistent=false`.
  - `class Secret { #v; reveal(): string; toString(){return '[redacted]'}; toJSON(){return '[redacted]'} ; [util.inspect.custom](){return '[redacted]'} }`
- `host.ts`:
  - `class FactoryHost { constructor(vault, ctx: cordis.Context); list(): Promise<FactoryAppInfo[]>; configure(app,url,fields); test(app); remove(app); query(q: FactoryQuery): Promise<IpcResult<unknown>> }`
  - It instantiates the adapter on first use, inside a child cordis context per app, and disposes it on remove or reconfigure.
  - It maps errors as follows: 401/403 → `auth_failed`, network → `unreachable`, otherwise `upstream_error`.
  - Messages pass through `redact(message, secrets)`, which replaces every secret value, and its URL-encoded form, with `[redacted]`.
  - The timeout is 15 s per query.
- `ipc.electron.ts`: `assertTrustedSender` on every handler, and replies always `IpcResult`.

Tests:
- `ipc_replies_never_contain_secret` (configure, then JSON.stringify every reply of `list`, `test`, `query` and `configure`; the secret substring must be absent)
- `adapter_error_is_redacted` (a fake adapter throws `Error('bad token abc123')`)
- `unknown_op_rejected`
- `params_validated`
- `ops_are_read_only_names`
- `vault_session_only_when_no_backend`
- `reconfigure_disposes_old_adapter`
- `health_maps_status_codes`

Commit `feat(desktop): factory host with encrypted credentials`.

### Task 11: `@loams/plugin-factory` (factory home and native panels)

**Files:** `web/plugins/factory/{package.json,src/index.tsx,src/home.tsx,src/configure.tsx,src/panels/{forgejo,zulip,plane,glitchtip,openpanel,matomo,langfuse}.tsx,src/*.test.tsx}`

**Behaviour:**
- `inject: ['desktop','router','shell']`.
- **Route `/factory`:** a grid of 8 app tiles showing label, health badge, URL, and the buttons Configure, Open app and Panels. When `persistent=false` the grid shows the banner "Credentials are kept for this session only: no system keychain found."
- **Route `/factory/:app/configure`:** a form generated from `credentialFields`. Secret inputs use `type=password` and are never prefilled. On save the form calls `configure` and then `test`, and shows the resulting health.
- **Route `/factory/:app`:** the app's panels, each using `desktop.factory.query` and the shared `@loams/ui` table, card and status-tag components. The panel data follows the §37 §19.6 table, with a refresh button and loading and error states. An `auth_failed` error links to Configure.
- **Overview:** contributes a `factory.summary` card to `environment.overview.card` (open PRs, unresolved errors, open issues) when those apps are configured.

Tests:
- `home_renders_eight_tiles_with_health`
- `session_only_banner`
- `configure_never_prefills_secret`
- `forgejo_panel_renders_prs` (and one per panel, with a fake `desktop.factory.query`)
- `auth_failed_links_to_configure`

Commit `feat(plugins): software factory home and panels`.

### Task 12: Factory full-UI views

**Files:** `apps/desktop-electron/src/main/factory/views.electron.ts`, `src/main/factory/view-policy.ts`, `test/view-policy.test.ts`

**Interfaces:**
- `viewNavigation(appOrigin: string, to: string): 'allow' | 'external' | 'deny'`: allow the same origin; also allow the IdP origin when the app config lists `ssoOrigin` (an optional config field, used for an Authentik URL); `external` for any other http(s); `deny` for anything else.
- `openApp(app)`:
  - opens one `BrowserWindow` per app (reused if open; title "<Label> — Loams Desktop");
  - `webPreferences: { partition: 'persist:factory-'+app, sandbox: true, contextIsolation: true, nodeIntegration: false, preload: undefined }`;
  - loads the configured URL;
  - applies `viewNavigation` to `will-navigate`, `will-redirect` and `setWindowOpenHandler`;
  - its permission handler denies everything except `clipboard-sanitized-write`.
- Credentials are never injected into the app view. The user signs in on the app's own page.

Tests: `factory_view_navigation_policy` (a table), `sso_origin_allowed_only_when_configured`. Commit `feat(desktop): factory app windows`.


### Task 12b: Factory apps embedded in the main window (D678)

**Files:** `apps/desktop-electron/src/main/factory/embed.electron.ts`, `src/main/factory/embed-model.ts` (pure: LRU of 4, bounds validation), `test/embed-model.test.ts`; contracts gain `factory.showEmbedded(app, rect: {x,y,width,height}) / hideEmbedded() / popOut(app)` plus CH channels and preload bindings; `web/plugins/factory` gains route `/factory/:app/app` (a toolbar with the app label, Reload, Pop out, Open in browser, and a placeholder `<div>` whose `getBoundingClientRect()` is sent on mount, resize and route change, using a ResizeObserver throttled to one animation frame). The fake desktop's `showEmbedded` renders a "Embedded view appears in the desktop app" placeholder.

**Rules:**
- **Isolation:** the view reuses the Task 12 policy functions unchanged (frameNavigation, downloadDecision, viewPermission) and the same partition.
- **Bounds:** main clamps the bounds to the window's content size and rejects non-finite or negative values.
- **Hiding:** `hideEmbedded` runs on unmount, on route change, and when the dock or a modal overlaps the area. In that last case the renderer sends hide, then show.
- **Lifecycle:** views survive hide. LRU eviction applies at 4 live views. A view is destroyed on remove, reconfigure or quit. Pop out destroys the embedded view and opens the Task 12 window.
- **Keyboard and focus:** `Ctrl/Cmd+L` (focus console) moves focus back to the console. The app view never receives the console's keyboard shortcuts.
- **Entry points:** "Open app" on the factory home now opens the embedded route. A secondary menu offers "Open in new window".

**Tests:**
- `bounds_clamped_and_validated`
- `lru_evicts_least_recent_at_four`
- `reconfigure_destroys_view`
- `popout_moves_to_window`
- the renderer test `embedded_route_reports_rect_and_hides_on_unmount`

**Manual check:** with the Task 12 static server: embedded navigation works, an external link opens the system browser, and resizing the window keeps the view aligned.

Commit `feat(desktop): factory apps embedded in the main window`.

### Task 13: Tray, menu, window state, notifications

**Files:** `src/main/shell/{tray.electron.ts,menu.electron.ts}`, `src/main/shell/tray-model.ts`, `test/tray-model.test.ts`; adapt dsh `close-to-tray.ts`.

**Interfaces:**
- `trayModel(engine: EngineState, badge: number, server: ServerEntry): {tooltip: string; items: {id; label; enabled}[]}`. Items: Open Loams Desktop; Engine: <phase> (start/stop); Pending approvals (<n>); Servers ›; Quit.
- `menu.electron.ts`: an application menu (macOS app menu, Edit, View with reload only in dev, Window, Help with Documentation `https://loams.dev/docs`, Open logs folder and About).
- **Close behaviour:** close-to-tray on Windows and Linux when a tray exists (setting `shell.closeToTray`, default true). On macOS, standard hide on close.
- `shell.setBadge(n)`: tray badge plus `app.setBadgeCount` where supported. The approvals plugin's count reaches it from `plugin-factory`'s overview, or from the approvals store if exposed (Task 0).
- `crashReporter.start({ uploadToServer: false })` (D663).

Tests: `tray_model_each_engine_phase`, `badge_label`. Commit `feat(desktop): tray, menu and window state`.

### Task 14: Updater with a signed manifest

**Files:** `src/main/update/{manifest.ts,pubkey.ts,updater.electron.ts}`, `scripts/sign-manifest.mjs`, `test/update-manifest.test.ts`

**Interfaces:**
- `verifyManifest(yml: Uint8Array, sigB64: string, pubkeyHex: string): Promise<boolean>`, using `@noble/ed25519` `verifyAsync`.
- `pubkey.ts`: `export const UPDATE_PUBKEY_HEX = process.env.LOAMS_UPDATE_PUBKEY ?? ''`, replaced at build time by electron-vite `define`. Empty means the updater is disabled.
- `updater.electron.ts` (adapt dsh `update/*`):
  - It is disabled if `!app.isPackaged`, if there is no feed (`LOAMS_UPDATE_FEED` at build, written to `app-update.yml` by electron-builder `publish: {provider:'generic', url}`), or if there is no pubkey.
  - Before `downloadUpdate()`, it fetches `<feed>/<channelFile>` and `<feed>/<channelFile>.sig` and verifies them; on failure the state is `error: 'unsigned_manifest'`.
  - Schedule: the first check 15 s after startup plus up to 30 s jitter, then every 6 h, and on `powerMonitor` `resume`.
  - `autoDownload=false`, `autoInstallOnAppQuit=true` once downloaded.
- `scripts/sign-manifest.mjs <yml> <privkeyHexFromEnv LOAMS_UPDATE_SIGNING_KEY>` writes `.sig`. CI uses it after electron-builder.

Tests:
- `valid_signature_accepted`
- `unsigned_manifest_is_refused`
- `manifest_for_other_key_is_refused`
- `tampered_manifest_refused`
- `disabled_without_feed_or_key`

Commit `feat(desktop): signed-manifest updater`.

### Task 15: Packaging and CI

**Files:** `apps/desktop-electron/electron-builder.config.cjs`, `build/*` icons (generated from `docs/assets/loams-banner.svg` or the logo in `web/packages/ui` with `scripts/icons.mjs`), `build/entitlements.mac.plist`, `scripts/fetch-engine.mjs`, `scripts/jsign-windows-hook.mjs` (adapted), `.github/workflows/desktop-electron.yml`, `release/` docs row.

**Config:**
- Build identity: `appId: dev.loams.desktop`, `productName: Loams Desktop`, `protocols: [{name:'Loams', schemes:['loams']}]`.
- `extraResources`:
  - `{from: '../../web/apps/console/dist', to: 'console'}`;
  - `{from: 'resources/bin', to: 'bin'}`.
- `files`: `out/**`, `package.json`.
- `asar: true`.
- Linux: `AppImage`, `deb`, `rpm`, `pacman`; `category: Development`; `desktop: {MimeType: 'x-scheme-handler/loams'}`.
- macOS: `dmg`, `zip`; `hardenedRuntime: true`; entitlements; `notarize` only if `APPLE_API_KEY` is set.
- Windows: `nsis` x64; signing through the Jsign hook only if `WINDOWS_SIGN_*` is set.
- `publish`: `generic` with `${env.LOAMS_UPDATE_FEED}` when set, otherwise `null`.
- `fetch-engine.mjs`: copies `loams` from the cargo target directory (`cargo build --release -p loams` first, or the `LOAMS_BIN` env) into `resources/bin/`, `chmod +x`.
- The Nx targets `package-linux`, `package-macos` and `package-windows` run `fetch-engine` then `electron-builder --linux|--mac|--win --publish never`.

**CI** (`desktop-electron.yml`):
- Triggers: on PRs touching `apps/desktop-electron/**`, `web/**`, `plugins/core/**` or `plugins/packages/**`.
- Matrix ubuntu-24.04, macos-15, windows-2025.
- Steps: pnpm install, then `nx run-many -t test typecheck -p loams-desktop-electron @loams/console ...plugins`, then `cargo build --release -p loams` (with Swatinem/rust-cache), then `package-<os>` unsigned, then the Playwright smoke (Task 16) under `xvfb-run` on Linux, then upload the artifacts labelled `unsigned`.
- Signing jobs run only on `release` events, with environment secrets (owner action: Q420, Q421).

Steps:
- [ ] Verify locally that `nx run loams-desktop-electron:package-linux` produces an AppImage and a `.deb` in `apps/desktop-electron/dist`.
- [ ] Verify that the AppImage launches (`--no-sandbox` is not needed; check that `chrome-sandbox` permissions are handled by the deb `afterInstall`).
- [ ] Commit `ci(desktop): packaging and workflow`.

### Task 16: End-to-end smoke

**Files:** `apps/desktop-electron/test/e2e/smoke.spec.ts`, `playwright.config.ts`, and the Nx target `e2e`.

**Scenario:** `_electron.launch({ args: ['out/main/index.js'], env: { LOAMS_BIN: fake-engine wrapper or real engine, LOAMS_DESKTOP_USER_DATA: tmp } })`, then:
1. The window title is "Loams Desktop".
2. Within 60 s, the server switcher shows Local with the status "ready".
3. Navigate to `/ui/cordis/data`. With the real engine: create namespace `e2e` and collection `docs`, ingest 3 docs, and confirm that search for "hello" returns ≥ 1.
4. Navigate to `/ui/cordis/factory`. All 8 tiles show `unconfigured`.
5. `loams://open/servers` via the `second-instance` simulation navigates to Servers.
6. Quit: the engine process exits within 6 s (assert that the PID is gone).

The real-engine variant runs when `LOAMS_BIN` points at a built `loams`. CI uses the real engine on Linux and the fake engine on macOS and Windows until Q623.

Commit `test(desktop): electron smoke`.

### Task 17: Docs and plan status

**Files:**
- `apps/desktop-electron/README.md`: dev setup, running against the apps mock and the local engine, packaging, signing env vars.
- `docs/plans/README.md`: an AP1e row "In progress"; AP1n status "Paused (D652)".
- `docs/design/37-desktop-and-mobile-apps.md`: the header amendment line pointing to §19.
- `CHANGELOG.md` `[Unreleased]`: Loams Desktop (Electron).

Commit `docs(ap1e): desktop docs and status`.

---


---

## Part 2: the full cloud console, the agent panel, Linux releases (§37 §19.10, D666–D676)

**Execution order:**
1. Tasks 0–16.
2. Then 18–31.
3. Task 17 (docs) last. It also covers Part 2.

**Amendments to Part 1** (§19.10 D676):
- **Task 15** builds Linux artifacts (AppImage, deb, rpm, pacman; x64 and arm64), and those are what get published.
- The macOS and Windows CI jobs build unsigned, publish nothing, and run with `continue-on-error: true`.
- Release publishing is Task 31.
- **Task 5's engine args change:** keep durable on a free loopback port (`--durable-listen 127.0.0.1:<p3>`), and add `--no-live` unless Task 22 supplies `livePd`. `EngineState.ready` gains `durableUrl: string` and `liveUrl?: string`.

New shared contracts, added to `src/shared/contracts.ts` by the first task that needs them:

```ts
export type StackId = 'postgres' | 'wesql' | 'tikv';
export type StackState =
  | { phase: 'unavailable'; reason: 'no_container_runtime' }
  | { phase: 'stopped' } | { phase: 'starting' } | { phase: 'running'; services: { name: string; state: string; ports: string[] }[] }
  | { phase: 'error'; message: string };
export interface SqlResult { columns: string[]; rows: unknown[][]; rowCount: number; truncated: boolean; elapsedMs: number }
export type ChatEvent =
  | { kind: 'delta'; chatId: string; text: string }
  | { kind: 'thinking'; chatId: string; text: string }
  | { kind: 'tool_call'; chatId: string; callId: string; tool: string; args: unknown; risk: 'read' | 'write'; needsApproval: boolean }
  | { kind: 'tool_result'; chatId: string; callId: string; ok: boolean; text: string }
  | { kind: 'done'; chatId: string; stop: 'end_turn' | 'iteration_cap' | 'wall_clock_budget' | 'token_budget' | 'llm_error' | 'cancelled'; usage: { inputTokens: number; outputTokens: number } }
  | { kind: 'error'; chatId: string; message: string };
// LoamsDesktopApi gains:
//   stacks: { state(id: StackId): Promise<StackState>; start(id): Promise<IpcResult<void>>; stop(id): Promise<IpcResult<void>>; onState(cb:(id: StackId, s: StackState)=>void): ()=>void }
//   pg:    { tenants(); timelines(tenant); createBranch(tenant, {name; ancestorTimelineId; ancestorStartLsn?}); walStatus(tenant, timeline); connection(): Promise<{host;port;database;user; passwordRef: string}>; revealPassword(): Promise<string>; query(sql: string): Promise<IpcResult<SqlResult>> }
//   wesql: { connection(); revealPassword(); schemas(); tables(schema); query(sql: string): Promise<IpcResult<SqlResult>> }
//   chat:  { providers(); configureProvider(id, {baseUrl?; model; apiKey?}); list(); get(chatId); create(); send(chatId, text); cancel(chatId); approve(chatId, callId, decision: 'once'|'always'|'deny'); remove(chatId); onEvent(cb:(e: ChatEvent)=>void): ()=>void }
//   connectors: { catalog(): Promise<ConnectorSummary[]>; get(id): Promise<{manifest: unknown; schema: unknown; stub: boolean}> }
```

### Task 18: Shell layout for a cloud console

**Files:** `web/plugins/shell` (modify): add the slots `shell.nav.section` (a list of `{id, label, icon, path, order, group}`) and `shell.dock.right` (single) and a header toggle for the right dock. Also `web/plugins/shell/src/*.test.tsx`.

**Behaviour** (D666):
- The left nav renders sections grouped as **Data** (Overview, Data, Postgres, WeSQL, Live), **Compute** (Durable, Streams & Links), **Integrate** (Connectors, Graph, Software Factory) and **Organisation** (Cloud, Settings), sorted by `order`. It collapses to icons under 1100 px.
- The header holds the server switcher slot, the agent toggle (`Ctrl/Cmd+J`) and the user menu.
- The right dock is resizable between 320 and 720 px. Its open state and width persist in `localStorage` (try/catch).
- Existing plugins keep working. Their current nav entries map to sections.

Tests:
- `nav_groups_and_orders_sections`
- `dock_toggle_and_shortcut`
- `dock_width_clamped_and_persisted`
- `existing_routes_still_render`

Commit `feat(console): cloud console shell layout`.

### Task 19: Backend B1: list streams and links, link lag and status (Rust)

**Files:**
- `crates/loams/src/api/mod.rs` (routes) and the streams and links handlers.
- The metastore trait list method if it is missing (`crates/loams-common` `MetaStore`, with implementations in `loams-meta` and `loams-meta-tikv`).
- Tests under `crates/loams/tests/`.
- `docs/api/route-map.md`.

**Contract:**
- `GET /v1/namespaces/{ns}/streams` returns `{"streams":[{"name","partitions","retention"?}]}`, sorted by name.
- `GET /v1/namespaces/{ns}/links` returns `{"links":[{"name","source","target":{"kind","name"},"status"}]}`.
- Link describe gains `"lag":[{"partition":p,"records":high_watermark-applied}]` and `"status":"running"|"unregistered"`, where `unregistered` comes from `LinkApplySource::unregistered()`.
- Unknown namespace returns 404 with the existing error shape.

Tests:
- `list_streams_sorted`
- `list_links_with_status`
- `link_describe_has_lag`
- `unknown_namespace_404`
- the conformance suite for the metastore trait, if a method is added (`loams-meta-conformance`)

Run `cargo test -p loams --test <file>` and `cargo test -p loams-meta-conformance`. Commit `feat(api): list streams and links, link lag`.

### Task 20: Backend B2: `_system:tables` for Live (Rust)

**Files:** `crates/loams-live/src/system.rs` (or wherever `system::lookup` is), `catalog.rs`, and tests.

**Contract:** `Query{function:"_system:tables", args:{}}` returns `[{name, id, indexes:[{name, fields:[...]}]}]`, including the implicit `by_id` and `by_creation_time` indexes. It is read-only, and an attempt to call it through `Mutate` is refused.

Tests: `system_tables_lists_created_tables` (using the crate's existing TiKV test harness, or its in-memory catalog if the crate has one; follow how existing `_system:*` tests run), `system_tables_via_mutate_refused`. Commit `feat(live): _system:tables`.

### Task 21: Local stacks manager

**Files:** `apps/desktop-electron/src/main/stacks/{runtime.ts,stacks.ts,ipc.electron.ts}`, `test/stacks.test.ts`. `extraResources` copies `deploy/neon`, `deploy/wesql` and `deploy/tikv` into `resources/stacks/`.

**Interfaces:**
- `detectRuntime(which: (bin)=>string|null): {bin: string; args: string[]} | null`. It tries `docker compose` first, then `podman compose`, then `docker-compose`.
- `class StackManager extends EventEmitter { state(id); start(id); stop(id) }`.
  - It runs `<rt> -p loams-desktop-<id> -f <resources>/stacks/<dir>/compose.yaml up -d` and `down`.
  - State comes from `ps --format json`, polled every 5 s while a page is open and on demand.
  - Commands are passed as an argument array (never a shell string), with a 5-minute timeout. Their output goes to `<logs>/stacks/<id>.log`.
  - Stack directories: postgres → `neon`, wesql → `wesql`, tikv → `tikv`. Read each `compose.yaml` and record the real port mappings in `stacks.ts` as constants.
- When the tikv stack reaches `running`, the main process restarts the engine with `livePd` set to the PD port (Task 5 amendment). When the stack stops, the engine restarts without it.

Tests:
- `detect_runtime_order`
- `compose_args_exact`
- `parses_ps_json` (with fixtures for docker and podman output)
- `unavailable_without_runtime`
- `tikv_running_restarts_engine_with_live`

Commit `feat(desktop): local stacks manager`.

### Task 22: Postgres and WeSQL in the main process

**Files:** `apps/desktop-electron/src/main/sql/{pg.ts,wesql.ts,neon.ts,caps.ts,ipc.electron.ts}`, `test/sql-*.test.ts`. Dependencies: `pg` and `mysql2` (exact pins, at least 14 days old).

**Interfaces:**
- `runCapped(exec, sql, {maxRows: 1000, timeoutMs: 30000}): Promise<SqlResult>`. For pg it sets `statement_timeout`; for mysql2 it sets `MAX_EXECUTION_TIME`. Rows beyond 1000 are dropped and `truncated` is set to true.
- `readOnly(sql)`: the agent's `pg_sql` and `wesql_sql` tools run inside `BEGIN READ ONLY … ROLLBACK` (pg) or `START TRANSACTION READ ONLY` (mysql). The UI console allows writes after a confirm dialog when the statement is not a plain SELECT, SHOW, EXPLAIN or WITH-SELECT.
- `neon.ts`, against the pageserver at `http://127.0.0.1:9898`:
  - `tenants()` → `GET /v1/tenant`;
  - `timelines(t)` → `GET /v1/tenant/{t}/timeline`;
  - `createBranch(t, {name, ancestorTimelineId, ancestorStartLsn?})` → `POST /v1/tenant/{t}/timeline` with `{new_timeline_id: random 32-hex, ancestor_timeline_id, ancestor_start_lsn}`. Branch names are kept in `<userData>/postgres/branches.json` (timeline id → name).
  - `walStatus(t, tl)` → the safekeeper on 7676.
  - Verify each route against `deploy/neon/README.md` and record deviations.
- `connection()` comes from the compose file constants. The password stays in main, `revealPassword()` returns it only on an explicit click, and `passwordRef` is an opaque id.
- Every handler returns `IpcResult`. Errors from `pg` and `mysql2` carry `code` and `message`, never a connection string with a password (redact).

Tests:
- `caps_truncate_and_flag`
- `timeout_maps_to_error`
- `read_only_wrapper_blocks_insert` (against a fake client asserting the SQL sequence)
- `neon_create_branch_body`
- `errors_redact_password`

Integration test (skipped unless `LOAMS_IT_PG=1`): bring up the postgres stack, `SELECT 1`. Commit `feat(desktop): postgres and wesql backends`.

### Task 23: Postgres and WeSQL pages

**Files:** `web/plugins/postgres/` and `web/plugins/wesql/` (`@loams/plugin-postgres`, `@loams/plugin-wesql`), plus tests. Both are added to `desktop.yml` and `modules.ts`, with nav sections from Task 18.

**Behaviour:**
- **Stack card** (shared component in `@loams/ui` or a small `web/packages/desktop-ui`): phase, Start/Stop and Open logs. When no container runtime is found it shows install guidance with links to docs.docker.com and podman.io (through `openExternal`).
- **Postgres** has three tabs:
  - **Branches:** a tree of timelines by ancestor, with name, id, LSNs, WAL heads and "Create branch from here".
  - **Connect:** host, port, db, user, a hidden password with reveal and copy, and a `psql` command line.
  - **SQL:** editor, Run (Ctrl/Cmd+Enter), result grid, "truncated at 1,000 rows" notice, error panel, and a confirm dialog for writes.
- **WeSQL** has three tabs:
  - **Schemas:** schema list, then tables (with engine and rows).
  - **Connect.**
  - **SQL.**
- Both pages say "Local stack" in the header and include the note "a Loams control plane will manage this on remote servers".

Tests:
- `stack_card_each_phase`
- `no_runtime_guidance`
- `branch_tree_from_timelines`
- `password_hidden_until_reveal`
- `write_requires_confirm`
- `truncated_notice`

Commit `feat(plugins): postgres and wesql pages`.

### Task 24: Live page

**Files:** `web/plugins/live/` (`@loams/plugin-live`), tests. Connect clients for `loams.live.v1` come from `@loams/proto` (add `loams/live/v1` to `buf.gen.apps.yaml` if absent, then regenerate). The protocol proxy forwards `/loams.live.v1.` to `liveUrl` (extend Task 2's `isProxied` routing with a per-prefix target map: `{'/loams.live.v1.': engine.liveUrl, '/durable/': engine.durableUrl, default: active server}`).

**Behaviour:**
- When Live is not running, the page shows "Live needs the TiKV stack" with the TiKV stack card and Start.
- **Tables:** from `_system:tables`.
- **Documents:** paged with `_system:query`.
- **Live query:** pick a table and an optional index filter, then a `Watch` stream with the rows updating in place. Each Transition increments a counter, and the panel shows a "live" pill. It unsubscribes on unmount.
- **Mutate:** insert (JSON), patch and delete row, each with a confirm dialog and an `idempotency_key`.
- **Deploy:** disabled, with the tooltip "Not yet available (R1 Task 13)".

Tests:
- `tables_listed`
- `watch_updates_rows` (using `createRouterTransport` with a server stream)
- `unsubscribes_on_unmount`
- `mutate_confirms_and_sends_idempotency_key`
- `needs_tikv_state`

Commit `feat(plugins): live page`.

### Task 25: Durable page

**Files:** `web/plugins/durable/` (`@loams/plugin-durable`), `src/envelope.ts`, tests.

**Interfaces:** `envelope(fetch)(kind, data) → Promise<data>`. It POSTs `/durable/` with `{kind, head:{corrId: crypto.randomUUID(), version:'2026-04-01'}, data}`. Errors come from `head.status >= 400`, as `{status, message}`.

**Behaviour** (D671):
- **Promises:** filter by state (pending, resolved, rejected, rejected_canceled, rejected_timedout), tags (`k=v` chips) and cursor paging. Detail shows id, state, times, tags, and `param` and `value` decoded (JSON when valid, otherwise base64 with a toggle). Create (id, timeout, param JSON, tags) and Cancel (confirm, then `promise.settle` with `rejected_canceled`).
- **Schedules:** list; create (id, cron with a human-readable preview from `cronstrue` (exact pin) or a 10-line formatter, promise id template, timeout); delete with confirm.
- **Tasks:** list by state; detail.
- **Runs:** a tree view built from promises sharing the `resonate:root` tag, with children via `resonate:parent`. Verify the tag names in the pinned resonate checkout `~/.cargo/git/checkouts/resonate-*/e360669` and record them.

Tests:
- `envelope_shape_and_error`
- `promise_filters_and_paging`
- `param_decoding_json_or_base64`
- `cancel_requires_confirm_and_sends_rejected_canceled`
- `runs_tree_from_tags`

Commit `feat(plugins): durable execution page`.

### Task 26: Streams & Links page

**Files:** `web/plugins/streams/` (`@loams/plugin-streams`), tests. Uses the B1 routes (Task 19) and the existing create, describe, produce and fetch routes through `platform.fetch`.

**Behaviour:**
- **Streams:** list, create (name, partitions, retention), and a detail view with per-partition offsets.
- **Produce a test record:** JSON or a CloudEvent.
- **Tail:** poll fetch every 2 s from the high watermark. Pause and resume; at most 500 records kept.
- **Links:** list with status, create (source stream, target kind and name, options JSON), and a detail view with a lag chart (inline SVG bars per partition, no chart library) and applied offsets.

Tests:
- `streams_list_and_create`
- `tail_caps_at_500`
- `link_lag_bars`
- `unregistered_status_badge`

Commit `feat(plugins): streams and links page`.

### Task 27: Connectors catalog and Graph empty state

**Files:**
- `apps/desktop-electron/scripts/connectors-catalog.mjs` (YAML via `yaml`, already in the workspace if present, else exact pin);
- `src/main/connectors/{catalog.ts,ipc.electron.ts}`;
- `web/plugins/connectors/` (`@loams/plugin-connectors`) and `web/plugins/graph/` (`@loams/plugin-graph`), with tests.

**Behaviour** (D673, D674):
- The catalog script emits `resources/connectors.json`: `[{id, name, category, status, runtime, source, sink, modes, auth, licence, stub}]` plus a per-id map of manifest and schema. A schema counts as a stub when it has no `properties`, or when it is marked as a stub by `gen_registry.py` (check its marker).
- **Connectors page:** search, filter chips, and a card grid. The detail view shows capabilities, auth, licence and runtime. The **Configure** form is generated from the JSON Schema (string, number, boolean, enum, object and array of primitives; `secret` fields as password inputs and stored nowhere). Validate with `ajv` (exact pin, 2020-12) and export the instance YAML (copy or save file). The Run button is disabled with "Connector runtime not yet available (CN1)".
- **Graph page:** an empty state explaining GQL over `loams.graph.v1` and that it arrives with `loams-fabric`. It links to the docs.

Tests:
- `catalog_script_counts_match_registry`
- `stub_flagged`
- `form_renders_kafka_schema_required_fields`
- `secret_fields_password_type`
- `export_yaml_valid`
- `graph_empty_state`

Commit `feat(plugins): connectors catalog and graph`.

### Task 28: Agent loop in the main process

**Files:** `apps/desktop-electron/src/main/agent/{providers/anthropic.ts,providers/openai.ts,providers/types.ts,loop.ts,tools.ts,store.ts,ipc.electron.ts}`, tests `test/agent-*.test.ts`. Provider HTTP uses `fetch` with SSE parsing (no SDK dependency).

**Interfaces:**
- `interface Provider { id; streamTurn(req: {model; system; messages: Msg[]; tools: ToolDef[]; signal}): AsyncIterable<ProviderEvent> }`, where `ProviderEvent = text | thinking | tool_use{id,name,input} | usage | stop{reason}`.
- Anthropic: `POST {baseUrl ?? 'https://api.anthropic.com'}/v1/messages` with `stream:true`, `anthropic-version: 2023-06-01`, and model default `claude-sonnet-5-5`.
- OpenAI-compatible: `POST {baseUrl}/chat/completions` with `stream:true` and tools in function format. Presets:
  - deepseek (`https://api.deepseek.com/v1`, `deepseek-chat`);
  - openai (`https://api.openai.com/v1`);
  - ollama (`http://127.0.0.1:11434/v1`, no key).
- `tools.ts`: `TOOLS: ToolSpec[]`, with `{name, description, risk: 'read'|'write', schema (JSON Schema), run(ctx, args)}` for the D675 list. Each tool calls the same main-process services the pages use (data plane via `session.fetch` against the active server; pg, wesql, durable, streams, connectors, factory). SQL tools force read-only.
- `loop.ts`: `runTurn(chat, userText, deps)`.
  - It streams events to the renderer (`CH.chatEvent`). On a write tool without an "always" rule it pauses, emitting `tool_call` with `needsApproval: true`, until `approve`.
  - Deny returns the tool result "The user denied this action."
  - Budgets: 25 iterations, 10 min, 200k tokens. `cancel` aborts the provider stream and pending tools.
  - Tool results are truncated to 20k characters before they are sent to the model.
- `store.ts`: one chat per file `<userData>/chats/<id>.json` (atomic write), plus the list sorted by updated time. The provider key lives in the D659 vault under `agent:<provider>`.

Tests:
- `anthropic_sse_parsing` (fixtures)
- `openai_sse_tool_call_assembly` (arguments split over chunks)
- `write_tool_waits_for_approval`
- `always_allow_scoped_to_chat_and_tool`
- `deny_returns_denial_result`
- `iteration_cap_stops`
- `cancel_aborts`
- `sql_tools_read_only`
- `key_never_in_events_or_store`

Commit `feat(desktop): agent loop with approvals`.

### Task 29: Agent panel plugin

**Files:** `web/plugins/agent/` (`@loams/plugin-agent`), tests. It fills the `shell.dock.right` slot.

**Behaviour:**
- **Chat list:** new chat, rename (first message), delete.
- **Messages:** markdown rendered with a safe renderer (`marked` plus DOMPurify, exact pins; no raw HTML from the model). Code blocks get a copy button. Tool calls render as collapsible cards with name, args and result.
- **Approval card for write calls:** shows the tool and args, with Approve once, Always for this chat, and Deny.
- **Composer:** Enter to send, Shift+Enter for a newline, and Stop while a turn runs. A provider and model picker sits in the composer footer. A provider that is not configured links to the Settings section.
- **Context hint:** the current page route and the active namespace are added to the system prompt. Example: "The user is viewing Postgres › Branches."
- **Stop reasons** render as a footer note, for example "Stopped: iteration cap (25)".

Tests:
- `streams_deltas_into_message`
- `approval_card_actions`
- `stop_button_cancels`
- `no_raw_html_rendered`
- `unconfigured_provider_cta`

Commit `feat(plugins): agent panel`.

### Task 30: Overview and Settings pages

**Files:** `web/plugins/overview/` (`@loams/plugin-overview`), with Settings additions in `desktop-servers` (or `@loams/plugin-desktop-settings`), and tests.

**Behaviour:**
- **Overview:** a grid of status cards. Each card links to its page and shows "not running" honestly.

  | Card | Shows |
  |---|---|
  | Engine | phase, URLs |
  | Data | namespaces and collections count |
  | Postgres | stack phase, branches count |
  | WeSQL | stack phase |
  | Live | running or not, tables |
  | Durable | pending promises count |
  | Streams | count, max link lag |
  | Connectors | preview/planned counts |
  | Factory | health per app |
- **Settings sections:**
  - **Servers** (Task 8).
  - **Agent providers:** keys as password fields, never shown back; "Test" sends a one-token request.
  - **Local stacks:** the container runtime found.
  - **Updates:** state, check now.
  - **About:** version, licences (NOTICE), Open logs folder.

Tests: `overview_cards_each_state`, `provider_key_never_prefilled`, `test_provider_reports_result`. Commit `feat(plugins): overview and settings`.

### Task 31: Release pipeline: Linux, Windows (SignPath), macOS (unsigned)

> **Amended by D677 (2026-10-08).**
> - The matrix adds `windows-2025` (NSIS x64) and `macos-15` (dmg and zip for arm64 and x64).
> - **Windows:** submit the unpacked app executables and the NSIS installer to SignPath. Add `signpath/artifact-configuration.desktop-windows.xml` and its policy, reusing `release-sign.yml` as a called workflow. Unsigned artifacts are refused when `DESKTOP_REQUIRE_SIGNING` is true. The Windows engine is bundled only if `cargo build --release -p loams --target x86_64-pc-windows-msvc` and the Windows smoke test pass. Otherwise the build sets `LOAMS_DESKTOP_NO_LOCAL_ENGINE=1`, and the engine pages show the "not available on Windows yet" state.
> - **macOS:** set `identity: null` (no signing) and skip notarization. Release notes carry the `xattr` instructions. The updater's macOS path shows "Download vX" (`openExternal` to the release page) instead of `downloadUpdate`.
> - Rename the workflow to `desktop-electron-release.yml` (unchanged), and cover all three operating systems in `docs/release/desktop.md`.

(original title: Linux release pipeline)

**Files:**
- `.github/workflows/desktop-electron-release.yml`;
- `apps/desktop-electron/scripts/{gpg-sign.sh,checksums.mjs}`;
- `signpath/artifact-configuration.desktop-rpm.xml` (or extend the existing rpm config);
- `docs/release/desktop.md`.

**Behaviour** (D676):
- Triggers on a tag `desktop-v*`.
- Matrix: `ubuntu-24.04` (x64) and `ubuntu-24.04-arm` (arm64).
  1. Build the engine (`cargo build --release -p loams`).
  2. Run `fetch-engine`, then `electron-builder --linux AppImage deb rpm pacman --publish never`.
  3. Write `SHA256SUMS`.
  4. Submit the `.rpm` to SignPath (reuse `release-sign.yml` as a called workflow, `require_signing` taken from repository variable `DESKTOP_REQUIRE_SIGNING`, default false).
  5. GPG-sign the `.deb` (`dpkg-sig` or detached `.sig`), the `.pkg.tar.zst` (detached `.sig`, the pacman convention), the AppImage (detached `.sig`) and `SHA256SUMS` (`SHA256SUMS.asc`). The key comes from `LOAMS_GPG_PRIVATE_KEY`; when it is absent, skip with a warning and label the release "unsigned".
  6. Create the GitHub Release (draft) with every artifact through `gh release create --draft`.
- `latest-linux.yml` is signed with the update key (Task 14) when `LOAMS_UPDATE_SIGNING_KEY` is present, and uploaded beside the artifacts. The feed URL is the GitHub release download URL for `LOAMS_UPDATE_FEED`.
- `docs/release/desktop.md` documents the secrets the owner must add (`LOAMS_GPG_PRIVATE_KEY`, `LOAMS_GPG_KEY_ID`, the SignPath values, `LOAMS_UPDATE_SIGNING_KEY` and `LOAMS_UPDATE_PUBKEY`) and how to cut a release.

Tests:
- `actionlint` on the workflow (if available locally; otherwise `python3 -c 'import yaml; yaml.safe_load(...)'`).
- A dry run of `gpg-sign.sh` with a throwaway key generated in the test into the scratch directory, then `gpg --verify`.
- `checksums.mjs` unit test.

Commit `ci(desktop): linux release pipeline`.

## Self-review

- **Spec coverage.** Each §19 rule maps to a task:

  | §19 rule | Task(s) |
  |---|---|
  | 19.1 | 17 |
  | 19.2: console | 2, 7 |
  | 19.2: local engine | 5, 6, 9 |
  | 19.2: factory | 10–12 |
  | 19.3 | 1, 3, 5, 13, 14, 15 (attribution) |
  | 19.4 | 2, 5, 6, 7 |
  | 19.5 | 3, 10, 12 |
  | 19.6 | 10, 11 |
  | 19.7 | 14, 15 |
  | 19.8 | 1, 13, 15, 16 |
- **Types.** `ServerEntry`, `EngineState`, `FactoryAppInfo`, `FactoryQuery`, `IpcResult` and `CH` are defined once in `contracts.ts` and used by name in Tasks 4–14.
- **Review Focus.** Items 1–5 each name an owning test (Tasks 2, 2, 5, 10, 12).

## Rulings made during execution

(Task 0 and later tasks append here.)

### Task 0 rulings (reconciled with the code, 2026-10-08)

**R0.1. What `boot()` needs from `PlatformService`.** `boot()` (`web/packages/console-host/src/boot.ts`) never reads `PlatformService` itself. It takes `options.platform: PluginModule` and runs `ctx.plugin({name: platform.name ?? 'platform', apply})` once. That plugin must `ctx.provide('platform', PlatformService)` and `ctx.provide('transport', Transport)`. `flagsPlugin` injects `transport` and calls `InstanceService.getInstance`; `@loams/plugin-rpc` also injects `transport` and `flags`. The contract is `PlatformService` in `web/packages/console-host/src/services.ts`: `kind: 'web' | 'desktop'`, `fetch`, `baseUrl`, `openExternal(url)`, `notify({title, body, route?})`, `clipboardWrite(text)`. The reference is `web/packages/platform-web/src/index.ts`: it uses `credentials: 'include'`, `createConnectTransport({baseUrl, useBinaryFormat: true, fetch})`, and `openExternal` refuses anything but http(s). `@loams/platform-electron` copies that shape, sets `kind: 'desktop'`, uses `baseUrl = location.origin` (`loams-app://console`) and routes the three side-effect methods over IPC. `startConsole({platform, root, patches, extraModules, extraManifests, base, grant})` is in `web/apps/console/src/cordis/start.ts`. Three consequences:
- (a) The manifest `requires` block accepts only `console` and `api` (`manifest.ts` rejects other keys). D658's "requires the `desktop` service" must therefore be expressed as `inject: ['desktop']` (the plugin stays `pending` until a service is provided). `platform-electron` must `provide('desktop', ...)` itself: a non-core plugin may only provide `<plugin-id>.*` services (`guard.ts`), and the platform plugin gets the raw context. Add `desktop: DesktopService` to `Services` by declaration merging (`declare module '@loams/console-host'`), because `service()` is typed on `keyof Services`.
- (b) A manifest's `editions` is `oss | desktop | cloud` (`manifest.ts` `EDITIONS`), not the `[oss, cloud, byoc]` that D658 shows. Desktop plugins declare `editions: ['desktop']`, and the existing first-party plugins already list `desktop`.
- (c) A manifest's `slots` is a gate. The desktop plugins list `console.page` and `console.nav`, plus `console.settings.section` if used.

**R0.2. How `info.features` reaches plugins.** The path is `flagsPlugin` -> `GetInstance` over the Connect transport -> `ctx.provide('flags', FlagsService)`. `FlagsService` is `{edition, instanceName, serverVersion, features: Record<string, boolean>, apiVersions, has(api)}`, filled from `info.features` (proto `map<string,bool> features = 6`). A plugin reads it with `inject: ['flags']` and `service(ctx, 'flags').features['local']`. `stack-status` and `rpc` already do this. `flags` is read once at boot and is not reactive. A server switch reloads the window, so that is fine.

Two findings that change D657:
- (i) The local engine's `GetInstance` leaves `features` empty (`crates/loams/src/api/connect.rs`, `Instance::get_instance`, `..Default::default()`). It returns edition `OSS` and `api_versions` of only `loams.instance.v1` and `loams.collection.v1`. The `/api/v1/instance` REST shim therefore does NOT feed the cordis console's flags. Only the classic console (`web/apps/console/src/session.tsx`) uses `/api/v1/instance`.
- (ii) Ruling: `@loams/platform-electron` builds the transport with a Connect **interceptor** that, when the active server's kind is `local`, sets `res.message.features.local = true` on the `InstanceService/GetInstance` response. This is decoded client-side, so the main process never re-encodes protobuf. The `/api/v1/*` shim in the protocol handler still exists for the classic pages.
- `identity` calls `rpc.instance.whoAmI`. The engine answers it `not_implemented`, which `identity` swallows into an empty session. A local session therefore has no principal and no environments, and `namespaces` shows its "No environments" empty state. Pages that need a control plane hide on `flags.features.local`.

**R0.3. `@loams/proto` does not generate `loams.collection.v1`.** `buf.gen.apps.yaml` lists only `options`, `instance`, `devices`, `approvals`, `operations`, `notifications` and `errors`. `web/packages/proto/src/gen/loams/` has the same seven directories. The source exists in `proto/loams/collection/v1/{collection,document,query}.proto`. It imports `google/protobuf/{empty,struct}.proto`, `loams/collection/v1/document.proto` and `loams/options/v1/options.proto`, all of which are already available (the Google types come from `@bufbuild/protobuf`'s wkt). The engine serves `NamespaceService`, `CollectionService`, `DocumentService` and `QueryService` (`crates/loams/src/api/connect.rs` `CATALOGUE`). Task 9 therefore adds `- proto/loams/collection/v1` to `inputs.paths` and runs `pnpm --filter @loams/proto generate`. That script is `cd ../../.. && buf generate --template buf.gen.apps.yaml` (buf 1.73.0). The generation is `clean: true` and is checked in CI for drift. Task 9 must also update the `proto/src/index.ts` namespace exports (a `collection` namespace like `instance`) and the package.json description.

**R0.4. Console linking.** The two entries do not link to each other. The classic console is `web/apps/console/index.html` -> `src/main.tsx`, a `createBrowserRouter(..., {basename: '/ui'})`, so a classic page is a real path such as `/ui/projects/x`. The cordis console is `web/apps/console/cordis.html` -> `src/cordis/main.tsx`, built into the same `dist` (`vite.config.ts`: `base: '/ui/'`, inputs `{main, cordis}`). Its router is a **hash router** (`web/plugins/shell/src/router.ts`, `HashRouter`, the comment names `#/approvals/apr_1`). A plugin route `/servers` is therefore reached at:

    loams-app://console/ui/cordis.html#/servers

(over http in dev: `http://127.0.0.1:5173/ui/cordis.html#/servers`). The nav renders `<a href="#/servers">`, and a parameterized route (`/approvals/:id`) gets no nav entry. A deep link `loams://open/console/<path>` navigates to `#/<path>`. Rulings:
- The desktop window loads `loams-app://console/ui/cordis.html`, and `onNavigate(path)` sets `location.hash = path` (or calls `router.navigate(path)`).
- Shell nav entries are hash links only (`<a href="#/path">`), so "Classic console" (`/ui/`, a real path, not a hash route) cannot go through `router.page`. Task 7 adds it as a plain anchor or button in a `console.settings.section` or a `shell.overlay` slot entry that calls `location.assign('/ui/')`; the nav slot's `meta.href` is always rendered as `#<href>`.
- The cordis page needs the CSP meta from `vite.config.ts` (`cspMeta`, production builds only: `script-src 'self'`, `connect-src 'self'`, `frame-src 'self'`). A `loams-app://` page counts as `'self'` only if the scheme is registered `standard`+`secure`+`supportFetchAPI`. This is the same constraint as D655.
- `public/config.json` is `{}`, and `loadRuntimeConfig(base)` fetches `<base>config.json`, so the protocol handler must serve it. `config.server` unset means same-origin, which is what D655 wants.

**R0.5. Catalog patch syntax** (`web/packages/console-host/src/catalog.ts`). The base is `web/apps/console/catalog/base.yml`: a YAML list of rows `{id, name, config?, disabled?, inject?}`, with `id` matching `[a-z][a-z0-9-]*`, `name` the package name, and **no YAML tags** (`!!js` etc. are rejected). A patch is also a list (`parsePatch`). A `- id:` row replaces a row's `config`/`disabled`/`inject` (the whole `config`, not a merge), and an `- insert:` row appends rows, the only key allowed beside it being none. `catalog/desktop.yml` is therefore:

```yaml
- insert:
    - id: desktop-servers
      name: '@loams/plugin-desktop-servers'
    - id: data-studio
      name: '@loams/plugin-data-studio'
    - id: factory
      name: '@loams/plugin-factory'
- id: hello
  disabled: true   # optional: the sandboxed sample is not shipped on desktop
```

The desktop edition passes `patches: [parsePatch(desktopYml)]` and `extraModules`/`extraManifests` to `startConsole`. A row's `inject` may only narrow its manifest's list. A patch naming an unknown `id` throws `CatalogError`. `base.yml` is imported with Vite's `?raw`, so `desktop.yml` must be imported the same way from the desktop entry.

**R0.6. How plugins register routes, nav items and slots.** A plugin is `{name, inject, apply(ctx, config)}` with the default export (`web/plugins/namespaces/src/index.tsx`):

```tsx
const plugin: PluginModule = {
  name: 'namespaces', inject: ['session', 'router'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    ctx.effect(() => router.page(
      { id: 'namespaces', path: '/namespaces', title: 'Namespaces', plugin: 'namespaces',
        nav: { group: 'Instance', order: 10 } },
      () => <NamespacesPage .../>));
  },
};
```

`router.page(spec, Component)` returns a disposer, so it is always wrapped in `ctx.effect`. It registers a keyed `console.page` slot (key = `spec.id`) and, when `spec.nav` is set and the path has no `:param`, a `console.nav` entry (`meta: {label, href: path, group}`). A page component gets `{params, environment}`. For other slots, use `slots.register({name, plugin, key?, order?, meta?}, Component)`; the slot names are in `web/packages/slots/src/types.ts` (`root` single, `console.nav` list, `console.page` keyed, `console.settings.section` list, `environment.overview.card` list, `approval.renderer` keyed, `shell.overlay` list). The package.json manifest is `"loams": {"plugin": {kind: 'console', entry: '.', tier: 'first-party', inject, provides, slots, permissions, requires: {console, api: [...]}, editions}}`. Registration is gated by the manifest `slots`, `inject` and `permissions` (the guard proxy throws on a service that was not injected). Nav groups come from `nav.group`. Pages register synchronously in `apply`. The sidebar groups by `meta.group` and sorts by `order`.

**R0.7. Adapters (under `plugins/packages/plugin-*-adapter/src/service.ts`).** Every adapter is a cordis `Service` subclass (`import { Context, Service } from 'cordis'`) with `static inject = []` and `constructor(ctx: Context, config)`; `super(ctx, '<name>')` registers `ctx.<name>`. They need no plugins server: the package tests do `new ForgejoAdapterService(new Context(), {baseUrl, token})`. A bare `new Context()` from `cordis` works, because `loggerFrom(ctx)` tolerates a missing `ctx.logger`. They call the global `fetch` through `@loams-core/http` (`UpstreamClient`, `UpstreamError{status, body, url, method}`). The only runtime dependencies are `cordis ^4.0.0-rc.10` and `@loams-core/http`; `@loams-core/host` is imported with `import type` only. Note the **two different workspaces**: the adapters are in the pnpm workspace under `plugins/packages/*` (names `@loams-plugins/plugin-*-adapter`, `main: src/index.ts`, raw TypeScript), while the web console's cordis is `@loams/cordis` (`web/packages/cordis`). The FactoryHost uses the plugins `cordis` and one `new Context()` per app, with the adapter built from a TypeScript path (bundle it into the main process with electron-vite, or alias the package). Constructor and config keys:

| App | Class (package export) | Config | Notes |
|---|---|---|---|
| Forgejo | `ForgejoAdapterService` | `{baseUrl, token, timeoutMs?, concurrency?(4), cacheTtlMs?, limit?}` | PAT sent as `Authorization: token <pat>`; `/api/v1` appended |
| Zulip | `ZulipAdapterService` | `{baseUrl, email (delivery email), apiKey, timeoutMs?, rateLimitFloor?(10), maxPages?(50)}` | `/api/v1` appended |
| ItsAPlan | `ItsAPlanAdapterService` | `{baseUrl, apiKey ('itp_...'), timeoutMs?}` | header `x-api-key`; no `/api/v1` in baseUrl |
| GlitchTip | `GlitchtipAdapterService` (lower-case t) | `{baseUrl, token, timeoutMs?}` | Bearer; `/api/0/` appended |
| OpenPanel | `OpenPanelAdapterService` | `{baseUrl, clientId (UUIDv4-shaped), clientSecret, apiPrefix?('/api'), funnelStepEncoding?, timeoutMs?}` | rate throttles built in |
| Matomo | `MatomoAdapterService` | `{baseUrl, apiToken, timeoutMs?, defaultRowLimit?, defaultPeriod?, defaultDate?}` | Bearer `token_auth` |
| Langfuse | `LangfuseAdapterService` | `{baseUrl, publicKey, secretKey, timeoutMs?}` | HTTP Basic; `/api/public` appended |

OpenObserve has no adapter (full UI only), and there is no `plane` adapter: Plane is served through the ItsAPlan adapter (Task 10 maps `plane` to ItsAPlan). A bad config throws in the constructor for some adapters (OpenPanel validates `clientId`). `forgejo.detach()` releases the cache, and the others hold no timers except OpenPanel's throttles. Each adapter also exports `<x>Manifest`, `<x>Loader` and `*_SKILLS`, which the FactoryHost does not use.

**R0.8. Proposed op -> method mapping for §19.6** (Task 10 fixes it in `ops.ts`; every listed method exists today and is a GET). Defaults noted are the adapter's, and `limit` is clamped to <= 50 for every op:
- Forgejo
  - `repos`: `searchRepositories(q ?? '', {page, limit})` -> `{ok, data: ForgejoRepository[]}`
  - `issues`: `searchIssues(q ?? '', {page, limit})`, which sends `type=issues` (Forgejo's default state is open). Pull requests are NOT in this search, so `issues` with `type: 'pulls'` is `listPullRequests(owner, repo, {state: 'open', page, limit})`, with `owner` and `repo` required in `params` for that variant.
  - `version`: `getVersion()`
  - `health`: `getVersion()` succeeding.
- Zulip
  - `streams`: `listStreams()`
  - `messages`: `fetchMessages({narrow: [{operator:'channel', operand: <name>}], anchor:'newest', num_before: n, num_after: 0})` returns `{messages, ...}`. `channel` is a valid `ZulipNarrowOperator` in the adapter.
  - `server`: `getSelf()` (there is no realm-settings method; `/users/me` proves auth and returns identity)
  - `health`: `getSelf()`.
- ItsAPlan
  - `stats`: `getStats(projectKey)`
  - `issues`: `listIssues(projectKey, {limit})`
  - `health`: `health()` plus `me()` (to separate `auth_failed`).
  - **Gap:** there is no project-listing method, so `projectKey` is an extra credential field or a required op param. Add the credential field `projectKey` (not secret) for Plane.
- GlitchTip
  - `organizations`: `listOrganizations()` -> `GlitchtipListPage` (it has `.data`)
  - `issues`: `listIssues(orgSlug, {query: 'is:unresolved', limit, sort?})`. The adapter has no `status` param (see `GlitchtipIssuesQuery`), so "unresolved" is the Sentry-style `query` string. Task 10 verifies it against the GlitchTip fixtures, and falls back to filtering `issue.status === 'unresolved'` client-side.
  - `health`: `root()` or `getCurrentUser()`.
- OpenPanel
  - `insights`: `overview(projectId, {range, interval})` for visitors and sessions, plus `topPages(projectId, {range, limit})`. `activeUsers(projectId)` and `live(projectId)` are available for extra tiles.
  - `health`: `health()` (unauthenticated `/healthcheck`) and `status()` (never throws). **Gap:** `projectId` is required for the insights, and `manageProjects()` needs a `root` client. Make `projectId` a credential field.
- Matomo
  - `visits`: `getVisitsSummary({idSite, period, date})` (`VisitsSummary.get`)
  - `pages`: `getPageUrls({idSite, period, date, rowLimit})`
  - `health`: `getVersion()`
  - **Gap:** `idSite` required (credential field `idSite`, or pick the first of `listSites()`).
- Langfuse
  - `traces`: there is NO trace-list method. Use `listObservations({isRootObservation: true, limit, fromStartTime?})` (the v2 observations feed, root observations = trace roots).
  - `daily`: `metricsDaily()` (the adapter flags `/metrics/daily` as undocumented, so a 404 is expected on some versions and the panel must show "unavailable")
  - `health`: `status()` (never throws).
- Health mapping: `UpstreamError.status` 401 or 403 -> `auth_failed`, any other HTTP error or a thrown fetch error -> `unreachable`, no credentials -> `unconfigured`.
- All adapters return upstream JSON as-is, so `ops.ts` must project each reply to a small fixed DTO before it crosses IPC, and a per-op zod-style param schema must reject anything else (§19.5).

**R0.9. `loams-apps-mock` and the `/api/v1/session` shape.** The prebuilt `/mnt/Projects/rust-cache/target/debug/loams-apps-mock` dated 2026-10-07 was STALE: it answered `/api/v1/*` with `404 {"code":"unimplemented"}`. It predates commit `26c6877e` (unified console routes), so run `cargo build -p loams-apps-mock` first (40 s incremental). Options: `--listen <addr>` (default `127.0.0.1:8084`), `--heartbeat-secs`, `--signed-out` (makes `GET /api/v1/session` answer 401), `--public-url` and `--ui-dir` (serves a console build at `/ui/`, default `web/apps/console/dist` if built). `GET /api/v1/instance` and `GET /api/v1/session` need **no auth or cookie** and answer 200. The fixtures are in `apps/desktop-electron/test/fixtures/apps-mock-instance.json` and `apps-mock-session.json` (the timestamps are relative to "now"). The schemas are in `api/console/openapi.json` (`Instance`, `Session`, both `additionalProperties: false`, so the shim's `desktop: true` and `features.local` are extensions that the typed generated client (`web/apps/console/src/api/schema.d.ts`) does not declare; `openapi-fetch` does not validate at runtime, so they pass through). Required fields:
- `Instance`: `{name, edition ('oss'|'cloud'|'byoc'), version, setup_required, sign_in: {password, totp, passkeys, oidc: [{id, name, kind}]}, features: {billing, multi_org, passkeys}}`.
- `Session`: `{user: {id, name, email, avatar_url, two_factor, sso, created_at, last_seen_at}, org: {id, name, slug, created_at, require_two_factor, allowed_domains}, role ('owner'...), csrf_token, expires_at}`. The classic console stores `csrf_token` and sends it as `X-CSRF-Token` on non-GET requests (`api/client.ts`).
- The local shim answers `instance` with edition `oss`, `setup_required: false`, `sign_in: {password: false, totp: false, passkeys: false, oidc: []}`, `features: {billing: false, multi_org: false, passkeys: false, local: true}`, plus `desktop: true`, and `session` with one fixed owner (`role: 'owner'`, `csrf_token: 'local'`, `expires_at` far in the future, the org `Local`).
- Error body: the engine's REST errors are `{"error": <code>, "message": ...}` (`crates/loams/src/api/errors.rs`) while the mock's are `{"code", "message"}`. D657's `404 {code:"not_in_local_edition"}` follows the mock/OpenAPI `Error` shape, so keep `code` and add `message`.
- The mock also serves the Connect/gRPC app protos on the same port (`/loams.instance.v1.InstanceService/GetInstance` etc.), plus `/health`, `/ready` and seeded `/v1/namespaces/{ns}/collections`. For Task 6's UI, the dev bearer is `mock-access-usr_omar` (`VITE_LOAMS_DEV_BEARER`) for the Connect calls (cordis console in dev mode), though the REST routes ignore auth.

**R0.10. SQL REST endpoint** (`crates/loams/src/api/sql.rs`, route in `api/mod.rs:240`). `POST /v1/namespaces/{ns}/sql`, content-type JSON. Request: `{"query": string, "consistency"?: ReadConsistency}`; unknown fields are rejected (`deny_unknown_fields`). `ReadConsistency` is serde snake_case: `"strong"` (the default), `"eventual"`, `{"at_least": <token>}` or `{"pinned": {"manifest_version": n, ...}}`. A `Loams-Consistency-Token` header may also be sent. One **read-only** statement only (`run_read_only`). Response 200: `{"columns": [{"name": string, "type": string}], "rows": [[...], ...], "truncated": boolean}`, where `type` is Arrow's display string for the type (for example `Utf8`, `Int64`), `rows` are arrays in column order, floats that are not finite become `null`, and many types are serialised as Arrow display strings. `truncated: true` means the row limit (`collections.config().sql`) cut the result. Errors: `{"error": <code>, "message": ...}` with a 4xx/5xx and a `Retry-After` on 429/503. The Connect services under `loams.collection.v1` are the alternative, and the CATALOGUE comment says `loams.sql.v1` is future. Data Studio therefore uses the REST SQL route (through the proxy, `/v1/`) for the SQL tab.

**R0.11. Pinned tool versions** (newest stable at least 14 days old, so published on or before 2026-09-24; from `npm view <pkg> time --json`; today is 2026-10-08):

| Package | Pin | Published | Newer (too new or other major) |
|---|---|---|---|
| electron | **44.4.5** | 2026-09-23 | 44.7.0 (2026-10-07), 44.6.0 (2026-10-06) are < 14 days |
| electron-vite | **5.0.0** | 2025-12-07 | none |
| electron-builder | **26.16.1** | 2026-09-07 | 26.17.0 (2026-09-26) is < 14 days |
| electron-updater | **6.8.9** | 2026-06-05 | 6.8.10 (2026-09-26) is < 14 days |
| @playwright/test | **1.63.0** | 2026-09-04 | 1.64.0 (2026-10-07) is < 14 days |
| @noble/ed25519 | **3.2.0** | 2026-08-27 | none (ESM only, node >= 20) |

- Electron 44.4.5 bundles **Node 24.21.0**, Chromium 152.0.7977.130 and V8 15.2 (from `electronjs.org/headers/index.json`; 44.7.0 has the same). The main process therefore has Node 24 APIs and a global `fetch`. The repo's own `engines.node` is `>=22`, so the dev Node and Electron's Node differ, and the Vitest unit tests must not assume Electron's.
- **electron-vite 5.0.0 declares `peerDependencies: vite ^5 || ^6 || ^7`** (+ optional `@swc/core`) and `engines.node ^20.19 || >=22.12`, while `web/apps/console` pins **vite 8.3.1**. The desktop package must pin its own `vite` 7.x (newest 7 is 7.3.7) beside electron-vite. pnpm resolves per package, but the monorepo `pnpm-workspace.yaml` overrides only touch `vite-plus`, so there should be no clash; the console is built by its own package (the renderer is the console's `dist`, copied in) and electron-vite then builds only `main` and `preload`.
- electron-updater 6.8.9 depends on `builder-util-runtime 9.7.0`, and electron-builder 26.16.1 should be pinned with the matching `app-builder-lib`. `@noble/ed25519` 3.x is async-first, with sync only after setting `etc.sha512Sync`. For the Ed25519 manifest check in main, Node's `crypto.verify(null, data, publicKeyObject, sig)` is the zero-dependency alternative, since the main process runs Node 24. Task 14 decides, and `@noble/ed25519` can stay as the pinned fallback.
- dsh-desktop is cloned at `~/Documents/Ostriumlabs/dsh-desktop` (HEAD `51f9896`) with `src/main/runtime/harness-runtime.ts`, `src/main/security.ts`, `src/main/security-policy.ts`, `electron-builder.dev.cjs` and `scripts/electron-builder-windows.mjs` for Tasks 3, 5 and 15.

### Controller rulings on Task 0 (2026-10-08)

- **C0.1** `features.local` and `features.desktop` reach cordis plugins through a Connect interceptor in `@loams/platform-electron`. The interceptor merges them into `GetInstance` responses when the active server is local, using `desktop.servers.list()`. The `/api/v1/*` shim (Task 6) serves only the classic console. Cost if wrong: one interceptor to move into main.
- **C0.2** Desktop plugins declare `editions: ['desktop']` (an edition console-host already knows) **and** `inject: ['desktop']`. That replaces D658's "requires the desktop service" wording, which is the same intent. Cost if wrong: none; a browser build never loads them twice over.
- **C0.3** `apps/desktop-electron` builds only main and preload, so it pins its own Vite 7 dev dependency for electron-vite 5.0.0. The console keeps Vite 8. Cost if wrong: a second Vite in the lockfile.
- **C0.4** Factory credential fields add `projectKey` (ItsAPlan), `projectId` (OpenPanel) and `idSite` (Matomo) as non-secret fields. Langfuse "traces" maps to `listObservations`. Cost if wrong: panel ops adjust in Task 10.
