# Getting started with Loams Studio

Studio is the console's second mode ([overview](index.md)). It needs a `loams-agentd`: the local one on the desktop, a remote one in a browser.

## On the desktop

1. **Open Loams Desktop.** On the first run it opens in Studio (Q801). Later it opens in the mode you last used. You can switch with the header's **Console | Studio** control, **Mod+Shift+M**, or the **View** menu's **Studio**, **Console** and **Switch mode** items.
2. **Start the daemon if Studio asks.** Studio works through the local `loams-agentd`. While it looks for the daemon, the home says "Looking for the daemon…". If the daemon isn't running, the home says **Start the daemon** with a **Start** button. If it can't be reached at all, the home says **The daemon is unavailable**: check **Settings › Background agents** in Console mode. That page also decides whether agents keep running after you close the app.
3. **Add a project.** Use **Add** beside the **Projects** heading in Studio's sidebar. On the desktop it opens your OS folder picker: pick a git repository's folder. The project is that repository; its id is its path.
4. **Start a thread.** Click the project's name. Its page has a **New thread** composer: type a prompt and press Enter. By default the first run gets its own git worktree on a new branch. See [Projects and threads](projects-and-threads.md#starting-a-thread) and [The composer](composer.md).

The desktop also gives Studio:
- **Deep links:** `loams://open/studio/t/<threadId>` opens a thread, `loams://open/studio/g/<groupId>` a run group. `loams://open/agent/<id>` opens `/studio/t/<id>`.
- **A tray block:** up to five threads that are running or waiting for an approval, each opening its thread, then **Open Studio**.
- **OS notifications** when a turn finishes or a question is waiting ([Notifications](notifications.md)).
- **Open in editor** ([Settings](settings.md#editor-desktop-only)).
- **A native preview** ([Preview](preview.md)).

## In a browser

The browser console shows the **Console | Studio** switch in every edition. Without a reachable agentd, Studio's home says **Connect an agentd host**. The console ships with your deployment: your engine serves it at `https://loams.<tailnet>.ts.net/ui/`, and Loams Desktop carries it. The hosted console at `console.loams.dev` is retired (design 78); to try Loams without running anything, see [loams.dev/cloud](https://loams.dev/cloud).

**Which console reaches a remote agentd.** A browser console reaches an agentd host only when its Content-Security-Policy lists that host. For the console your engine serves at `/ui/`, set `LOAMS_AGENTD_ORIGINS` on the engine (in the compose stack's `.env`) to the agentd host's exact origin, for example `https://agentd.<tailnet>.ts.net`: the engine sends the console's policy as a header and adds those origins to `connect-src` and `frame-src`. Without it, Studio there reaches no remote agentd. Loams Desktop needs none of this. A console you deploy yourself (the stack's `--profile console`, Cloudflare or Vercel) takes the same variable at deploy time ([details](remote-agentd.md#the-consoles-content-security-policy)).

Studio in the browser needs a remote `loams-agentd`:

1. Run `loams-agentd serve --remote` on a machine you control, on your tailnet ([Remote agentd](remote-agentd.md)), with `--allow-origin` set to your console's origin (for the console your engine serves, `https://loams.<tailnet>.ts.net`).
2. In Studio, open **Settings** (Mod+,) and add the host under **Agentd hosts**, with a pasted access token for now (MT1).
3. **Add** (beside **Projects**) asks for a git URL (`https://`, `ssh://` or `user@host:path`) and clones it on the remote machine. The daemon checks the URL again and refuses local paths, `file:` and `ext::` URLs, and anything that starts with `-`.

**Previews and tokens.** Previews render when the console's `frame-src` lists the agentd host: the console your engine serves at `/ui/` adds the hosts in `LOAMS_AGENTD_ORIGINS` to `frame-src` as well as `connect-src`. **Not yet:** the agentd token is pasted rather than issued by the console's sign-in. See [the console's Content-Security-Policy](remote-agentd.md#the-consoles-content-security-policy) and [Previews](remote-agentd.md#previews).

## Trying Studio without a daemon: the dev preview

The console's dev server can show Studio with sample data. You need Node 22 or later and pnpm (the root `package.json` pins the version):

```sh
pnpm install --frozen-lockfile          # from the repository root
cd web && pnpm dev                      # the console on http://localhost:5173/ui/
```

- `http://localhost:5173/ui/?desktop` is the **desktop edition with a fake bridge** (`web/apps/console/src/cordis/fake-desktop.ts`). Its fake agentd (`fake-agentd.ts`) answers Studio's calls with sample projects, threads, worktrees, a run group and a diff, so every Studio page has something to show. It opens in Studio.
- `http://localhost:5173/ui/?demo` is the **browser edition with an in-browser mock server**, in dev builds only. Studio there has no host until you add one.

Neither runs agents. Both exist only in dev builds (`import.meta.env.DEV`); a production build drops them. The rest of the dev setup (the console API mock, `protoc`) is in [`web/README.md`](../../../web/README.md#develop).
