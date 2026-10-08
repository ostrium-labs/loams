# Writing a plugin

A plugin is two things: a **manifest** that describes it, and a **loader** that
says how to bring it up and take it down. Neither the router, the shell, the
console nor the A2A layer needs to know your plugin exists.

All the types below are in `core/host/src/types.ts`, which is frozen
contract — other packages code against those field names.

If you are writing a plugin for the Loams console specifically, note that the
catalog in `apps/server/src/plugin-catalog.ts` is where a plugin is registered.
An adapter that registered _itself_ would make load order a function of import
order, so registration is always the composition root's job.

## The manifest

```ts
import type { PluginManifest, PluginAgentSkill } from "@loams-core/host";

export const weatherManifest: PluginManifest = {
  id: "weather",
  name: "Weather",
  description: "Read-only weather observations from the internal service.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/weather",
  icon: "cloud",
  order: 40,
  defaultEnabled: false,
  requiredScopes: ["weather:read"],
  upstream: { product: "Weather", envPrefix: "WEATHER" },
  agent: {
    name: "Weather Agent",
    description: "Reads observations from the internal weather service.",
    version: "1.0.0",
    skills: WEATHER_SKILLS,
  },
};
```

Field by field:

| field            | required | meaning                                                                              |
| ---------------- | -------- | ------------------------------------------------------------------------------------ |
| `id`             | yes      | Unique, and the path segment in `/api/plugins/:id` and `/.well-known/agent-card/:id` |
| `name`           | yes      | Human label, also the console sort key of last resort                                |
| `description`    | yes      | Shown in the console and in the agent card                                           |
| `version`        | yes      | Your plugin's version                                                                |
| `uiPath`         | yes      | Where the plugin's own page is served. `/` means "this is the product"               |
| `category`       | no       | Free-form grouping label                                                             |
| `icon`           | no       | Icon name                                                                            |
| `alwaysOn`       | no       | `true` ⇒ the console toggle is disabled and the registry refuses to disable it       |
| `order`          | no       | Console sort key, lower first. Defaults to 100                                       |
| `defaultEnabled` | no       | Enabled state for a plugin that has never been toggled                               |
| `requiredScopes` | no       | Scopes a caller must hold before this plugin's routes or skills answer               |
| `upstream`       | no       | `{ product, envPrefix }` — metadata for the console                                  |
| `agent`          | no       | The agent this plugin advertises, with its skills                                    |

### `alwaysOn`

`alwaysOn` is load-bearing, not cosmetic. It is what makes the host load the
plugin at boot, what makes the registry throw if you try to disable it (the API
turns that into a 409), and what disables the console toggle. The dashboard and
control plane are the two plugins in this repository that use it: without a
dashboard there is no front door, and without the control-plane service the
original REST routes — which read `ctx.controlPlane` directly — have nothing to
read.

Do not reach for `alwaysOn` to mean "important". It means "cannot be turned off".

### `requiredScopes`

`requiredScopes` is enforced **per request**, not at enable time. The host wraps
every route the plugin registers in a guard derived from this field, and the A2A
layer checks it before dispatching to a skill.

Per-request is the only correct point. A session created before your plugin was
deployed predates it and cannot have been granted its scopes, so a check at
enable time would wave it straight through. Where the caller lacks the scopes:

- a plugin's route answers `403` with code `insufficient_scope` and the missing
  scope names,
- `GET /api/plugins` reports that plugin as `blocked` with `missingScopes`, for
  that session only,
- `POST /api/plugins/:id/enable` returns the blocked status rather than switching
  the plugin on.

Do not implement your own scope check. The host's guard is the enforcement point,
and a second check in your handler is a second thing to get wrong. The full
model — including admin gating and per-agent service tokens for `message:send` —
is in [security.md](security.md).

## The loader

All four hooks are optional, and all four are torn down on disable.

```ts
import type { PluginLoader } from "@loams-core/host";

export const weatherLoader: PluginLoader = {
  service: WeatherService,
  config: { baseUrl: process.env.WEATHER_URL },
  attach(runtime) {
    runtime.ctx.logger.info("weather: attached");
    return () => runtime.ctx.logger.info("weather: detached");
  },
  routes(runtime) {
    return [{ method: "GET", match: "/api/plugins/weather/observations", handler: handler }];
  },
  skills(runtime) {
    return [{ id: "listWeatherObservations", handle: listObservations }];
  },
};
```

### The lifecycle

On enable, in this order:

1. `loader.service` is attached with `ctx.plugin(service, loader.config)`. The
   returned fiber is retained so it can be disposed.
2. `loader.attach(runtime)` runs. If it returns a function, that function is the
   disposer.
3. `loader.routes(runtime)` is called and every returned spec is registered with
   the mutable route table — wrapped in the scope guard if you declared
   `requiredScopes`. The returned remover is retained.
4. `loader.skills(runtime)` is called and every handler is subscribed to the bus
   under your plugin id. The unsubscribers are retained.

On disable, the reverse:

1. routes are removed,
2. bus subscriptions are dropped,
3. the service fiber is disposed,
4. any disposer from `attach` runs.

