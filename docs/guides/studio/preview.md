# Preview

The **Preview** tab of a thread's right panel shows the web app the thread is working on, from a dev server running on the daemon's machine (design [§53](../../design/53-studio-ade.md) §5.8, Q806; SD1 Task 26). **Mod+Shift+J** opens and closes it.

## Where previews come from

`loams-agentd` discovers HTTP dev servers running under a project and gives each a stable address through its local preview proxy: `http://<name>.<project>.localhost:7331`. The Preview tab lists them under **Local servers**, with an address bar, Back, Forward, Reload and **Open in browser**.

Start the dev server yourself, in a [terminal](terminal.md) or through the agent. The tab's own "start the dev server" button runs a project action, which needs an agentd space; Studio's projects are plain repositories for now, so the button stays hidden.

## On the desktop

The preview is a native view that Electron main places over the panel (D678's shape):
- **Its own session** (`persist:studio-preview`), with no preload, sandboxed, no Node in subframes and no `<webview>`.
- **Only http(s) on this machine loads in it** (`localhost`, `*.localhost`, 127.0.0.0/8, `::1`, no user info in the URL). Any other http(s) link in the page opens in your browser, and other subframes are blocked.
- **Popups** on this machine open in the view, never as windows.
- **Permissions and downloads.** The only permission granted is a sanitised clipboard write, and downloads come only from this machine.
- **Ctrl+L** (Cmd+L on macOS) returns focus to the console.

## In a browser

The preview is the remote agentd's `/preview/…` path in a sandboxed frame:
- **The frame:** `sandbox="allow-scripts allow-forms allow-modals allow-downloads"`, plus `allow-same-origin` only when the preview's origin differs from the console's, `allow=""`, and no referrer.
- **The address:** the console first asks the daemon for a one-preview ticket and frames `/preview/pt_<ticket>/`. Your token is never put in a URL and never reaches the dev server.
- **Base paths:** the daemon strips the `/preview/<…>/` prefix before the dev server sees the request. An app that uses root-relative URLs (`/assets/…`) needs its dev server's base path set to that prefix.
- **HMR:** WebSockets pass through.

The details, and the headers that keep other sites from framing a preview, are in [Remote agentd](remote-agentd.md#previews).

**Note:** the console's Content-Security-Policy must allow framing your agentd host. The console your engine serves at `/ui/` adds the hosts in `LOAMS_AGENTD_ORIGINS` to `frame-src` as well as `connect-src`, so its previews render once your host is listed; so do a console you deploy with your host in `LOAMS_AGENTD_ORIGINS`, and the dev server ([details](remote-agentd.md#the-consoles-content-security-policy)).

**When the daemon can't serve previews** (an older daemon, or previews off), the tab shows how to forward the preview proxy's port instead, then open the preview locally:

```sh
ssh -N -L 7331:localhost:7331 <host>
```

## Not carried over from T3 Code

Zoom, colour-scheme and devtools controls, recent URLs, favicons (no remote images, D919), the mini player, annotation and capture.
