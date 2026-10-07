![Loams — Your data. Your bucket.](../../../../docs/assets/loams-banner.svg)

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

## The invariant, not one browser's answer

**The API's availability moves with the Chrome channel.** Measured 2026-10-05
under the same `--enable-features=WebMCPTesting,DevToolsWebMCPSupport`:

| surface | Chrome 149.0.7827.155 | Chrome 154.0.8037.57 |
|---|---|---|
| `document.modelContext` | **absent** | **present** (a real `ModelContext`) |
| `navigator.modelContext` | **present** | absent |
| `navigator.modelContextTesting` | **present** | absent |

On 154 the real attribute is present with **no feature flags at all**. So the
job asserts that the console **agrees with whatever the browser reports**, in
both directions:

- **absent** → `detectModelContext` reports `not-exposed` and
  `WebMcpTools.register` resolves `undefined` — D568's path, and with WebKit's
  standards position `oppose` (D635) the production one;
- **present** → detection is available, `register` returns a handle, `exposedTo`
  carries only potentially-trustworthy origins, and sign-out unregisters **one
  abort per tool**.

Three things are asserted on **every** channel, and they are why this job is
worth having at all:

- the `navigator` decoys are **never** mistaken for `document.modelContext`.
  On 149 the decoys are present and the API is not, so that build rules out a
  detection reporting "available" off `navigator` alone; on 154 the API is
  present and the decoys are not, and an identity check that the detected
  context **is** `document.modelContext` rules out the rest. Each channel
  catches half of the mistake. A test that asserted
  `'modelContextTesting' in navigator` would be green and would never once have
  touched the code the console ships;
- a plaintext `http://` origin never reaches `registerTool` — compared as parsed
  origins, not by substring, so `https://agent.example.attacker.test` is not
  accepted for `https://agent.example`;
- `clear()` is idempotent, and a partial registration failure keeps the working
  tools.

The seam properties above run against a **spec-shaped `ModelContext`** installed
on the real document, because they are properties of the module and no browser
reports its own `registerTool` arguments back. Where the build already has a
native API the fixture shadows it and **records that it did**
(`shadowedNative`) — it does not skip, because skipping is what made an earlier
version of this suite assert nothing at all on exactly the build CI runs.

Every assertion holds whether or not the flags still work, which is checked
both ways — flags on, and flags explicitly disabled, on both channels. A Chrome
that renames or drops the features changes the *diagnostics*, not the result:
each run logs `typeof document.modelContext`, whether the `navigator` surfaces
exist and what they carry, the `tools` permissions policy, and the resolved
browser version, so a channel change is visible in the CI log rather than only
as a red build.

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

To run it against a specific build — which is the point, since the answer moved:

```sh
LOAMS_CHROME=/path/to/chrome pnpm --filter @loams/console test:browser
```

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

## What is primary, and what is not

Primary: Chrome's own WebMCP page naming the local switch
`chrome://flags/#enable-webmcp-testing` and the origin trial running from 149,
and Chromium's `runtime_enabled_features.json5`, where `WebMCPTesting` exists as
`status: "experimental"` and sits in the `implied_by` list of the origin-trial
feature `WebMCP`.

Not primary: the `--enable-features=WebMCPTesting` spelling and the name
`navigator.modelContextTesting` both came from a vendor integration doc, and no
primary page states them — that doc's `getTools()` on the testing surface is
wrong, it is `listTools()`. `DevToolsWebMCPSupport` appears on no primary page
and is measurably inert on Chrome 149 (byte-identical surface set with and
without it); it is kept and labelled rather than quietly dropped. The
two-channel measurement settles that availability *moves*; it does not establish
why, and no primary page was found stating the 154 gate. D638 says all of this
rather than implying a source that was never read.
