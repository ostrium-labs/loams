![Loams — Your data. Your bucket.](../docs/assets/loams-banner.svg)

# web

The Loams console and its design system (design [§19](../docs/design/19-console-identity-and-agents.md)).

| Package | What |
|---|---|
| `packages/ui` (`@loams/ui`) | Tokens, the mark and wordmark, grain textures and React primitives: buttons, status tags, cards, empty states, fields, tables, stats, notices, dialogs, snippets, meters, avatars, formatters. Plain CSS with `loams-` classes; works in Vite and Next.js |
| `apps/console` (`@loams/console`) | One single-page app for OSS, Loams Cloud and BYOC, served at the base path's root (`/ui/`). It reads `InstanceService/GetInstance` to know its edition and the APIs the server serves, and `AuthService/ListProviders` for its sign-in methods |

### The cordis console (design §37 §5, AP1a)

The console is a [cordis](https://github.com/cordiverse/cordis) v4 application: a small host boots a cordis context from a plugin catalog, and everything else is a plugin with a manifest, declared services, slots and permissions. The original React console's pages moved into plugins (AP1a Task 5, API2 Task 10); its old addresses (`/ui/teams`, `/ui/projects/<p>/environments/<e>`, `/ui/cordis.html`) land on the same pages' hash routes (`/ui/#/teams`).

| Package | What |
|---|---|
| `packages/cordis` (`@loams/cordis`) | The only import of cordis (pinned `4.0.0-rc.10`) and its patch log |
| `packages/slots` (`@loams/slots`) | Typed `single`, `list` and `keyed` slots, the React renderer, per-entry error boundaries |
| `packages/console-host` (`@loams/console-host`) | Boot, the `loams.yml` catalog (no YAML tags, so no `!!js`), manifests (`loams.plugin` in package.json, `plugin-manifest.schema.json`), trust tiers, the guard proxy, permissions, the sandbox bridge; `./testing` has an in-memory app-protos mock |
| `packages/platform-web` (`@loams/platform-web`) | `platform` and `transport` in a browser |
| `packages/proto` (`@loams/proto`) | Generated clients for the app protos (AP0) |
| `packages/admin-ui` (`@loams/admin-ui`) | What the console's own pages share: page heads, the audit table, the activity chart, labels for the `loams.admin.v1` enums, the pages' styles; `./testing` has an in-memory `loams.auth.v1` and `loams.admin.v1` |
| `packages/agentd-client` (`@loams/agentd-client`) | The `loams-agentd` wire types, the method table, the transcript reducers, and the Connect transports for `loams.agentd.v1` (desktop through Electron main, remote over TLS) |
| `packages/studio-ui` (`@loams/studio-ui`) | Loams Studio's components, vendored from T3 Code (MIT) and rebuilt on `@loams/ui`; see its [README](packages/studio-ui/README.md) for the vendoring rules |
| `plugins/*` | `shell` (layout, nav, `router`, the mode switcher and the command palette), `rpc` (`rpc.*` clients, gated on `api_versions`, and `rpc.agentd.*`), `identity` (`session`, with the console's sign-in gate), the console's own pages (`account`: sign-in, setup, consent; `org`: overview, teams, members, settings; `projects`: projects, environments, access and the project switcher; `agents`; `audit`, live), `stack-status`, `namespaces`, `approvals`, `sandbox` (the in-frame runtime), `studio` (Loams Studio) and the desktop's own plugins |
| `examples/plugin-hello` | A third-party plugin that runs in a sandboxed frame |
| `apps/console/catalog/base.yml` | The `oss` plugin set |

```bash
cargo run -p loams-apps-mock                 # the console API and the app protos on :8084
pnpm dev                                     # then open http://localhost:5173/ui/ (proxied to the mock)
                                             # http://localhost:5173/ui/?demo is the in-browser demo, no server
VITE_LOAMS_APPS_URL=http://127.0.0.1:8084 VITE_LOAMS_DEV_BEARER=mock-access-usr_omar pnpm dev
                                             # straight at the mock with a bearer token
pnpm test                                    # vp test (Vitest), every package
```

Third-party plugins are off unless the instance sets the `console.third_party_plugins` feature (the demo mock does): until the unified auth plan can vend attenuated tokens there is no server-side boundary for them. `window.loamsConsole` in the browser console shows the plugin table and `pending()`.

### Console and Studio

The console has two modes in one shell (design [§53](../docs/design/53-studio-ade.md) §4, D900–D902; plan SD1): **Console**, the cloud console, and **Studio**, the agentic development environment. A plugin's manifest names its modes (`"modes": ["console"]`, `["studio"]` or both; the default is `["console"]`), and the host's `mode` service filters the nav, pages, header controls and palette rows by mode. Switching (the header's segmented control, Mod+Shift+M, or the palette) never reloads the page. Studio's routes all start with `/studio`. The desktop opens in Studio and the browser in Console; after that, the last mode used on the device (`loams.shell.mode`). In the browser Studio is gated on the instance feature `studio` until SD1 Task 32; the dev server's `?demo` turns it on. Studio reaches `loams-agentd` over `loams.agentd.v1`; only first-party plugins with the `agentd:use` permission get those clients, and git writes also need `agentd:git-write`. The user guides are in [`docs/guides/studio`](../docs/guides/studio/index.md).

### Connect

The console speaks Connect: `loams.auth.v1` and `loams.admin.v1` (API2 Tasks 6, 7) through `@loams/proto` and `@connectrpc/connect-query`, over the console's one transport. Until the server serves them (API2 Task 7's server half), [`loams-apps-mock`](../crates/loams-apps-mock) serves them from the console seed on `:8084`, beside the app protos; the seed itself lives in [`loams-console-mock`](../crates/loams-console-mock). The console's pages appear only where `GetInstance.api_versions` names those packages.

## Develop

The pnpm workspace is rooted at the repository root: its `package.json` pins pnpm `11.27.1`, and `pnpm-workspace.yaml` and `pnpm-lock.yaml` are shared by all workspace packages. Prefer running `pnpm install --frozen-lockfile` from the repository root; `pnpm install` from `web/` also discovers the root workspace and uses that same lockfile. The `web/` package scripts and directory layout are unchanged; run the web commands below from `web/`.

```bash
cargo run -p loams-apps-mock          # the console API mock on :8084 (add --signed-out for the sign-in screens)
cd web && pnpm install && pnpm dev       # the console on http://localhost:5173/ui/
```

**`protoc` is needed for the Rust side.** `loams-apps-mock`'s build script (and `loams-agentd-connect`'s, for `loams-agentd`) generates its Connect code with `protoc`, the binary on `PATH` or the one `PROTOC` names; without it the build stops with "is protoc installed?". On Debian and Ubuntu, `sudo apt-get install -y protobuf-compiler libprotobuf-dev` (what CI does); elsewhere see [build from source](../docs/build-from-source/README.md). The web side needs no `protoc`: `@loams/proto`'s generated code is checked in, and `buf generate` is only for changing a `.proto`.

`pnpm dev` proxies the Connect paths (`/loams.<package>.<Service>/<Method>`), `/api`, `/v1`, `/.well-known`, `/health` and `/ready` to the mock. To use a real engine instead, set `LOAMS_API=http://127.0.0.1:8080`.

**`LOAMS_UI_BASE`** sets where the console is mounted (Vite's `base`): `/ui/` by default, as the engine serves it (design §19 §3). `LOAMS_UI_BASE=/ pnpm dev` serves it at the root, and the same variable applies to `pnpm build`. Old addresses (`/ui/cordis.html`, `/ui/teams`) are moved to the console's hash routes on load.

Two dev-only entry points need no server at all (they exist only under `import.meta.env.DEV`; a production build drops them):
- **`/ui/?demo`** is the browser edition on an in-browser mock of the app protos, with the sample third-party plugin and Studio turned on.
- **`/ui/?desktop`** is the desktop edition on a fake `window.loamsDesktop` (`src/cordis/fake-desktop.ts`), with a fake agentd (`fake-agentd.ts`) that gives Studio sample projects, threads, worktrees, a run group and diffs. Any new `window.loamsDesktop` namespace needs a fake there too.

## Check and build

```bash
pnpm lint          # vp check: Oxlint and Oxfmt (vite.config.ts)
pnpm typecheck     # tsc, every package
pnpm test          # vp test (Vitest), every package
pnpm build         # @loams/ui to packages/ui/dist, the console to apps/console/dist
```

The console builds for the base path `/ui`, bundles its fonts and assets, and loads nothing from the internet, so an air-gapped install works. The engine will embed `apps/console/dist` behind a `console` feature (design §19 §3).

## `@loams/ui` outside this workspace

```ts
import '@loams/ui/styles.css';
import { Button, Card, Logo } from '@loams/ui';
```

Put the class `dark` on `<html>` for dark mode. The tokens use the system UI and monospace stacks (`--loams-font-sans`, `--loams-font-mono`), so no font needs loading. Inside the workspace the package resolves to its TypeScript sources; `pnpm --filter @loams/ui publish` publishes the built `dist` (`publishConfig`).

## Fonts

The console bundles no font: it uses the system UI and monospace stacks (design 59, D1075). Monospace is for code, ids and SQL only. The wordmark is outlined from Archivo (see `NOTICE`).

## Deploy the console (hosted, Cloudflare, Vercel) — preview

Loams ships as a preview. The console ships with each deployment: the engine serves this build at `/ui/` (the engine image and the Linux packages carry it in `/usr/share/loams/console`), same-origin with its API, and Loams Desktop bundles it. The hosted console at `console.loams.dev` is retired (design 78; the site redirects to `loams.dev/cloud`). Ostrium hosts only the auth service; everything else runs on your machine or your own VMs (Docker Compose, `deploy/release/compose.yaml`). The browser talks to **your** engine over your tailnet (Tailscale; Headscale is coming soon): the device running the browser needs the Tailscale client, logged in to the engine's tailnet. The engine is never exposed on the public internet. The rest of this section describes a console deployed on its own origin (the `hosted` edition: the stack's `--profile console`, or your own Cloudflare or Vercel account, coming soon); that origin must be in the engine's `LOAMS_CORS_ORIGINS`, which allows no origin by default.

The hosted console is the cordis console (`cordis.html`) served at `/`. On first load it:

1. shows **Connect your engine** on first run (and whenever the API key is needed): the person picks how to sign in and, unless the deploy fixes one, enters the tailnet URL (`https://loams.<tailnet>.ts.net`); **Test connection** calls `GET /health`. The engine URL and the chosen mode are remembered in this browser (neither is a credential);
2. signs in the chosen way (below);
3. boots the console against that engine (no cookies); the header shows who the engine is talking to.

| Mode | Sign-in |
|---|---|
| `tailscale` (default) | No sign-in screen. The engine identifies the caller from the tailnet; the header shows the user `AuthService/GetSession` returns. |
| `oidc` | A Loams account: Authorization Code + PKCE at `https://auth.loams.dev` with the `loams-console` client, sent as `Authorization: Bearer <token>`. Tokens stay in memory; a reload renews silently with `prompt=none`, a refresh token is used while the page lives; nothing that grants access goes to `localStorage` or `sessionStorage`. Offered when the deploy has a client. |
| `apikey` | The connect screen requires an engine API key on every load, kept in the tab's memory only and sent as the bearer. |

`LOAMS_AUTH_MODE` sets the default. Any other value fails the deploy (`config.mjs`), and a `config.json` with an unknown mode, or `oidc` without a client, stops at a screen that says so; the console never falls back to another mode on its own.

`LOAMS_AUTH_MODES` narrows what the connect screen offers: the modes this deploy has actually tested, separated by spaces or commas, for example `tailscale`. Unset, all of them are offered (`oidc` when the deploy has a client). It must be a subset of the three, must include the default `LOAMS_AUTH_MODE`, and `oidc` in it needs `LOAMS_AUTH_CLIENT_ID`; anything else fails the deploy. With one mode there is no picker: the connect screen asks only for the engine URL.

**Coming soon** (shown as such on the connect screen, not linked): deploying this console to your own Vercel or Cloudflare account, self-service client registration at auth.loams.dev (`https://auth.loams.dev/register`, URL to confirm), and Headscale networks.

**Demo mode**: `?demo` boots the console with no engine at all — the built-in sample data over an in-browser mock of the Loams APIs (`@loams/console-host/testing`), a **Demo data** badge in the header, and a link back to the connect screen. The gate screens link to it (and to `?demo=1`), because it is the one way in that needs no engine, no tailnet and no sign-in: [console.loams.dev/?demo](https://console.loams.dev/?demo). Nothing is sent anywhere and nothing is stored.

Pages whose backend the engine serves (Live, Graph, Analytics/House, Status, Namespaces, Approvals) are on; pages that still run on built-in sample data (Deployments, Live deployments, SQL, Metrics, Pipelines, Git) appear only with the **Demo data** toggle in the header. Every page shows the **Preview** badge.

Runtime config is written at deploy time from the environment by `apps/console/deploy/config.mjs` (and by the stage step), never baked into the bundle:

| Variable | Meaning |
|---|---|
| `LOAMS_AUTH_MODE` | the default mode: `tailscale` (default), `oidc` or `apikey`; anything else is an error. |
| `LOAMS_AUTH_MODES` | optional: the modes the connect screen offers, separated by spaces or commas (`tailscale`). Unset offers all three (`oidc` with a client). Must include `LOAMS_AUTH_MODE`; `oidc` in it needs `LOAMS_AUTH_CLIENT_ID`. |
| `LOAMS_AUTH_CLIENT_ID` | the console's public client id at the issuer (`loams-console`; redirect URI: the console's origin + `/`). With it, `oidc` is offered; required for `LOAMS_AUTH_MODE=oidc`. |
| `LOAMS_AUTH_ISSUER` | the issuer, default `https://auth.loams.dev`. Your own IdP is accepted here but not offered in the UI yet (coming soon). |
| `LOAMS_AUTH_SCOPE` | optional, default `openid profile email offline_access`. |
| `LOAMS_ENGINE_URL` | optional: fixes the engine (skips the connect screen and adds its origin to the CSP). |
| `LOAMS_AGENTD_ORIGINS` | optional: the origins of the remote agentd hosts Studio may use (`loams-agentd serve --remote`), separated by spaces or commas, each an exact `https://host[:port]`. They join the CSP's `connect-src` (the Connect calls) and `frame-src` (the previews). Each host must also list this console in its `--allow-origin`. Unset, Studio in the browser can reach no agentd host. The engine-served console reads the same variable from the engine's environment: the engine takes the build's meta CSP out of the page it serves, sends it as a header, and adds these origins to `connect-src` only (not `frame-src`, so previews stay off there). |

`config.json` keys: `hosted`, `engineUrl`, `authMode`, `authModes`, `authIssuer`, `authClientId`, `authScope`. The engine-embedded console keeps serving `{}` (same origin, cookie session).

The stage step injects a Content-Security-Policy `<meta>`: no inline scripts in the cordis console, no eval, `connect-src` limited to the console itself, `https://*.ts.net` (port 443, where `tailscale serve` publishes the engine), the issuer and the fixed engine. Headscale networks are coming soon (Headscale issues no HTTPS certificates, so such an engine needs its own certificate and a console deployed with `LOAMS_ENGINE_URL` set to it).

The engine must answer CORS for the console's origin, and the issuer must answer CORS on its discovery and token endpoints (the browser exchanges the code itself).

### Cloudflare

Ostrium deployed console.loams.dev this way until it was retired (design 78, D1705); its deploy workflow is gone. Deploying to your own Cloudflare account (`--env byoc`) is coming soon.

Workers static assets, configured in [`apps/console/wrangler.jsonc`](apps/console/wrangler.jsonc): assets only, SPA fallback, free plan.

A deploy that has tested only some sign-in modes lists them in `LOAMS_AUTH_MODES` (for example `LOAMS_AUTH_MODES=tailscale`). Everything the stage step needs comes from the environment, so a deploy with no `LOAMS_AUTH_MODES` offers all three.

```bash
pnpm build
LOAMS_AUTH_CLIENT_ID=loams-console LOAMS_AUTH_MODES=tailscale \
  node apps/console/deploy/stage.mjs     # dist/ -> dist-cloudflare/: /, ui/, config.json, _headers, _redirects
cd apps/console && wrangler deploy --env byoc   # your account, on workers.dev (coming soon)
node deploy/smoke.mjs https://<your-console>
```

No workflow deploys the console any more: it ships with each deployment, served by the engine at `/ui/`, and with Loams Desktop.

### Vercel (coming soon)

Deploying the console to your own Vercel account is coming soon: the configuration below is kept but not yet supported or advertised in the console. [`apps/console/vercel.json`](apps/console/vercel.json): set the project's Root Directory to `web/apps/console` and the environment variables above. It installs from the workspace, runs `pnpm build` and `stage.mjs --target vercel` into `dist-vercel/`, rewrites unknown paths to the console and sends the security headers. Vercel headers are static, so its CSP names `https://*.ts.net` and `https://auth.loams.dev`; the stage step fails the build when the configured issuer or engine is missing from it — edit `connect-src` in `vercel.json` for another issuer.

`node --test apps/console/deploy/*.test.mjs` tests the config, stage and smoke scripts, `wrangler.jsonc` and `vercel.json`. Sandbox frames receive their own response CSP with `sandbox allow-scripts`; HTML paths stay exact so their security headers apply without canonical redirects.
