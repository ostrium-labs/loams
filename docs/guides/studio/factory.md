# The factory in Studio

The software factory's apps belong to Studio (design [§53](../../design/53-studio-ade.md) §5.9, D916; SD1 Task 27): Forgejo, Zulip, ItsAPlan (Plane), GlitchTip, OpenPanel, Matomo (Q813), Langfuse and SigNoz (Preview: SigNoz on Loams House).

## Where they are

**On the desktop and in the browser.** The apps' connections live in the agent daemon, `loams-agentd`, on the machine it runs on (SF1 amendment F1, §50 §13). Loams Desktop and the browser console both read the panels from it:

- **On the desktop**, the factory reads through the local daemon and shows each app's full UI inside Studio.
- **In the browser**, the factory reads through the agentd host selected in **Studio settings › Agentd hosts**. **Open app** opens the app in a new tab (Plane's App URL where set), and an app's settings are read-only: its credentials are entered in Loams Desktop on the daemon's machine. A browser never sends a credential to a daemon.

- **The factory hub** is at `/studio/factory`, with each app's panels at `/studio/factory/<app>`, its full UI at `/studio/factory/<app>/app` and its settings at `/studio/factory/<app>/configure`. The command palette (**Ctrl K**) reaches it as **Software Factory**; you can also pin it to the sidebar's Pages.
- **Studio's sidebar** has a **Factory** group, below the projects when the daemon runs and, on the desktop, on its own when it doesn't: one line with how many apps are connected, which opens to list the apps with their state, linking to their panels, to the full UI (SigNoz has the full UI only; it is labelled "Preview: SigNoz on Loams House") or to configure. Its icon opens the hub. The count updates as you configure apps.
- **In Console mode**, the overview keeps the factory's card.

**Old addresses still work.** `/factory`, `/factory/<app>`, `/factory/<app>/app` and `/factory/<app>/configure` redirect to their `/studio/factory…` addresses, keeping the query, and replace the history entry so Back doesn't bounce. `loams://open/factory[/<app>]` opens `/studio/factory[/<app>]` directly.

The factory's credentials are kept by the daemon in the OS keyring (service `dev.loams.agentd`) and never reach a page or a reply; the rest of an app's settings is in the daemon's `factory.json`. Without a keyring (a headless session) they last until the daemon stops, and the factory says so. Credentials that an earlier Loams Desktop kept move into the daemon the first time it runs. The panels are read-only (§37 §19), and every error the daemon returns is scrubbed of the credentials.

## Connecting the apps

**Save and test** (on the desktop) checks the connection. For SigNoz, which has no panels and no credentials, it checks that `/api/v2/healthz` answers at the URL's origin.

**Sign-in inside the app.** In the full UI, pages on the app's origin stay in Studio and every other site opens in your browser. If the app signs in through a single sign-on provider, put the provider's origin (for example `https://sso.example.com`) in **SSO origin**, so the sign-in pages stay in Studio too.

**Plane (ItsAPlan).** Its URL is the API's (for example `https://api.plan.example.com`). ItsAPlan serves its web UI on another origin: put that in **App URL**, and Open app, a new window and Open in browser load it.

**Langfuse.** The recent traces panel reads Langfuse's v2 observations API (Langfuse 4, or 3.x on its v4 data model). On a Langfuse 3.x without it, the panel reads the older trace list instead.

**SigNoz (Preview: SigNoz on Loams House).** SigNoz opens as its full UI only, with no native panels. Its URL is the one you run it at (for example `http://127.0.0.1:3301`, when you start it from `deploy/factory/signoz/compose.yaml`). Its telemetry is stored in your Loams House, not in a separate ClickHouse. It is a preview: SigNoz on House is the House's third gate application (D1571) and is not passing yet. It is not in the default compose, so you start it yourself. The MIT community build has no single sign-on, so it sits behind forward-auth with one service user.

## Threads from factory objects

- **Plane: Start thread** on an issue. Studio's home shows a **New thread from Plane** card with the issue's key, title and priority, and a branch named after the key.
- **GlitchTip: Fix this error** on an issue. The card holds the latest event's stack trace (innermost frame first, up to 40 frames and 4 000 characters) and release.

The hand-off is an address, `/studio?from=plane|glitchtip&title=…&prompt=…&branch=…&link=…`, and Studio checks every field: a known source, bounded text, a git-safe branch and an http(s) link.

**Not built yet:** the card doesn't fill the composer. It shows the draft for you to copy into a project's **New thread** composer.

## Pull requests

On a thread, when the branch has an open pull request on Forgejo, the [git toolbar](git.md) links it through the factory. **Not built:** "Post result to Zulip", which needs a write with an approval (§39 §8). The factory's operations are read-only.
