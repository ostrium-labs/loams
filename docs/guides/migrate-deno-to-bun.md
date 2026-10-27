# Moving from Deno to Bun

Loams doesn't support Deno, as of 2026-10-23 ([design 79](../design/79-void-and-vite-plus.md), D1790). This guide helps you move a Deno app or tool to Bun, and from Deno Deploy to hosting you own.

**Why now.** On 2026-10-09, the Deno team announced that it is [joining Cloudflare](https://deno.com/blog/cloudflare):
- "We will support the Deno runtime for another year with monthly releases containing bug fixes and security updates. After that year we will end our development of the Deno runtime."
- "Deno Deploy will continue operating for six months before shutting down."
- "JSR will continue operating, with its infrastructure moving to Cloudflare."
- Deno stays open source.

Deno Deploy Classic (dash.deno.com) already shut down on 2026-07-20 ([Deno's migration guide](https://docs.deno.com/deploy/migration_guide/)).

Your Deno code keeps running for now. But a runtime whose own team is winding it down isn't a safe place for new work. Bun runs TypeScript directly, implements most Node.js APIs, and has a fast toolchain. Most Deno apps move in an afternoon.

Facts about Bun below were read from [Bun's docs](https://bun.com/docs) on 2026-10-23, for Bun 1.4.4. Facts about Deno are from [Deno's docs](https://docs.deno.com). If you're reading this much later, check both.

---

## 1. Install Bun, and the Loams toolchain

```sh
curl -fsSL https://bun.com/install | bash   # or: npm install -g bun
bun --version
```

If your project is part of a Loams workspace, or you want one toolchain for checks, tests and builds, add [Vite+](https://viteplus.dev). Loams uses it everywhere (design 79):

```sh
bun add -d vite-plus
```

---

## 2. Config: `deno.json` becomes `package.json` (plus `tsconfig.json`, `bunfig.toml` and `vite.config.ts`)

Deno keeps everything in `deno.json`. Under Bun, the same settings live in a few standard files:

| `deno.json` field | Where it goes under Bun |
|---|---|
| `tasks` | `package.json` `scripts`. Run them with `bun run <name>` |
| `imports` (the import map) | `package.json` `dependencies`, plus `tsconfig.json` `compilerOptions.paths` or `package.json` `imports` (`#`-prefixed subpath imports) for your own aliases. Bun supports both (Bun docs, "Module resolution") |
| `compilerOptions` | `tsconfig.json` |
| `fmt`, `lint` | `vite.config.ts` `fmt` and `lint` sections (Vite+), or `.oxfmtrc.json` and `.oxlintrc.json` |
| `test` | `bunfig.toml` `[test]`, or `vite.config.ts` `test` (Vitest) |
| `nodeModulesDir` | Not needed. Bun always installs into `node_modules` |
| `lock` | `bun.lock` (written by `bun install`) |
| `unstable` flags | Usually no equivalent. Check each API below |

**A minimal `package.json`:**

```json
{
  "name": "my-app",
  "type": "module",
  "scripts": {
    "dev": "bun --watch src/main.ts",
    "start": "bun src/main.ts",
    "test": "bun test",
    "check": "vp check",
    "build": "bun build ./src/main.ts --compile --bytecode --minify --sourcemap --format=esm --outfile dist/my-app"
  },
  "dependencies": {},
  "devDependencies": { "@types/bun": "latest", "vite-plus": "1.3.0" }
}
```

Pin exact versions in real projects. `latest` is shown only for brevity.

**`bunfig.toml`** is optional. Use it for Bun-specific settings such as registries and test options. Bun also reads `.npmrc` (Bun docs, ".npmrc support").

---

## 3. Imports: `jsr:`, `npm:` and URL imports

Bun resolves imports like Node: bare package names from `node_modules`, `node:` built-ins, relative paths, and `tsconfig` paths. Bun's module-resolution docs don't describe inline `jsr:` or `npm:` specifiers, or `https://` imports in source (checked 2026-10-23). **Rewrite them to bare package names.**

| Deno | Bun |
|---|---|
| `import { z } from "npm:zod@3"` | `bun add zod`, then `import { z } from "zod"` |
| `import { join } from "jsr:@std/path"` | `bunx jsr add @std/path`, then `import { join } from "@std/path"` |
| `import x from "https://deno.land/x/foo@1.0/mod.ts"` | Find the package on npm or JSR and add it. For an old `deno.land/x` module with no package, vendor the file into your repo |
| `import { serve } from "https://deno.land/std/http/server.ts"` | Not needed. Use `Bun.serve` (§4) |
| `import * as path from "node:path"` | Unchanged |

**JSR packages work in Bun through JSR's npm compatibility layer** ([JSR: using packages](https://jsr.io/docs/using-packages)):
- `bunx jsr add @luca/cases` adds the package as `"npm:@jsr/luca__cases@^1.0.1"` in `package.json`.
- It writes a `.npmrc` with `@jsr:registry=https://npm.jsr.io`. Commit that file.
- You import it by its JSR name: `import { camelCase } from "@luca/cases"`.

---

## 4. APIs

Most Web APIs are the same in both: `fetch`, `Request`, `Response`, `URL`, `crypto.subtle`, streams and `WebSocket`. The `Deno.*` namespace is what changes. Where you can, prefer the `node:` API: it works on Bun, Node and many other hosts.

### HTTP servers: `Deno.serve` becomes `Bun.serve`, or Hono for portability

```ts
// Deno
Deno.serve({ port: 8000 }, (req) => new Response("hello"));

// Bun
Bun.serve({ port: 8000, fetch: (req) => new Response("hello") });
```

For code that should run on Bun, Node, Cloudflare Workers and Loams Functions, write a [Hono](https://hono.dev) app, or a [Void](https://void.cloud) app (Hono underneath). Then pick the server per target:

```ts
import { Hono } from "hono";
const app = new Hono().get("/", (c) => c.text("hello"));
export default app; // Bun serves a default export with a fetch method
```

### Environment: `Deno.env` becomes `process.env` or `Bun.env`

| Deno | Bun |
|---|---|
| `Deno.env.get("KEY")` | `process.env.KEY` (or `Bun.env.KEY`) |
| `Deno.env.set("KEY", "v")` | `process.env.KEY = "v"` |
| `Deno.args` | `process.argv.slice(2)` |
| `Deno.exit(1)` | `process.exit(1)` |
| `Deno.cwd()` | `process.cwd()` |
| `import.meta.main` | Unchanged. Bun supports it |

Bun loads `.env` files automatically (Bun docs, "Environment variables"). Deno needed `--env-file` for that.

### Files: `Deno.readFile` and friends become `Bun.file` or `node:fs`

| Deno | Bun |
|---|---|
| `await Deno.readTextFile(p)` | `await Bun.file(p).text()` |
| `await Deno.readFile(p)` | `await Bun.file(p).bytes()` |
| `await Deno.writeTextFile(p, s)` | `await Bun.write(p, s)` |
| `await Deno.stat(p)` | `await import("node:fs/promises").then((fs) => fs.stat(p))` |
| `Deno.readDir(p)` | `fs.readdir(p, { withFileTypes: true })` from `node:fs/promises` |
| `await Deno.remove(p, { recursive: true })` | `fs.rm(p, { recursive: true })` |

See Bun docs, "File I/O".

### Subprocesses: `Deno.Command` becomes `Bun.spawn`

```ts
// Deno
const { stdout } = await new Deno.Command("git", { args: ["status"] }).output();

// Bun
const proc = Bun.spawn(["git", "status"], { stdout: "pipe" });
const out = await new Response(proc.stdout).text();
await proc.exited;
```

`Bun.spawnSync` is the blocking form. `node:child_process` works too (Bun docs, "Child processes").

### KV: Deno KV becomes Loams, SQLite or Cloudflare KV

Deno KV (`Deno.openKv()`) was local SQLite under the CLI, and a hosted database on Deno Deploy. The hosted one goes away with Deploy. Pick by what you need:

| You need | Use |
|---|---|
| A local or single-server key-value store | `bun:sqlite`: `new Database("kv.db")`, a `kv(key TEXT PRIMARY KEY, value TEXT, version INTEGER)` table, `INSERT … ON CONFLICT` for set, `WHERE version = ?` for atomic checks (Bun docs, "SQLite") |
| Shared state for an app on Loams | Loams Live tables (keyed documents, reactive queries) through `@loams/client`. Redis clients will work against Loams' RESP gateway once it ships (design 77; not built yet) |
| Portable app state across clouds | The `items` port of `@loams/app` (design 71): keyed JSON documents with conditional writes, backed by D1, DynamoDB, Firestore, Cosmos DB or Loams |
| An app on Cloudflare | Workers KV (eventually consistent; for read-heavy data), or a Durable Object's SQLite storage (strongly consistent per object) |

Deno KV's `atomic()` checks map to a version column (SQLite), `ifVersion` (`@loams/app` items), or a Durable Object.

### Cron: `Deno.cron` becomes `Bun.cron`

```ts
// Deno
Deno.cron("cleanup", "0 * * * *", async () => { await cleanup(); });

// Bun: in-process, while the server runs
Bun.cron("0 * * * *", async () => { await cleanup(); });

// Bun: an OS-level job (crontab, launchd or Task Scheduler) that runs a script
await Bun.cron("./cleanup.ts", "0 * * * *", "cleanup");
```

See Bun docs, "Cron". In a Void app, put jobs in `crons/`. On Loams' targets, the deploy wires them to each cloud's scheduler (design 79 §7.3).

### WebSockets

```ts
// Deno
Deno.serve((req) => {
  const { socket, response } = Deno.upgradeWebSocket(req);
  socket.onmessage = (e) => socket.send(e.data);
  return response;
});

// Bun
Bun.serve({
  fetch(req, server) {
    if (server.upgrade(req)) return; // the upgrade took over
    return new Response("expected a websocket", { status: 400 });
  },
  websocket: { message(ws, msg) { ws.send(msg); } },
});
```

See Bun docs, "WebSockets". Bun adds publish and subscribe topics (`ws.subscribe`, `server.publish`).

---

## 5. Tooling

| Deno | Bun, or Vite+ |
|---|---|
| `deno run main.ts` | `bun main.ts` |
| `deno run --watch` | `bun --watch main.ts` |
| `deno task build` | `bun run build` |
| `deno test` | `bun test` (Jest-style `test`/`expect` from `bun:test`), or `vp test` (Vitest) |
| `deno fmt` | `vp fmt` (Oxfmt) |
| `deno lint` | `vp lint` (Oxlint). `vp check` runs format, lint and type checks together |
| `deno check` | `vp check`, or `tsc --noEmit` |
| `deno compile` | `bun build --compile` (see below) |
| `deno bench` | `bun:test`'s timing, or Vitest's `bench` |
| `deno doc` | TypeDoc, or JSR's generated docs if you publish there |

**Tests.** `Deno.test("name", fn)` becomes `test("name", fn)` from `bun:test`. Deno's `assertEquals(a, b)` from `@std/assert` becomes `expect(a).toEqual(b)`. You can also keep `@std/assert`: add it with `bunx jsr add @std/assert`.

**`deno compile` becomes `bun build --compile`, with bytecode.** Both make one executable that includes the runtime. Bun's version:

```sh
bun build ./src/main.ts --compile --bytecode --minify --sourcemap --format=esm --outfile dist/my-app
# cross-compile: add --target=bun-linux-x64 | bun-linux-arm64 | bun-linux-x64-musl | bun-linux-arm64-musl |
#                     bun-darwin-x64 | bun-darwin-arm64 | bun-windows-x64 | bun-windows-arm64
```

What the flags do, and the limits (Bun docs, "Single-file executable" and "Bytecode caching"):
- **`--bytecode`** precompiles your code at build time, so the binary starts faster: "1.5-2x faster" for a small CLI, "2-4x faster" for a large app, per Bun's own measurements.
  - Bytecode is tied to the Bun version that built it. On a mismatch, Bun parses the source instead. That is slower, but it still works.
  - It isn't obfuscation.
- **`--bytecode-order=<profile>`** (Bun 1.4.3) lays out the bytecode from a recorded run, for a faster cold start.
- **The binary includes the Bun runtime,** so expect tens of megabytes. The Linux x64 runtime alone is about 77 MB in Bun 1.4.
- **Native `.node` addons** can be embedded, but each is built for one platform.
- **`deno compile --include`** becomes `bun build --compile --asset <path>` (Bun 1.4.0). Read embedded files with `Bun.file()`.
- **Licence:** Bun statically links JavaScriptCore, which is LGPL-2. If you distribute the binary, read Bun's `LICENSE.md` first.

---

## 6. Security: permissions don't carry over

Deno runs code with no file, network or environment access unless you grant it (`--allow-read`, `--allow-net`, `--allow-env` and so on). **Bun has no permission flags.** A Bun process can do whatever its user account can do. Bun's docs describe no permission model (checked 2026-10-23).

If you relied on Deno's permissions, enforce the same limits outside the runtime:
- **Containers.** Run as a non-root user, with a read-only root file system and writable `tmpfs` or volume paths only where needed. Drop all capabilities (`--cap-drop=ALL`), set `no-new-privileges`, and limit memory, CPU and PIDs. Use a network policy or an egress proxy in place of `--allow-net=<hosts>`.
- **Loams.** Code that Loams runs for you (agents through `loams-agentd`, Functions, House workers) runs inside the Loams sandbox ([design 56](../design/56-loams-sandbox.md)). That is a `SandboxSpec` of file-system binds, network egress, syscalls and resource limits. On Linux it uses namespaces, cgroups, Landlock and seccomp. That is stricter than Deno's flags, because it holds even for native code and subprocesses.
- **A plain Linux host.** Use systemd's sandboxing directives (`ProtectSystem=strict`, `ReadWritePaths=`, `PrivateTmp=`, `IPAddressAllow=`/`IPAddressDeny=`, `NoNewPrivileges=`) on the service unit.
- **Environment.** Pass only the variables the app needs. Never mount a whole `.env` of unrelated secrets.

---

## 7. Hosting: off Deno Deploy

Deno Deploy shuts down about six months after 2026-10-09. Pick a home you control:

| Your app | Move it to |
|---|---|
| A server you want to own | **Self-host on your own machines (the ground, BYOG)** with Loams' light compose stack. Build a Bun-compiled image (§5) and run it in the `apps` profile beside Loams (design 79 §7.4, D1791). Reach it privately on your tailnet, or publish it through Cloudflare Tunnel |
| A Workers-style app (a `fetch` handler, edge, little CPU per request) | **Void on Cloudflare,** in your own Cloudflare account: `void deploy`, or `loams app deploy --target cloudflare` (design 79 §7.1). The Deno team is moving to Cloudflare, and Deno's post offers paying Deploy customers migration help to Workers |
| An app that must run on AWS, Google Cloud, Azure or Vercel | **Loams app BYOC** ([design 71](../design/71-app-byoc-free-tiers.md)): the same app on each cloud's free tier. Containers (Cloud Run) run the Bun binary; Lambda, Azure Functions and Vercel run Node |
| An app that needs Loams features (retrieval, Live, Durable, Postgres) | **A Loams data plane in your own cloud account** ([design 70](../design/70-byoc-clouds.md)), with the app beside it |

Deno Deploy's environment variables, custom domains and cron jobs don't move by themselves. Recreate each one on the new host. Deno KV data needs an export (§4).

---

## 8. Checklist

- [ ] Install Bun (and Vite+ if you want `vp check` and `vp test`).
- [ ] Turn `deno.json` into `package.json` (scripts, dependencies), `tsconfig.json` and, if needed, `bunfig.toml`.
- [ ] Replace `npm:` and `jsr:` specifiers and URL imports with bare names. Run `bun add` and `bunx jsr add`, and commit `.npmrc` if JSR wrote one.
- [ ] Replace `Deno.*` calls: `serve`, `env`, `args`, `exit`, file APIs, `Command`, `openKv`, `cron` and `upgradeWebSocket`. Search with `grep -rn "Deno\." src`.
- [ ] Move `Deno.test` to `bun:test` or Vitest, then run `bun test` or `vp test`.
- [ ] Replace `deno fmt` and `deno lint` with `vp fmt` and `vp lint`, or `vp check`.
- [ ] Replace `deno compile` with `bun build --compile --bytecode`. Test the binary on every target platform.
- [ ] Replace the permission flags with container, systemd or Loams sandbox limits.
- [ ] Export Deno KV data and load it into its new store.
- [ ] Recreate environment variables, secrets, domains and cron schedules on the new host.
- [ ] Remove `deno.json`, `deno.lock` and Deno from CI.

## 9. Pitfalls

- **Missing permissions don't fail anymore.** Code that Deno refused to run (no `--allow-net`) now just runs. Add the outside limits from §6 before you deploy.
- **`.env` loads by itself.** Bun reads `.env`, `.env.local` and `.env.<NODE_ENV>` automatically. A stray `.env` in the working directory can change your config. For compiled binaries, `--no-compile-autoload-dotenv` turns this off.
- **Top-level `await` in a module that something `require()`s** fails under Bun's CommonJS interop. Use `import` or dynamic `import()` (Bun docs, "Module resolution").
- **Deno-only packages** that call `Deno.*` internally won't work. Look for a Node-compatible version, or a JSR package that doesn't use `Deno.*`.
- **Bytecode and Bun upgrades.** A binary built with one Bun version and run with a different one parses its source again. That is slower, but correct. Rebuild after each Bun upgrade.
- **Native addons and cross-compiling.** A `.node` addon built on your Mac won't load in a Linux binary. Build the addon for each target, or use a pure-JavaScript alternative.
- **Big binaries.** Compiled binaries carry the runtime. Use `--minify` and a minimal base image, and don't ship `node_modules` beside the binary.
- **Timezones and cron.** `Bun.cron.parse` uses the system's local time zone unless you pass `{ tz }`. Deno Deploy ran cron in UTC. Set `TZ=UTC`, or pass `{ tz: "UTC" }`.

## Sources (read 2026-10-23)

- **Deno:**
  - [Deno is joining Cloudflare](https://deno.com/blog/cloudflare);
  - [Cloudflare's post](https://blog.cloudflare.com/deno-joins-cloudflare/);
  - [the Deno Deploy migration guide](https://docs.deno.com/deploy/migration_guide/);
  - [Deno's docs](https://docs.deno.com) (configuration, permissions, `deno compile`).
- **Bun:**
  - [Single-file executable](https://bun.com/docs/bundler/executables) and [Bytecode caching](https://bun.com/docs/bundler/bytecode);
  - [Cron](https://bun.com/docs/runtime/cron), [SQLite](https://bun.com/docs/runtime/sqlite), [File I/O](https://bun.com/docs/runtime/file-io), [Child processes](https://bun.com/docs/runtime/child-process) and [WebSockets](https://bun.com/docs/runtime/http/websockets);
  - [Environment variables](https://bun.com/docs/runtime/environment-variables), [Module resolution](https://bun.com/docs/runtime/module-resolution) and [.npmrc support](https://bun.com/docs/pm/npmrc);
  - [Bun v1.4.0](https://bun.com/blog/bun-v1.4.0) and [Bun v1.4.3](https://bun.com/blog/bun-v1.4.3).
- **JSR:** [using packages](https://jsr.io/docs/using-packages).
- **Loams:** designs [56](../design/56-loams-sandbox.md), [70](../design/70-byoc-clouds.md), [71](../design/71-app-byoc-free-tiers.md), [77](../design/77-kv-gateways.md) and [79](../design/79-void-and-vite-plus.md).
