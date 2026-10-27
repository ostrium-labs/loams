# Privacy policy

This policy covers the Loams software in this repository: the `loams` engine and CLI, the Docker
Compose stack, Loams Desktop, the SDKs and the console. It also covers the two services Ostrium Labs
runs for the preview: the sign-in service at `auth.loams.dev`, and, until it was retired, the
hosted console at `console.loams.dev`.

**Short version:** Loams collects no telemetry or analytics. The software you run sends your data
only where you tell it to. The services we run see only what they need to serve a page or sign
you in.

## The software you run

- **No telemetry.** The engine, the CLI, the Compose stack, the SDKs and Loams Desktop send no usage
  data, crash reports or analytics to Ostrium Labs or anyone else.
- **Your data stays where you put it.** Documents, vectors, tables, graphs, logs and backups live in
  the object store and databases you configure: your bucket, your machines, your tailnet. Loams
  connects only to the services in your configuration, such as your bucket, your connectors' source
  and sink systems, and your identity provider.
- **Observability is opt-in.** Metrics, traces and logs go only to endpoints you configure, for
  example your own OpenTelemetry collector.

## The one automatic network request: desktop update checks

Packaged Loams Desktop builds check for updates. They download the update manifest from this
repository's GitHub releases (the `desktop-latest` release) about 30 seconds after the app starts,
every 6 hours after that, and when the computer wakes. Windows and AppImage builds then download and
install the update. macOS and `.deb`/`.rpm`/`.pacman` installs only show a "Download" link.

The request goes to GitHub and carries what any download carries: your IP address and a user
agent. It carries no account, no usage data and nothing about your engines or data. GitHub's own
privacy statement covers that request.

You can turn the check off. In **Settings › Updates**, untick **Check for updates automatically**
(on by default). While it is off the app makes no update request on its own: the start-up, 6-hour
and wake-up checks are not scheduled. The **Check now** button still works and makes the request
only when you press it. The choice is saved as `update.autoCheck` in the app's `settings.json`.

To turn updates off entirely, start the app with the environment variable
`LOAMS_DESKTOP_NO_UPDATE_CHECK=1`. That overrides the setting: the app makes no update request at
all, including from **Check now**, and the setting is shown as off and cannot be changed. There is
no policy file for this yet. Builds you compile yourself (unpackaged) never check.

## Services we run

### Hosted console (`console.loams.dev`, retired)

`console.loams.dev` is retired: it redirects to `loams.dev/cloud`. From 0.1.2 the console ships
with your deployment (your engine serves it) and with Loams Desktop, so no Ostrium service is
involved in loading it. This section describes the hosted console while it ran.

- **What it was:** a static web app served from Cloudflare. Your browser downloads the app and then
  talks **directly to your own engine**, over your tailnet or at the address you enter. Your queries,
  results and data never pass through Ostrium Labs.
- **What is stored:** the engine address you enter is kept in your browser's local storage. We
  receive nothing about it.
- **Hosting logs:** Cloudflare, as the host, processes standard request data (IP address, user agent,
  requested path) to serve and protect the site. We don't add analytics or tracking.

### Sign-in (`auth.loams.dev`)

You only use this if you choose **Loams account** sign-in. Tailscale sign-in and API keys don't
touch it.

- **What it stores:** your account: email address, name, the hashed credentials or the identity
  provider link you sign in with, and sign-in records kept for security.
- **What it shares:** it issues signed tokens with your identity to the Loams engines **you** connect
  to. It shares nothing else with anyone.
- **Your control:** you can ask for your account to be exported or deleted (see Contact).

## Children

Loams is developer and infrastructure software and isn't directed at children.

## Changes

Changes to this policy are made in this file, so the git history shows every version. Material
changes are noted in the release notes.

## Contact

Privacy questions or requests (account export or deletion): open a
[GitHub Discussion](https://github.com/ostrium-labs/loams/discussions), or for anything sensitive,
use the private reporting channel in [SECURITY.md](SECURITY.md).

_Last updated: 2026-10-15._
