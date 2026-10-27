# The remote browser provider (removed)

**Superseded on 2026-10-27 by [D2110](design/13-decision-log.md).** On that date the owner gave this direction: "forget Cloudflare, move it to the deploy folder, use Obscura itself".

`loams-web-bridge` used to have a second provider, `BrowserRunProvider`.

- It drove Cloudflare Browser Run over its CDP endpoint, with Chromium by default and Kitesurf as an option.
- It used Cloudflare session guardrails and the free and paid plan limits.
- It was written against Cloudflare's documentation (D565–D567, plan [AP1c](plans/2026-10-03-ap1c-remote-browser-provider.md)) and **was never run against a live account**.

It has been removed: the code, its `[browser_run]` configuration, guardrails and domain sets, and its tests. The engine-neutral pieces are kept in `crates/loams-web-bridge/src/cdp/`: the CDP client, the WebSocket transport, the accessibility-tree parser and the daily budget guard.

The one engine is now Obscura, both locally and in the cloud. See **[The headless browser](headless-browser.md)**.

A configuration that still names `browser-run` is refused at startup with a message that names D2110, rather than being silently ignored.
