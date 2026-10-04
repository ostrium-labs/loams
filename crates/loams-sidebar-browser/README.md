# loams-sidebar-browser

The docked sidebar browser for Loams Desktop (design
[§37 §18.5](../../docs/design/37-desktop-and-mobile-apps.md), decision
**D620**): [Obscura](https://github.com/h4ckf0r0day/obscura) driven over the
Chrome DevTools Protocol, with a persistent profile per
`(environment, app)` and a credential boundary that keeps Loams tokens out of
the engine.

The `sidebar-browser` cargo feature is **on by default**.

## What this is, and what it replaces

SF1 Task 0 concluded that the sidebar browser "does not ship on any operating
system" (ruling **E1**), because the per-platform webview it would have been
built on had an ephemeral website-data store on both platforms, no WebAuthn on
the GTK and WPE WebKit ports, and no Windows implementation at all. **D620
supersedes E1.** This crate is what ships instead.

**No webview is forked, vendored or embedded.** That was not a cost decision:

- **WKWebView** and **WebView2** are proprietary and cannot be forked.
- Forking **WebKit** is a multi-year effort.

A separate, Apache-2.0 process speaking a documented protocol is the only shape
that is both licence-clean and one code path for Linux, macOS and Windows.

| E1's blocker | Closed by | Where |
|---|---|---|
| Ephemeral store on every platform | The engine's `--storage-dir`, keyed per `(environment, app)`; Loams keeps its own cookie ledger beside it and re-injects on the next launch | [`profile`](src/profile.rs), [`ledger`](src/ledger.rs) |
| No WebAuthn in WebKitGTK | Loams runs the OIDC/Authentik ceremony itself with its own OIDC client; the engine is handed only post-auth scoped session cookies and never performs a ceremony | [`session`](src/session.rs) |
| No Windows browser at all | There is no per-platform webview left to be missing: one command line covers all three targets | [`engine`](src/engine.rs) |

## Install the engine

The engine is **not** a Rust dependency. `obscura-cdp` is not published to
crates.io (the `obscura` name there is an unrelated 2019 crate), so the only
supported integration is the release binary over CDP.

Pinned version: **0.2.3** (`engine::PINNED_ENGINE_VERSION`). Point
`LOAMS_OBSCURA_BIN` at the binary you install.

| Target | Asset to download |
|---|---|
| linux x86_64 | `obscura-x86_64-linux.tar.gz` |
| linux aarch64 | `obscura-aarch64-linux.tar.gz` |
| macOS Apple silicon | `obscura-aarch64-macos.tar.gz` |
| macOS Intel | `obscura-x86_64-macos.tar.gz` |
| Windows x86_64 | `obscura-x86_64-windows.zip` |

> **Do not use the `-no-render` assets.** Obscura builds four variants per
> target, and the `-no-render` ones lack the `render` cargo feature.
> `Page.startScreencast`, `Page.stopScreencast`, `Page.captureScreenshot` and
> `Page.printToPDF` are compiled behind it and answer
> *"requires a build with the render feature"* otherwise — the whole frame
> path is dead. The **unsuffixed** assets are the render-enabled ones.

## Fidelity: read this before you promise anyone a browser

**This is an agent-driven surface. It is not a general-purpose human-interactive
browser.**

Obscura is headless with a software rasteriser. There is no window, no
compositor and no GPU surface to host a real webview in. What the panel does is:

- frames arrive from `Page.startScreencast` as base64 PNG and go into the GPUI
  panel as an image;
- interaction goes back as `Input.dispatchMouseEvent` and
  `Input.dispatchKeyEvent`.

What that costs, concretely:

- **No scroll momentum, no caret, no text selection, no compositor-driven
  animation at 60 Hz.** A frame is emitted when the engine's activity
  generation changes; there is no frame clock.
- **Every frame is a full-page raster plus a base64 round trip.** The panel
  drops frames rather than queueing them when it falls behind, and reports how
  many it dropped.
- **No WebAuthn in the engine.** Not a gap to work around: Loams does the
  ceremony, so the page never asks the engine for one.
- **Not pixel-identical to Chromium.** Obscura is an independent, evolving
  engine. It reaches high WPT-equivalent coverage by Cloudflare's measurement,
  but "passes the tests" is not "renders identically".

### Why that is acceptable here, and what would falsify it

The embedded targets are **Zulip, Plane and Forgejo** — controlled, known
applications, not the open web:

- Their login is a server-rendered HTML form, so the post-auth cookie injection
  is the whole sign-in story and there is no client-side auth ceremony to
  render.
- Their pages are server-rendered with progressive enhancement, not apps built
  against a specific engine's layout or event quirks.
- All three ship `SameSite=Lax` sessions and none sends a CSP
  (SF1 Task 0, Decision 2), so there is no per-app header rewrite to keep in
  sync with the engine.

That is a real bound on the fidelity risk, and it is a bound on *these* apps, not
a general one. What would falsify it: an app whose UI depends on a Chromium-only
behaviour (a `:has()`/container-query interaction, a WebGL or `<canvas>`
visualisation, a `content-visibility` layout assumption), or a target outside
this set. If Loams later embeds something like that, the honest fallback is
`AppOpener` into the system browser, which is what D620 leaves in place — the
engine path is additive, and nothing removes the system-browser route.

## Recording a product demo

`record::VideoRecorder` turns the frames the panel already receives into one
playable video file, so a demo of a Zulip thread or a Plane issue can be sent to
someone who does not have Loams installed (**D628**).

```rust
use loams_sidebar_browser::{Container, RecorderConfig, VideoRecorder};

let recorder = VideoRecorder::new(
    RecorderConfig::new(demo_dir.join("zulip-thread.webm"))
        .with_container(Container::WebM),
);

recorder.start().await?;                     // AlreadyRecording if already running
// ... on each decoded frame, with the moment it arrived:
recorder.write_frame(&frame, std::time::Instant::now()).await?;
// ... when the user presses stop:
let summary = recorder.stop().await?;       // None if nothing was recording
```

### What it guarantees

- **Starting twice is safe.** The second `start` returns
  `StartOutcome::AlreadyRecording` and touches nothing: no second scratch
  directory, no second screencast, no second encoder. The transition happens
  under the same lock `stop` takes, so racing callers cannot both see `Idle`.
- **Stopping always works, including after a failed start, and stopping an idle
  recorder is `Ok(None)`** rather than an error. The screencast release in
  `stop_all` cannot fail the stop: a dead engine still leaves the caller wanting
  the video.
- **The output is always a finished, playable container, or there is no output
  file at all.** Frames are written to a scratch directory beside the output;
  finalisation encodes them to `<output>.loams-partial.<ext>`, fsyncs it, and
  **renames** it into place. Nothing else ever writes to the output path. A
  zero-frame recording publishes nothing and says so, because a zero-length
  video is not a short video.
- **The failure path produces the same file as the graceful path.** The frames
  are still on disk when the recorder is dropped, so `Drop` runs the same
  finalisation. This is the reason for the whole design: a streaming encoder
  that gets killed leaves an unrepairable container.
- **Every timestamp is monotonic.** Frame holds come from `std::time::Instant`
  deltas, never the wall clock, so a clock step cannot produce negative frame
  lengths or a duration players disagree about.
- **Nothing is left behind.** Stop, error and drop all delete the scratch
  directory and remove the partial. `tests/recording.rs` asserts no leftover for
  every terminal path.

### The fidelity limits, stated rather than discovered

- **Video only. No audio, ever.** No microphone, no system audio, no narration
  track. The engine has no audio output and the screencast carries none.
- **No editing and no post-processing.** Nothing is trimmed, re-timed,
  speed-changed, colour-graded, annotated or composited. What the page rendered
  is what the file contains. *(Note one internal exception, which is a
  correctness requirement rather than an edit: frames are scaled to even
  dimensions for `yuv420p`, at most one pixel, in the encoder and never on the
  PNGs on disk.)*
- **The recording is only as good as the panel's frames**, so everything under
  "Fidelity" above applies: no scroll momentum, no caret, no text selection,
  coarser-than-60 Hz updates, and not pixel-identical to Chromium.
- **The frame rate is a cap, not a target.** Frames arriving inside the
  interval are dropped and counted (`RecordingSummary::dropped`) rather than
  queued, so a fast animation is recorded as sampled frames.
- **A still page is a still frame.** The engine is activity-driven and emits
  nothing while a page is idle, so a quiet stretch is a held frame. Real holds
  mean it is held for the *right* length, but `max_gap` (default 2s) clamps a
  single frame's hold so a stalled agent or a closed laptop lid does not become
  a frozen minute.

### `ffmpeg` is an optional runtime dependency

Encoding shells out to `ffmpeg`, resolved like the engine is: `LOAMS_FFMPEG_BIN`
wins, otherwise it is found on `PATH`. **The sidebar browser does not need
it** — a machine without `ffmpeg` gets a working panel, and only `start` is
refused, with an error naming the variable. The encoder the chosen container
needs is probed at `start`, so an `ffmpeg` built without `libvpx` fails
immediately with the list of what it does have, rather than after a two-minute
recording.

Why an external binary rather than a pure-Rust encoder, in short: a pure-Rust
path needs three new dependencies (PNG decode, encode, mux) and its only
encoder-quality option is `rav1e`, which is heavyweight and slow on flat UI
content. `ffmpeg` adds zero Rust dependencies, so `cargo deny` has nothing new
to check and the `libvpx`/`libx264` licence questions attach to a distribution
of ffmpeg rather than to Loams. It also means the file can actually be verified:
`tests/recording.rs` opens the output with `ffprobe` and asserts on the stream
it reports. The costs are disk for the recording's length and a pause at stop
while encoding runs.

## The credential boundary

SF1's Global Constraint is **"embeds never hold a Loams token."** E1 could claim
that constraint was easy to keep precisely because an ephemeral store retains
nothing. A persistent profile is the opposite case, so here the constraint is
enforced rather than inherited:

1. **The engine is never asked to authenticate.** Loams runs the OIDC/Authentik
   ceremony itself; only the resulting app session cookies cross into
   `ScopedSessionCookie`, which is the only type in the crate that crosses.
2. **Cookies are host-only and origin-scoped by construction.**
   `ScopedSessionCookie::new` has **no `domain` parameter** — the CDP payload
   carries `url`, never `domain`, which is also what Zulip's `__Host-sessionid`
   prefix requires (SF1 ruling E6). The `__Host-` and `__Secure-` prefixes are
   enforced, not merely observed.
3. **The invariant is audited, not asserted.**
   `boundary::audit_profile` walks the whole profile directory — including the
   engine's own `cookies.json`, which this crate did not write — for the token
   in its raw and base64 forms, and `boundary::assert_cookies_exclude` does the
   same over `Storage.getCookies`. `tests/token_boundary.rs` runs both against
   a real profile directory holding a real token, **including the negative
   cases**: the audit has to fail when something is actually wrong, or it
   proves nothing.

A naive persistent profile would have violated all of this: point the engine at
a persistent directory with no injection boundary and the app's own login flow
runs inside the embed, whatever the page writes to its jar and its
`localStorage` lands on disk forever — and the Loams console session the desktop
already holds for the API would sit in the same jar one bad redirect away.

## Private networks and SSRF

Obscura refuses loopback, RFC1918, link-local and IPv6 unique-local targets by
default, enforced **after** DNS resolution so a public name resolving to a
private address is refused too. That default is wrong for Loams: a development
console runs on `http://127.0.0.1:8084` or `https://console.localhost:8443`, and
every page would render an SSRF error.

`net_policy` decides per origin, and the launch spec follows it exactly:

- a public origin launches the engine **without** `--allow-private-network`, so
  the engine's SSRF guard is exactly as the engine ships it;
- a loopback or RFC1918 origin launches it **with**
  `--allow-private-network`.

Two things this crate adds on top of the engine:

- **A hard-deny set the switch does not open.** `169.254.169.254` and the rest of
  link-local, plus the unspecified and broadcast addresses, are refused by Loams
  whatever the engine's flag says. The most valuable member is the cloud
  metadata endpoint, which is inside the range `--allow-private-network`
  relaxes. `SidebarBrowser::prepare` refuses such an origin before any process
  could be spawned, so there is no state in which a refused target has an engine.
- **The flag is process-wide, so the per-request control lives in the allowlist.**
  One `obscura serve` serves every origin the page can navigate to, so relaxing
  it for a localhost console relaxes it for everything that console can link to.
  `navigate::NavigationAllowlist` is what confines it: the embedded origin plus
  the identity provider's, nothing else, with zeron's three rules (`http`/`https`
  only, host required, no userinfo) on top.

Loams passes the **flag**, not the `OBSCURA_ALLOW_PRIVATE_NETWORK` environment
variable. Both work upstream; the flag is per-process and survives command
pipelines, while the variable is inherited by every child process the desktop
spawns.

## Wiring it to GPUI

The desktop (`loams-ui-collab`, SF1 Task 3) owns the panel; this crate owns
everything below it.

```rust
use loams_sidebar_browser::{EmbeddedOrigin, ProfileKey, SidebarBrowser, SidebarBrowserError};

# fn main() -> Result<(), SidebarBrowserError> {
// 1. Derive the profile, decide the network policy, refuse anything refused.
let browser = SidebarBrowser::prepare(
    data_dir,
    ProfileKey::new("env_dev", "zulip")?,
    EmbeddedOrigin::parse("https://chat.example.com")?,
    loams_sidebar_browser::resolve_program(),
    loams_sidebar_browser::DEFAULT_CDP_PORT,
)?
.allow_origin(EmbeddedOrigin::parse("https://sso.example.com")?);

// 2. Launch the engine and connect.
let engine = loams_sidebar_browser::ObscuraEngine::start(
    browser.plan().spec().clone(),
    loams_sidebar_browser::DEFAULT_STARTUP_TIMEOUT,
).await?;
let mut client = loams_sidebar_browser::CdpClient::connect(&engine.devtools_url()).await?;

// 3. Inject the app's scoped session cookies — never a Loams token.
browser.inject(&mut client, &session_id, &cookies_from_the_oidc_ceremony).await?;

// 4. Pump frames and route input.
let mut pump = loams_sidebar_browser::FramePump::new(loams_sidebar_browser::FrameFormat::Png);
// ... on each Page.screencastFrame: pump.handle(&mut client, &event).await
// ... on each click:  PanelInput::PointerDown { .. }.dispatch(&mut client, &session_id).await
# Ok(()) }
```

`PanelState::take_frame` is what the GPUI side calls once it has drawn, and it
reports how many frames were dropped in the meantime — that number is how a
"the sidebar feels laggy" report becomes a measurement instead of an opinion.

## Tests

106 tests, no engine binary required (9 of them — everything in
`tests/recording.rs` that needs an encoder — also need `ffmpeg` and `ffprobe`
on `PATH`; they print a SKIP and return if either is absent, so a green run
without an encoder is never mistaken for proof of playback):

| File | What it holds |
|---|---|
| `tests/isolation.rs` | distinct `(environment, app)` pairs get distinct directories; hostile identifiers are refused, not sanitised |
| `tests/persistence.rs` | a session survives a process restart; a rotation replaces rather than accumulates; a corrupt or future ledger is an error, never a silent logout |
| `tests/token_boundary.rs` | a Loams token is caught in the profile directory in every shape, **and a clean profile passes**; the cookie jar is checked; the library only ever compares a token |
| `tests/frames.rs` | the decode path: dimensions from `IHDR`, malformed payloads refused, JPEG refused with an actionable message, pixels never re-encoded |
| `tests/net_policy.rs` | localhost consoles are permitted with the engine relaxed; public origins are not; the metadata endpoint is refused by Loams |
| `tests/launch_spec.rs` | one platform-neutral command line for all three operating systems; the version pin; `--allow-file-access` never set |
| `tests/wire_contract.rs` | the CDP contract against a fake engine: frames decode and are acknowledged with an integer session id, input becomes the right parameter objects, events are buffered, engine errors keep their message |
| `tests/recording.rs` | recording against a fake engine and a synthetic frame source: a double start opens no second screencast; stopping an idle recorder is a no-op; a failed start leaves nothing behind; frames are acknowledged with an **integer** `sessionId`; the cadence comes from monotonic timestamps and counts what it dropped; a burst is sampled rather than queued; a long silence is clamped; dropping the recorder still publishes; abandoning publishes nothing; and **`.webm` and `.mp4` are opened by `ffprobe`**, which reports the codec, the dimensions, the frame count and a duration matching the recorded holds |

```console
$ cargo test -p loams-sidebar-browser -j 4
```

## Licence

Apache-2.0, the repository's licence. Obscura is Apache-2.0 and is consumed as a
separate process over a protocol; no Obscura source is vendored here.