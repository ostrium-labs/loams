# The headless browser: one engine, locally and in the cloud

Agents use one browser tool contract:

- `open`, `navigate` and `take_snapshot` (text with uids)
- `find`, `click`, `fill` and `wait_for`
- `take_screenshot` and `extract`
- the WebMCP pair, `list_webmcp_tools` and `call_webmcp_tool`

**One engine sits behind the contract: [Obscura](https://github.com/h4ckf0r0day/obscura)** (Apache-2.0), driven over the Chrome DevTools Protocol by one driver, `loams_web_bridge::ObscuraProvider`. The engine is the same everywhere it runs:

| `LOAMS_BROWSER_PROVIDER` | Where the engine runs | Who starts it |
|---|---|---|
| `local` (default) | the machine the agent runs on, as `obscura serve` on 127.0.0.1 | agentd or Desktop, on the first page |
| `service` | the deployment's `browser` service: compose profile `browser`, or the Kubernetes manifests in [deploy/browser](../deploy/browser/) | the deployment |

Decisions: D2110 (Cloudflare removed, Obscura everywhere), D2111 (the two providers), D2112 (the shared defaults), D2113 (the pin) and D2114 (the packaging). Design: [§37](design/37-desktop-and-mobile-apps.md) §18.5 and §18.14.

**Status: preview.**

- The provider code is checked in CI only against a scripted engine (`crates/loams-web-bridge/tests/it`).
- The run against the real Obscura binary and the real image is lane BR-b's parity suite and CI job. Until that job is green, treat local and cloud browsing as **not yet run end to end**.

## How agents get it

When `LOAMS_BROWSER_PROVIDER` is set, every agent run's MCP server (`loams-agentd mcp`) lists the browser tools (D2115):

- `browser_open`, `browser_navigate`
- `browser_snapshot`, `browser_find`
- `browser_click`, `browser_fill`, `browser_wait_for`
- `browser_screenshot`, `browser_extract`
- `browser_close`

The daemon passes on its own `LOAMS_BROWSER_*` settings, `LOAMS_OBSCURA_BIN` and the service token only when they are set.

- With `local`, each run starts its own engine the first time it opens a page.
- With `service`, every run shares the deployment's `browser` service, and each page gets its own browser context.

## What is the same on every provider

The two providers are one driver with two endpoints, so the following is shared code, not two implementations kept in step (D2112).

| Area | Default | Notes |
|---|---|---|
| Isolation | a browser context per page | Closing a page disposes its context. It never closes the engine, which other agents may share. |
| Network policy | public web only | Private, loopback, link-local and cloud-metadata addresses are refused. The policy is checked before each navigation and on the URL the page lands on: a refused redirect sends the page to `about:blank`. It is also checked on every request the page makes, through `Fetch.requestPaused`. Underneath this, Obscura's own guard refuses private addresses after DNS resolution. |
| Allow and deny lists | none | `allowed_domains` and `denied_domains`, or `LOAMS_BROWSER_ALLOWED_DOMAINS` and `LOAMS_BROWSER_DENIED_DOMAINS`. |
| Credential boundary | secret fills off | `credentials.allow_secret_fills` and `credentials.allowed_fill_hosts` turn them on. A secret is resolved at the moment of the fill and scrubbed from every error and log. |
| Budgets | 4 pages, 60 s per call, 10 min per page, no daily ceiling | `[budgets]`, or the `LOAMS_BROWSER_MAX_SESSIONS`, `…_REQUEST_TIMEOUT_MS`, `…_MAX_SESSION_MS` and `…_DAILY_BUDGET_MS` variables. |
| Recording | off | `LOAMS_BROWSER_RECORD=1` writes `<page>.actions.jsonl` (tool, redacted target, outcome, time) beside the screenshots. A typed value is never recorded. |
| Artifacts | the temp dir | `artifact_dir` or `LOAMS_BROWSER_ARTIFACT_DIR`. Screenshots are files; answers carry a path and a digest, never the bytes. |

Only one capability differs between the providers: `credentials_stay_local` is true for `local` and false for `service`.

## Local

Install the pinned Obscura release on the machine that runs agentd or Desktop, and point `LOAMS_OBSCURA_BIN` at it. If the variable is unset, `obscura` is looked up on `PATH`.

- Use the **unsuffixed** asset for your platform: `obscura-x86_64-linux.tar.gz`, `obscura-aarch64-macos.tar.gz`, `obscura-x86_64-windows.zip`, and so on.
- Never use the `-no-render` assets, which cannot take screenshots.

The process is launched like this:

- it binds 127.0.0.1 on a free port, with a fresh random token, which the provider presents as a bearer token on every connection;
- its environment is cleared except for a short pass-through list, so no `LOAMS_*` credential reaches it;
- it gets no `--storage-dir`;
- it gets `--allow-private-network` only when `allow_private_hosts` is set.

## Service (self-hosted and cloud)

**Compose:**

1. Set `LOAMS_BROWSER_TOKEN` in `.env` to at least 32 random bytes, for example with `openssl rand -hex 32`.
2. Add `browser` to `COMPOSE_PROFILES`.
3. Set `LOAMS_BROWSER_PROVIDER=service`.

The `browser` service publishes no port. It shares an internal network with agentd and has an egress-only network of its own.

**Kubernetes:** apply `deploy/browser/k8s/` (see [its README](../deploy/browser/README.md)). The Service is ClusterIP only, and a NetworkPolicy admits agentd and loams-gateway pods only.

**Provider settings** (TOML or environment):

```toml
[web_bridge]
provider = "service"

[web_bridge.service]
url = "ws://browser:9222/devtools/browser"   # the default
token_secret_ref = "env:browser-token"       # read from LOAMS_WEB_BRIDGE_SECRET_BROWSER_TOKEN
```

`ws://` is accepted only for loopback, private addresses, and single-label or cluster names. Anything else needs `wss://`. A URL with credentials in it is refused.

## The pin

Obscura **0.2.3**, released 2026-09-20 (D2113):

- Image: `docker.io/h4ckf0r0day/obscura:0.2.3@sha256:475def3ddf1ec513b3d1bc36e8ad15f0d192538cb15f814c77215aa70c418ca2`
- Linux tarball digests:
  - x86_64: `sha256:1534d1e6ddaf3d080ec4091eb41d0a4d8cc042a48b607d3c410fc13b482a9eec`
  - aarch64: `sha256:5ecf980bca3060236a7a86ec7ed83d943e6598ee87caa46d20325d90bc75f979`
- The same release as Desktop's sidebar browser (`loams-sidebar-browser`).

## Engine limits to know

Obscura is an independent engine. It is not Chromium, and its output is not pixel-identical to Chromium's.

- It has no `Input.insertText` and no `Browser.setDownloadBehavior`.
- It does implement every CDP method the driver uses. That was checked against the v0.2.3 source on 2026-10-27:
  - `Target.createBrowserContext`, `createTarget`, `attachToTarget`, `closeTarget`, `disposeBrowserContext`
  - `Accessibility.getFullAXTree`
  - `DOM.resolveNode`
  - `Runtime.evaluate`, `callFunctionOn`, `releaseObject`
  - `Page.navigate`, `captureScreenshot`
  - `Fetch.enable`, `continueRequest`, `failRequest`