Three properties you can rely on:

- **It is idempotent.** Enable → disable → enable produces one set of handlers,
  not two. The failure mode this guards against is invisible until the second
  toggle.
- **It is isolated.** A loader that throws marks _your_ plugin `error`, rolls back
  whatever it had already done, and lets boot continue for every other plugin.
  The REST layer turns that into a 200 with `state: "error"` and the message,
  because one broken adapter must not make the console look globally broken.
- **Teardown failures are logged, not thrown.** A plugin that refuses to unload
  cleanly must not block the others.

`PluginRuntime` is what the hooks are handed:

```ts
interface PluginRuntime {
  id: string;
  manifest: PluginManifest;
  ctx: Context; // the root cordis context
  router: HttpRouter; // register routes here; they disappear on unload
  bus: AgentBus; // subscribe skills here; they disappear on unload
}
```

The runtime exists so a loader never reaches for a global. Note that cordis
resolves an inherited context key only on a context that declared it in
`inject`: reading a service you have not declared throws rather than returning
`undefined`, so guard optional reads in a `try`/`catch` rather than with a
truthiness check.

## Routes

A route is a `RouteSpec` from `core/host/src/router.ts`:

```ts
{
  name: "weather:observations",          // free-form label, used in logs and tests
  method: "GET",                          // or an array; defaults to "*"
  match: "/api/plugins/weather/observations", // exact string, or a RegExp on the pathname
  handler: async ({ req, path, url, sendJson, readJson }) => {
    const { limit } = url.searchParams;
    sendJson(200, { observations: await listObservations(limit) });
    return true;                            // truthy claims the request
  },
}
```

A handler returns a truthy value to claim the request and a falsy value to let
the next route try. Routes are consulted in registration order, after the
ConnectRPC bridge and the thirteen original REST routes, so a plugin can never
shadow either — and a plugin that has been disabled falls straight through to the
server's 404.

Add a `guard` only if you need something the scope guard does not cover; the
host supplies the `requiredScopes` guard for you. A guard may return `false` to
decline (the route falls through) or throw to answer with a specific status — the
router honours an `httpStatus` property on the thrown value.

## UI pages

The manifest's `uiPath` is where the console will link to. The dashboard app
routes are:

| route          | page               |
| -------------- | ------------------ |
| `/`            | the dashboard      |
| `/plugins`     | the plugin list    |
| `/plugins/:id` | that plugin's page |
| `/console`     | the plugin console |

To give a plugin a real screen, add one entry to
`core/host/src/ui/pluginPages/registry.ts`:

```ts
import type React from "react";
import { WeatherPage } from "./WeatherPage";

export const pluginPages: Record<string, React.ComponentType<{ plugin: PluginStatus }>> = {
  dashboard: DashboardPluginPage,
  weather: WeatherPage,
};

export const fallbackPluginPage = PluginOverviewPage;
```

That map is the whole extension point — no change to the router, the shell or
the pages. Anything absent from it falls through to the overview page, which
renders the manifest: name, description, state and skills. `uiPath` is treated as
advisory (a trailing slash is trimmed, a missing leading slash added, a blank or
`"/"` value falls back to `/plugins/<id>`), so it will never produce a dead link.

## Agent skills and the bus

Skills are declared on the manifest and implemented by the loader. The two must
agree on the id: a `PluginAgentSkill.id` with no handler means an advertised agent
that cannot answer, which is why the catalog refuses to auto-enable an adapter
whose package exports a manifest but no loader.

```ts
export const WEATHER_SKILLS: PluginAgentSkill[] = [
  {
    id: "listWeatherObservations",
    name: "List observations",
    description: "Recent observations from the internal weather service.",
    tags: ["weather", "read"],
    examples: ['listWeatherObservations {"limit":10}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];
```

```ts
skills: () => [
  {
    id: "listWeatherObservations",
    handle: async (params, api) => {
      api.progress("fetching", { limit: params.limit });
      return { observations: await listObservations(Number(params.limit) || 10) };
    },
  },
];
```

`handle` receives `(params, api)`. `api` is an `AgentSkillContext`:

| field                      | meaning                                                                                                                                |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `api.ctx`                  | the cordis context — read platform services off this                                                                                   |
| `api.message`              | the `AgentMessage` that triggered the call                                                                                             |
| `api.progress(name, data)` | emit a progress artifact; the A2A layer collects these into `Task.artifacts`, and over the raw bus they become `agent/progress` events |

Use `api.ctx`, never `this`. A skill handler is a plain method on an object
literal, so `this` is the handler record and not your plugin's context.

Return value handling is forgiving: returning `{ data }` or `{ text }` is
interpreted as a reply message, and anything else is wrapped as `data`. If you
want to return `{ text: "..." }` as _plain structured data_ rather than as a
message, include a `timestamp` field too, or the bus will read it as a reply.

Every skill of a plugin shares one agent id, so the host filters each
subscription by skill id. Without that filter a `describeWidget` message would
also reach the `listDashboards` handler and `request` would take whichever replied
first.

