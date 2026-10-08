# Loams Desktop (Electron)

Loams Desktop is one window with four things in it, built from plan AP1e
([design 37 §19](../../docs/design/37-desktop-and-mobile-apps.md)):

- **A cloud console.** The full cordis console (projects, environments, agents, access, teams, audit,
  settings) against any server you add by URL: Loams Cloud, a BYOC or self-hosted control plane, or the apps mock.
- **A local engine.** The app supervises `loams dev` on your machine, plus optional local stacks
  (Postgres on Neon, WeSQL, TiKV) with pages for Data, Postgres, WeSQL, Live, Durable, Streams & Links,
  Connectors and Graph.
- **Software Factory.** Forgejo, Zulip, ItsAPlan (Plane), GlitchTip, OpenPanel, Matomo, Langfuse and OpenObserve,
  as read-only native panels and as embedded full UIs.
- **An agent panel**, docked on the right, that works over the same services with approval for every write.

Main and preload are built by electron-vite; the renderer is the console build (`web/apps/console/dist`).

## Architecture

| Layer | What it does |
|---|---|
| `src/main` | Everything privileged. Each module has a pure core with no `electron` import (unit-tested) and a thin `*.electron.ts` adapter. |
| `src/preload` | One typed bridge, `window.loamsDesktop`, whose contracts live in `src/shared/contracts.ts`. |
| renderer | The cordis console with the `@loams/platform-electron` platform and the `catalog/desktop.yml` patch. Sandboxed, no Node. |

Main-process pieces:

- **`loams-app://` protocol** (`src/main/protocol`). The console is served from `loams-app://console/ui/`.
  `/api/`, `/v1/`, `/loams.`, `/.well-known/`, `/health` and `/ready` are proxied to the active server's origin, so the
  console keeps same-origin cookies and its router (D655). When the active server is the local engine, a small shim answers
  `/api/v1/instance` and `/api/v1/session` and returns `404 not_in_local_edition` for the rest (D657). `/durable/` is
  forwarded to the engine's durable listener.
- **Engine supervisor** (`src/main/engine`). Runs `loams dev` on free loopback ports, waits for `GetInstance`, restarts with
  backoff (at most 5 in 10 minutes), and logs to `engine.log`. The binary comes from `LOAMS_BIN`, the packaged
  `resources/bin`, or the Cargo target directory (dev only). Live flags are passed only if the binary supports them.
- **Local stacks** (`src/main/stacks`). Postgres (`deploy/neon`), WeSQL (`deploy/wesql`) and TiKV (`deploy/tikv`) run through
  `podman compose` or `docker compose`, whichever is found first, with project names `loams-desktop-<stack>`.
  The Postgres and WeSQL pages and SQL consoles live in `src/main/sql`.
- **Factory host and vault** (`src/main/factory`). A cordis context runs the `plugins/` adapters. Credentials are encrypted
  with `safeStorage` and never reach the renderer; IPC exposes only an allowlist of read-only ops. Full UIs are embedded as
  `WebContentsView`s (partition `persist:factory-<id>`, no preload, sandboxed) and can be popped out.
- **Agent loop** (`src/main/agent`, `src/main/agent-tools`). Runs in main. Providers are Anthropic Messages and
  OpenAI-compatible endpoints (presets for DeepSeek, OpenAI, Ollama). Tools are tagged read or write and every write waits
  for approval in the panel.
- Also: server registry, tray, deep links (`loams://open/...`, navigate only), single instance, and an updater that is off
  unless a feed is configured.

## Development setup

```sh
pnpm install
cargo build --release -p loams --features live,durable   # the engine; the default build lacks both features
pnpm --filter @loams/desktop dev                         # builds the connector catalog, then electron-vite dev
```

Cargo output goes to the shared target directory from `~/Documents/.cargo/config.toml`; do not set `CARGO_TARGET_DIR`.
Point the app at your build with `LOAMS_BIN=<target>/release/loams`.

Running against:

- **The apps mock** backs the cloud console REST API and the app protos with seed data. `cargo run -p loams-apps-mock`
  serves `http://127.0.0.1:8084` (change it with `--listen`). Add that URL as a server in the app. In a browser,
  `LOAMS_API=<url> pnpm --filter @loams/console dev` serves the console on `:5173` under `/ui/` (the cordis console is at
  `/ui/cordis.html`; add `?desktop` for the desktop edition with the fake bridge).
- **The local engine**: the default server. The cloud-only pages hide themselves (`features.local`).

### Browser preview

The desktop edition also runs in a plain browser with a fake `window.loamsDesktop`
(`web/apps/console/src/cordis/fake-desktop.ts`, sample data labelled "(fake)"):

```sh
pnpm --filter @loams/console dev --port <free port>
# open http://127.0.0.1:<port>/ui/cordis.html?desktop      (add &noruntime to see the no-container-runtime state)
```

Any new `window.loamsDesktop` namespace needs a fake there too, or the preview breaks.

## Tests

```sh
pnpm --filter @loams/desktop test        # unit tests (Vitest), no Electron needed
pnpm --filter @loams/desktop typecheck
pnpm --filter @loams/desktop test:e2e    # Playwright _electron smoke test (needs a built app: pnpm --filter @loams/desktop build)
```

