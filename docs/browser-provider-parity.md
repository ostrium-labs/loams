# Browser provider parity

Status: **preview. The suite is written, but it has not yet been run end to end.** The matrix below stays empty until CI has run it, and every cell will name the CI run it came from.

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
| `downloads_policy` | With downloads blocked, a `Content-Disposition: attachment` response is failed at Fetch's response stage and never reaches the page, while an inline response still does. With downloads allowed, the bytes arrive intact (D2123). |
| `network_policy_allowlist` | Loams refuses a navigation to the denied origin before the engine sees it. A subresource request to the denied origin is failed at Fetch's request stage, and that server receives nothing. The allowed origin still works. The cloud metadata address is refused. |
| `cookie_ledger_persistence` | The engine keeps the app's session cookie, and Loams stores it in the profile's ledger. A fresh context gets the cookie back after Loams re-injects it. |
| `profile_restart_persistence` | The cookie survives an engine restart through `--storage-dir` alone. This is **`n/a` on `service`** by design, because the shared engine is not restarted per session and the ledger provides persistence there. |
| `credential_boundary` | A canary Loams token in the job environment never reaches the engine. The suite checks: the engine process's environment on `local` (`/proc/<pid>/environ`, D2122); the container's environment on `service` (a CI step); the cookie jar; page-visible state; every request header and body the fixture received; and the profile directory on disk. |
| `timeouts_budgets` | A 2 s per-call timeout fires on a request that hangs for 45 s. A 3 s Loams-side navigation budget cuts a page that never finishes. Afterwards a fresh page loads normally. |
| `recording` | A screencast of an animated page is recorded through `VideoRecorder` into a WebM file that is not empty. |

A scenario that cannot apply to a provider reports `n/a` and the reason. It is never counted as a pass.

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

Empty until CI has run the suite. Rows are filled only from a `Browser parity: matrix` job summary, with the run linked.

| Scenario | `local` | `service` |
|---|---|---|
| `navigate` | | |
| `accessibility_snapshot` | | |
| `click_type_forms` | | |
| `screenshot` | | |
| `content_extraction` | | |
| `downloads_policy` | | |
| `network_policy_allowlist` | | |
| `cookie_ledger_persistence` | | |
| `profile_restart_persistence` | | |
| `credential_boundary` | | |
| `timeouts_budgets` | | |
| `recording` | | |

CI run: none yet.
