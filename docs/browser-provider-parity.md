# Browser provider parity

Status: **preview.** CI has run the suite once against each provider. The matrix below reports that run, and nothing else.

Loams' built-in headless browser must behave the same whether it runs locally or in the cloud (owner, 2026-10-27). There is one engine, [Obscura](https://github.com/h4ckf0r0day/obscura) **0.2.3** (Apache-2.0), and two ways to reach it:

| Provider | What it is | How the suite reaches it |
|---|---|---|
| `local` | The engine binary, launched on the user's machine by the Desktop or by agentd. It binds loopback. | The suite launches the pinned binary itself, through `loams_sidebar_browser::ObscuraEngine`. Each scenario gets a fresh engine and profile directory. |
| `service` | The same engine as a `browser` service in compose and k8s, from the pinned image. | The suite connects to the running service's DevTools WebSocket with `Authorization: Bearer <OBSCURA_CDP_TOKEN>`. Each scenario gets its own browser context. |

Cloudflare Browser Run was removed (owner, 2026-10-27) and is not part of this suite.

## The suite

It lives in `crates/loams-sidebar-browser/tests/provider_parity/` (decision D2120). One test binary runs the same scenarios against each selected provider. Every page comes from an HTTP server inside the test process on 127.0.0.1, with two origins: one allowed and one denied. No scenario touches the public internet.

| Scenario | What passing means |
|---|---|
| `navigate` | Two navigations land on the right URL with the right title, and the fixture saw both requests. |
| `accessibility_snapshot` | The page exposes `heading "Fixture login"`, `link "Docs"`, `textbox "Email"`, `textbox "Note"` and `button "Sign in"`. The report says which path produced them: `Accessibility.getFullAXTree`, or a DOM-derived snapshot when the engine has no Accessibility domain. Obscura 0.2.3's README does not list one. |
| `click_type_forms` | Real `Input.dispatchMouseEvent` clicks and `Input.dispatchKeyEvent` typing fill two inputs. Submitting posts the form, and the server receives it. |
| `screenshot` | `Page.captureScreenshot` returns a PNG of at least 320×160. Its decoded pixels are not blank, and the fixture's red block is visible. |
| `content_extraction` | `innerText` contains the fixture's sentence and heading, and `outerHTML` contains the form. |
| `downloads_blocked` | After `Browser.setDownloadBehavior {behavior: "deny"}`, navigating to an attachment produces no `downloadWillBegin` event and writes no file. On `local` the suite checks the profile directory; on `service` CI checks the container with `docker diff` (D2126). |
| `downloads_allowed` | After `Browser.setDownloadBehavior {behavior: "allow", downloadPath}`, the same navigation produces a `downloadWillBegin` event or a file holding the attachment's bytes. On Obscura 0.2.3 it reports **unsupported**, which is neither a pass nor a failed run. See *Downloads* below. |
| `network_policy_allowlist` | Loams refuses a navigation to the denied origin before the engine sees it. A subresource request to the denied origin is failed at Fetch's request stage, and that server receives nothing. The allowed origin still works. The cloud metadata address is refused. |
| `cookie_ledger_persistence` | The engine keeps the app's session cookie, and Loams stores it in the profile's ledger. A fresh context gets the cookie back after Loams re-injects it. |
| `profile_restart_persistence` | The cookie survives an engine restart through `--storage-dir` alone. This is **`n/a` on `service`** by design, because the shared engine is not restarted per session and the ledger provides persistence there. |
| `credential_boundary` | A canary Loams token in the job environment never reaches the engine. The suite checks: the engine process's environment on `local` (`/proc/<pid>/environ`, D2122); the container's environment on `service` (a CI step); the cookie jar; page-visible state; every request header and body the fixture received; and the profile directory on disk. |
| `timeouts_budgets` | A 2 s per-call timeout fires on a request that hangs for 45 s. A 3 s Loams-side navigation budget cuts a page that never finishes. Afterwards a fresh page loads normally. |
| `recording` | A screencast of an animated page is recorded through `VideoRecorder` into a WebM file that is not empty. |

A scenario that cannot apply to a provider reports `n/a` and the reason. A capability the engine does not implement reports `unsupported`. Neither is ever counted as a pass.

## Running it

```sh
# local: the pinned binary
LOAMS_BROWSER_PARITY_PROVIDERS=local LOAMS_OBSCURA_BIN=/path/to/obscura \
  cargo test -p loams-sidebar-browser --test provider_parity -- --test-threads=1 --nocapture

# service: a running browser service
LOAMS_BROWSER_PARITY_PROVIDERS=service \
LOAMS_BROWSER_SERVICE_URL=ws://127.0.0.1:9222/devtools/browser \
LOAMS_BROWSER_SERVICE_TOKEN=... \
  cargo test -p loams-sidebar-browser --test provider_parity -- --test-threads=1 --nocapture
```

| Variable | Meaning |
|---|---|
| `LOAMS_BROWSER_PARITY_PROVIDERS` | `local`, `service`, or both, comma-separated. When unset, every scenario prints *skipped* and passes, so the workspace test run is unaffected. |
| `LOAMS_BROWSER_PARITY_REQUIRE=1` | Missing configuration (no binary, no URL, no `ffmpeg`) fails the scenario instead of skipping it. CI sets this. |
| `LOAMS_BROWSER_PARITY_REPORT` | A file that receives one JSON line per provider and scenario. The canary and the service token are redacted. |
| `LOAMS_BROWSER_PARITY_CANARY_ENV` | The name of the env var that holds the canary token. |
| `LOAMS_OBSCURA_BIN`, `LOAMS_FFMPEG_BIN` | The engine and encoder binaries for `local` and `recording`. |
| `LOAMS_BROWSER_SERVICE_URL`, `LOAMS_BROWSER_SERVICE_TOKEN` | The `service` endpoint and its bearer token. |