The smoke test launches the app, waits for the local engine, loads the console and checks Data Studio and the factory home.
`LOAMS_E2E_ENGINE` picks the engine: `real` (fails if the binary is missing; needs `LOAMS_BIN` or a release engine in the Cargo target directory), `fake` (a Node script serving `GetInstance`),
or `auto` (default: the real engine if found, else the fake). CI sets `real`. Live tests against the container stacks are opt-in
(`LOAMS_IT_PG=1`, `LOAMS_IT_WESQL=1`).

## Security model

- **One origin, proxied (D655).** Only the listed API prefixes are forwarded, only to the active server, and no cross-origin
  redirect is followed. Cookie `Domain` attributes are stripped.
- **No credentials in the renderer (D659).** Factory and provider secrets live in `<userData>/factory/credentials.bin` under
  `safeStorage`. On Linux, persistence needs a real keyring backend; with none (or `basic_text`) credentials last for the session
  only and the UI says so. Provider API keys are bound to the origin they were saved with.
- **Hardening.** `contextIsolation`, `sandbox`, no `nodeIntegration`, `<webview>` refused, `window.open` limited to http(s) in the
  system browser, most permissions denied. Every IPC handler checks the sender.
- **Updates (D661).** Off unless `LOAMS_UPDATE_FEED` is set; `latest*.yml` must carry a valid Ed25519 signature against the key compiled in from `LOAMS_UPDATE_PUBKEY` (see Packaging).
- **Agent (D675, D679).** The panel is Loams' own, with budgets per turn (25 iterations, 10 minutes, 200k tokens; the clock pauses
  during approval waits). Output is rendered as text or markdown without raw HTML or remote images. Chats stay on disk locally.
  Anthropic refusal fallback is off unless enabled per provider.
- **Least-privilege SQL.** UI queries that are not plain reads ask for confirmation. The agent's SQL tools never use the
  administrative login: Postgres reads connect as a generated `loams_ro` role holding `pg_read_all_data`, MySQL reads as a `loams_ro`
  user with `SELECT` grants per schema (never `mysql.*`) and `secure-file-priv` disabled in `deploy/wesql`. Both run in read-only
  transactions with a shared dialect-aware lexer, a statement timeout, and a 1,000-row cap enforced while streaming.
- **No telemetry (D663).** Crash reports stay local.

## Troubleshooting

- **"No container runtime".** The Postgres, WeSQL and TiKV pages need `podman compose` or `docker compose` on `PATH`. Install
  Podman or Docker and reopen the page; the engine and the other pages work without it.
- **TiKV and Live.** Live needs the TiKV stack. TiKV must run with `api-version = 2` and TTL enabled
  (see `deploy/tikv/tikv.toml`); a TiKV started from another config makes Live fail at startup. The app restarts the engine with
  the PD address once the stack is running.
- **Keyring on Linux.** Saved credentials need a Secret Service provider (GNOME Keyring, KWallet). Without one `safeStorage` falls
  back to `basic_text`, which Loams treats as no encryption: credentials are session-only.
- **AppImage without FUSE.** `./Loams*.AppImage --appimage-extract-and-run`.
- **Logs.** Settings, About, "Open logs folder": `engine.log`, `stacks/<id>.log`.

## Packaging and releases

```sh
LOAMS_BUILD_ENGINE=1 node apps/desktop-electron/scripts/fetch-engine.mjs   # optional: build the engine first
pnpm nx run loams-desktop-electron:package-linux     # or package-macos / package-windows
```

- `scripts/fetch-engine.mjs` copies the engine (`LOAMS_BIN`, else `<cargo target_directory>/release/loams`)
  into `resources/bin/` and strips the copy. Build it with
  `cargo build --release -p loams --features live,durable`; the default build lacks both features.
- Config: `electron-builder.config.cjs`. Output goes to `dist/` (git-ignored). Targets: Linux AppImage, deb,
  rpm, pacman (x64 and arm64, built natively on each runner); Windows NSIS x64; macOS dmg and zip (arm64 and x64),
  unsigned. Linux rpm needs `rpmbuild`; pacman needs `bsdtar`; deb and rpm need a glibc with `libcrypt.so.1`
  for the bundled fpm (`libxcrypt-compat` on Arch).
- `build/tray-*.png` ship inside the asar (`files`), because the tray reads `app.getAppPath()/build`. App icons
  (`build/icon.*`) are generated by `scripts/make-icons.sh` and read by electron-builder.
- Build-time constants: `LOAMS_UPDATE_FEED` and `LOAMS_UPDATE_PUBKEY` are inlined into the main bundle by
  electron-vite `define`; `LOAMS_UPDATE_FEED` also turns on the generic `publish` block (writes `latest*.yml`).
- Windows signing: set `WINDOWS_SIGN_KEYSTORE` (+ `WINDOWS_SIGN_STOREPASS`, optional `_ALIAS`, `_STORETYPE`,
  `_TSA`) to sign through Jsign (`scripts/windows-sign.cjs`). SignPath submission is a release-workflow step.
- The package ships the console build (`web/apps/console/dist`), the stripped engine (built with `live,durable`) in `resources/bin/`, `stacks/` (the compose files), `connectors.json`, and the tray icons (inside the asar).
- AppImage on a host without FUSE: `./Loams*.AppImage --appimage-extract-and-run`.
- CI: `.github/workflows/desktop-electron.yml` (unsigned artifacts; macOS and Windows are `continue-on-error`).

Releasing (tags, signing secrets, the update feed) is covered in [docs/release/desktop.md](../../docs/release/desktop.md).
