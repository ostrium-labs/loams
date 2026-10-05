# The console's browser harness

The first browser harness in this repository, and deliberately the smallest one
that answers the question the vitest suite cannot (AP1d Task 4; design §42 §5,
D635, D638).

## What it is for

`web/apps/console/src/webmcp/index.ts` feature-detects WebMCP on
`document.modelContext`. That attribute is `[SecureContext]` and it does not
exist in jsdom, so `test/webmcp.test.ts` can only ever prove that the console
agrees with an object the test itself built. That is worth having, and it is not
the same thing.

Here, a real Chrome is launched with Chrome's local-development WebMCP flag, a
fixture page is served from loopback so the page is in a secure context, and the
module is loaded through TypeScript's own transpiler — so the test runs the
module as written, not a bundled copy.

## The honest scope

Chrome's local-development switch exposes `navigator.modelContext` and
`navigator.modelContextTesting`. It does **not** expose `document.modelContext`.
Measured on Chrome 149.0.7827.155 on 2026-10-05, with `--enable-features=WebMCPTesting`
and with `--enable-features=WebMCP` as well. Reaching the real attribute needs an
origin-trial token for the console's origin, which CI cannot assume.

So the load-bearing assertion is the **absent**-API one: `detectModelContext`
reports `not-exposed` and `WebMcpTools.register` resolves `undefined`, in a real
browser, while the API's near-miss spellings are sitting right there on
`navigator`. That is worth more than a passing test against the testing surface,
because it is also the path that matters in production — WebKit's standards
position on WebMCP is closed and `oppose` (D635), so no version of AP1d may
assume the API arrives.

A test that asserted `'modelContextTesting' in navigator` would be green and
would never have touched the code the console ships.

## Why hand-rolled

No Playwright, no Puppeteer, no browser binary in the lockfile:

- nothing new to download for any other job, and no engine to keep current;
- `src/webmcp/index.ts` is transpiled from source on every run, so this cannot
  pass against a stale bundle;
- the whole of the DevTools protocol used is `Target.createTarget` and
  `Runtime.evaluate`.

## Running it

```sh
cd web
pnpm --filter @loams/console test:browser
```

It is not part of `pnpm test`, and not part of the `web` CI job: the
`web-browser` job runs it. Both are deliberate — a browser harness should not be
able to hold up a lint or typecheck run, and it needs a Chrome binary the other
jobs do not ask for.

Without a Chrome or Chromium binary the suite **skips** locally and **fails** on
CI. A browser job that skips itself reports green having tested nothing.

## Environment

| Variable | Effect |
|---|---|
| `LOAMS_CHROME` | The binary to test in. An explicit value that is not executable is an error, not a fallback. |
| `LOAMS_WEB_MCP_FLAGS` | Comma-separated Chrome flags. Defaults to `--enable-features=WebMCPTesting,DevToolsWebMCPSupport`. |
| `LOAMS_CHROME_HEADLESS` | `0` runs headful, for debugging with DevTools attached. |
| `LOAMS_CHROME_EXTRA_ARGS` | Space-separated extra Chrome arguments. |

## Files

| File | What it holds |
|---|---|
| `harness.mjs` | Chrome discovery and launch, the CDP client, the loopback fixture server, and the TypeScript transpile. |
| `fixture/index.html` | The page. Loaded over loopback, never `file:`, so it is a secure context. |
| `fixture/page.mjs` | The three scenarios: the untouched document, the native API if present, and a spec-shaped `document.modelContext` on the real document. |
| `webmcp-detect.test.mjs` | The assertions. |

`fixture/page.mjs` labels its own context as a fixture wherever it reports one.
A spec-shaped fake is not Chrome's implementation and is never claimed to be.