`scripts/browser/parity-matrix.py report-*.jsonl` renders the reports as the matrix below.

## CI

`.github/workflows/browser-parity.yml` (D2124) is report-only and never a required check. It runs on a pull request or a push to dev that touches the browser crates, `deploy/browser` or the workflow, and on manual dispatch.

- **`local`** downloads Obscura 0.2.3's unsuffixed Linux asset, checks the sha256 from the GitHub release, and points `LOAMS_OBSCURA_BIN` at it.
- **`service`** runs `h4ckf0r0day/obscura:0.2.3@sha256:475def3d…` with host networking and a random 64-hex `OBSCURA_CDP_TOKEN`.
  - It also passes `--allow-private-network`. That flag is only there because the fixtures are on the runner's loopback, and the shipped service does not set it.
  - A CI step then checks that the container's environment holds no Loams token and no `LOAMS_*` variable.
- **`matrix`** merges both reports into the job summary.

What the suite does not cover yet:

- **The `BrowserProvider` contract.** This is the engine-level layer: CDP plus Loams' policy pieces. The contract-level layer drives `loams_web_bridge::BrowserProvider` through `ObscuraProvider` local and service, and it follows once that provider lands.
- **The compose and k8s manifests themselves.** The CI service job runs the pinned image directly, which is the same image the manifests pin. Once `deploy/browser` exists, the job will switch to bringing up the compose `browser` service.

## Parity matrix

Filled only from CI. Source: [run 40619354478](https://github.com/ostrium-labs/loams/actions/runs/40619354478), commit `aa287923`, Obscura 0.2.3, ubuntu-24.04 runners, 2026-10-28.

| Scenario | `local` | `service` |
|---|---|---|
| `navigate` | pass | pass |
| `accessibility_snapshot` | pass (via `Accessibility.getFullAXTree`) | pass (same) |
| `click_type_forms` | pass | pass |
| `screenshot` | pass (1280×720) | pass (1280×720) |
| `content_extraction` | pass | pass |
| `downloads_blocked` | pass | pass (plus `docker diff`) |
| `downloads_allowed` | **FAIL** (now reported as unsupported) | **FAIL** (now reported as unsupported) |
| `network_policy_allowlist` | pass | pass |
| `cookie_ledger_persistence` | pass | pass |
| `profile_restart_persistence` | pass | n/a |
| `credential_boundary` | pass (process env included) | pass (container env checked by CI) |
| `timeouts_budgets` | pass | pass |
| `recording` | pass | pass |

In that run, 12 of 13 scenarios passed on `local` and 11 of 13 on `service`. On `service`, `profile_restart_persistence` is `n/a` by design.

The one failing scenario, `downloads_allowed`, failed identically on both providers because of the engine gap described below. The suite now reports it as `unsupported`, not as a failure. That change takes effect from the next run.

Earlier runs:

- [40539985305](https://github.com/ostrium-labs/loams/actions/runs/40539985305) and [40583789342](https://github.com/ostrium-labs/loams/actions/runs/40583789342) failed the old Fetch-based `downloads_policy` scenario on both providers, and `profile_restart_persistence` on `local`.
- The restart failure came from the suite itself. It shut the engine down while its own CDP connection was still open, and the engine had not yet written that connection's cookies. See *Cookie persistence on `local`* below.

### Downloads: unsupported on Obscura 0.2.3

The engine does not download. This is from Obscura's v0.2.3 source (`obscura-cdp/src/domains/browser.rs` and `page.rs`), confirmed by the CI runs above:

- `Browser.setDownloadBehavior` and `Page.setDownloadBehavior` are acknowledged and ignored ("Obscura does not download files to disk").
- There are no `downloadWillBegin` or `downloadProgress` events and no `Browser.cancelDownload`.
- `Fetch.enable` with `requestStage: "Response"` is accepted, but requests are paused only at the request stage, with no response headers.

So:

- **Blocked downloads hold on both providers**, because nothing is ever written. `downloads_blocked` checks this on each provider: the profile directory on `local`, and `docker diff` on the service container.
- **Allowed downloads cannot be honoured on either provider.** `downloads_allowed` reports `unsupported` and does not pass. If a future engine release starts emitting download events or writing files, the scenario will pass without any change to the suite.
- A page script can still read an attachment's bytes with `fetch()`. No policy on this engine can stop that by response header (D2125, D2126).

### Cookie persistence on `local`

Obscura 0.2.3 (`obscura-cdp/src/server.rs`) behaves as follows:

- It merges a CDP connection's default-context cookies into `--storage-dir/cookies.json` only when that WebSocket connection closes.
- On SIGTERM it saves only what had already been merged.
- Contexts from `Target.createBrowserContext` are never persisted.

A launcher that kills the engine, or stops it with the connection still open, therefore loses the session. To restart, close the CDP connection, then send SIGTERM, then kill. The suite does this, and `ObscuraProvider` should too.