The bus caps delivery depth (8 by default, tracked in an `AsyncLocalStorage`)
and `request` times out at 10s. `request` to an agent with no subscribers raises
`AgentNotLoadedError`; that is how a disabled plugin fails.

## A2A

Skills on the manifest become A2A skills automatically. The aggregate card
namespaces them as `<pluginId>.<skillId>` so two plugins that both declare
`listChannels` do not collide; per-plugin cards use the bare id. A disabled
plugin's card and skills are refused. See [a2a.md](a2a.md).

## A complete minimal plugin

Save as `packages/plugin-weather-adapter/src/index.ts` in a new workspace
package that depends on `@loams-core/host`.

```ts
/**
 * A minimal read-only plugin.
 *
 * Shows the smallest thing that is genuinely useful: a manifest with one
 * advertised skill, a loader that serves one route and implements that skill,
 * and nothing else. Register it from the app's catalog, not from here --
 * an adapter that registered itself would make load order a function of import
 * order.
 */
import { Context, Service } from "cordis";
import type {
  AgentSkillContext,
  PluginAgentSkill,
  PluginLoader,
  PluginManifest,
} from "@loams-core/host";

declare module "cordis" {
  interface Context {
    weather: WeatherService;
  }
}

export interface WeatherConfig {
  /** Host root, e.g. https://weather.internal. */
  baseUrl: string;
  /** Optional bearer token. */
  token?: string;
}

export class WeatherService extends Service {
  static inject = [];
  public readonly config: WeatherConfig;

  constructor(ctx: Context, config: WeatherConfig) {
    super(ctx, "weather");
    this.config = config;
  }

  async listObservations(limit: number): Promise<unknown[]> {
    this.ctx.logger.debug("weather: listing %s observations", limit);
    const response = await fetch(`${this.config.baseUrl}/observations?limit=${limit}`, {
      headers: this.config.token ? { Authorization: `Bearer ${this.config.token}` } : {},
    });
    if (!response.ok) throw new Error(`weather: upstream returned ${response.status}`);
    const body = (await response.json()) as unknown;
    return Array.isArray(body) ? body : [];
  }
}

export const WEATHER_SKILLS: PluginAgentSkill[] = [
  {
    id: "listWeatherObservations",
    name: "List observations",
    description: "Recent observations from the internal weather service.",
    tags: ["weather", "read"],
    examples: ['listWeatherObservations {"limit":10}'],
    inputModes: ["application/json"],
    outputModes: ["application/json"],
  },
];

export const weatherManifest: PluginManifest = {
  id: "weather",
  name: "Weather",
  description: "Read-only weather observations from the internal service.",
  version: "1.0.0",
  category: "upstream",
  uiPath: "/plugins/weather",
  order: 40,
  defaultEnabled: false,
  requiredScopes: ["weather:read"],
  upstream: { product: "Weather", envPrefix: "WEATHER" },
  agent: {
    name: "Weather Agent",
    description: "Reads observations from the internal weather service.",
    version: "1.0.0",
    skills: WEATHER_SKILLS,
  },
};

function limitOf(value: unknown): number {
  const parsed = Number(value ?? 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error('listWeatherObservations: "limit" must be a positive number');
  }
  return Math.min(Math.trunc(parsed), 500);
}

export const weatherLoader: PluginLoader = {
  service: WeatherService,
  config: {
    baseUrl: process.env.WEATHER_URL,
    token: process.env.WEATHER_TOKEN,
  },

  routes: (runtime) => [
    {
      name: "weather:observations",
      method: "GET",
      match: "/api/plugins/weather/observations",
      handler: async ({ url, sendJson }) => {
        const requested = Number(url.searchParams.get("limit") ?? 10);
        sendJson(200, {
          observations: await runtime.ctx.weather.listObservations(limitOf(requested)),
        });
        return true;
      },
    },
  ],

  skills: () => [
    {
      id: "listWeatherObservations",
      handle: async (params, api: AgentSkillContext) => {
        const limit = limitOf(params.limit);
        api.progress("fetching", { limit });
        return { observations: await api.ctx.weather.listObservations(limit) };
      },
    },
  ],
};

/** Catalog entry shape used by the app to decide whether this is configured. */
export function weatherRequiredEnv(): readonly string[] {
  return ["WEATHER_URL"];
}

export function weatherConfigured(env: Record<string, string | undefined>): boolean {
  return typeof env.WEATHER_URL === "string" && env.WEATHER_URL.trim().length > 0;
}
```

Then register it in the app's catalog alongside the other adapters, with a
`refusingLoader` that names the missing variable when it is not configured. The
pattern is in `apps/server/src/plugin-catalog.ts`, and the reasoning for
"registered but off, never skipped" is in that file's header comment.

Two things to check before you open a review:

- **The manifest's skill ids and the loader's handler ids are identical.** The
  A2A layer validates a call against the manifest, but validation cannot stop the
  wrong handler from answering on the bus.
- **Your service is attached by the host, not by `ctx.plugin` in the boot chain.**
  If you attach it yourself it stays running for the life of the process and
  "off" becomes a lie.
