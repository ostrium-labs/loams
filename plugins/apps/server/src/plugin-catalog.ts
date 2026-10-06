/**
 * The upstream adapter catalog: which upstreams exist, and what has to be
 * configured before each one can be switched on.
 *
 * REGISTERING IS NOT ENABLING
 * ----------------------------
 * Adding a manifest to `PluginRegistry` puts a plugin in the plugin list. It does
 * not start it. What starts it is `PluginHost` finding the entry enabled and
 * loading the loader's service -- which is why no adapter service is registered
 * on the root context here. Handing `ctx.plugin(Service, config)` at boot would
 * attach every adapter for the life of the process, and `POST
 * /api/plugins/<id>/disable` would then flip a flag while the service kept
 * running, which is precisely the "off means off" contract `PluginHost` exists
 * to keep. The service classes below are therefore attached *by the host*, at
 * enable time, from the `service`/`config` pair on each loader.
 *
 * AUTO-ENABLE POLICY (deliberate, and the only place it lives)
 * -----------------------------------------------------------
 * An adapter is auto-enabled at boot IFF its credentials are present in the
 * environment and its own manifest asks for that (`defaultEnabled`). Rationale:
 *
 *   - A fresh checkout has no upstream credentials, so auto-enabling on
 *     `defaultEnabled` alone would boot several adapters pointed at nothing.
 *     Their constructors only build a client, so they would not throw -- they
 *     would answer every agent skill with an upstream connection error, and the
 *     console would look broken rather than unconfigured.
 *   - Conversely, somebody who has exported `ZULIP_API_KEY` has already declared
 *     the intent to use that adapter, so waiting for a second toggle is friction
 *     with no information in it.
 *
 * An explicit console toggle always wins: the registry persists the flag and
 * consults `defaultEnabled` only for a plugin that has never been toggled.
 *
 * NOT-CONFIGURED IS NOT AN ERROR
 * ------------------------------
 * A plugin missing configuration is registered, listed, left disabled, and
 * carries a message naming the environment variables that would configure it.
 * Its `state` stays `unloaded`, never `error`: `error` is the console's word for
 * "this broke", and on a fresh checkout nothing has broken. See
 * `PluginRegistry.markNotConfigured`.
 */

import type { Context } from "cordis";
import type { PluginLoader, PluginManifest, PluginStatus } from "@loams-plugins/core";
import {
  GlitchtipAdapterService,
  glitchtipManifest,
} from "@loams-plugins/plugin-glitchtip-adapter";
import { LangfuseAdapterService, langfuseManifest } from "@loams-plugins/plugin-langfuse-adapter";
import {
  OpenPanelAdapterService,
  openPanelManifest,
} from "@loams-plugins/plugin-openpanel-adapter";
import { forgejoLoader, forgejoManifest } from "@loams-plugins/plugin-forgejo-adapter";
import { itsaplanLoader, itsaplanManifest } from "@loams-plugins/plugin-itsaplan-adapter";
import { loamsLoader, loamsManifest } from "@loams-plugins/plugin-loams-adapter";
import { matomoLoader, matomoManifest } from "@loams-plugins/plugin-matomo-adapter";
import { zulipLoader, zulipManifest } from "@loams-plugins/plugin-zulip-adapter";

export type Env = Record<string, string | undefined>;

/* -------------------------------------------------------------------------- */
/* Env helpers                                                                 */
/* -------------------------------------------------------------------------- */

function text(env: Env, key: string): string | undefined {
  const value = env[key];
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : undefined;
}

function integer(env: Env, key: string): number | undefined {
  const raw = text(env, key);
  if (raw === undefined) return undefined;
  const parsed = Number.parseInt(raw, 10);
  return Number.isFinite(parsed) ? parsed : undefined;
}

function boolean(env: Env, key: string): boolean | undefined {
  const raw = text(env, key)?.toLowerCase();
  if (raw === undefined) return undefined;
  return raw === "1" || raw === "true" || raw === "yes";
}

/** Drop undefined values so a config object never carries an explicit `undefined`. */
function compact(config: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(config)) {
    if (value !== undefined) out[key] = value;
  }
  return out;
}

/* -------------------------------------------------------------------------- */
/* Control plane                                                               */
/* -------------------------------------------------------------------------- */

/**
 * The control-plane client (`@loams-plugins/plugin-control-plane`), which ships no
 * manifest of its own.
 *
 * The plugin identity is "control plane", but the upstream it actually speaks to is
 * Apache Superset over its REST API, so `upstream` names Superset and `envPrefix`
 * stays `SUPERSET` -- both are facts about the wire, not about this project's naming.
 * Renaming them would misdescribe the protocol.
 *
 * It is `alwaysOn` for the same reason the dashboard is, and the mechanism is the
 * one `host.ts` documents: the service is attached on the root context at boot (the
 * thirteen original `/api/*` routes read `ctx.controlPlane` directly), so a console
 * toggle could not unload it. A toggle that reports success while nothing changes
 * is worse than no toggle, so the toggle is disabled instead.
 *
 * Because it is `alwaysOn`, `PluginRegistry` ignores any persisted `plugin_state` row
 * for it (`registry.ts` only consults persisted state when `alwaysOn` is not set), so
 * the id change costs no stored toggle state.
 */
export const CONTROL_PLANE_MANIFEST: PluginManifest = {
  id: "control-plane",
  name: "Control Plane",
  description:
    "The BI warehouse this dashboard reads from: datasets, charts and query results, over the Superset-compatible control-plane API.",
  version: "1.0.0",
  category: "core",
  uiPath: "/plugins/control-plane",
  order: 1,
  alwaysOn: true,
  defaultEnabled: true,
  upstream: { product: "Apache Superset", envPrefix: "SUPERSET" },
};

/* -------------------------------------------------------------------------- */
/* Entries                                                                     */
/* -------------------------------------------------------------------------- */

/**
 * One adapter's registration.
 *
 * `requiredEnv` is what makes an adapter not-configured; `buildConfig` maps the
 * same environment onto the service's own config type field for field, so a
 * rename upstream shows up as a type error rather than a silently ignored
 * variable.
 */
interface CatalogEntry {
  manifest: PluginManifest;
  /** Loader as the adapter package exports it. */
  loader: PluginLoader;
  requiredEnv: readonly string[];
  buildConfig: (env: Env) => Record<string, unknown>;
  /**
   * False for an adapter whose package exports a manifest but no `PluginLoader`.
   * Such a plugin is registered so the console can see it, but it is never
   * auto-enabled: its manifest declares agent skills that have no handlers, and
   * advertising an agent that cannot answer is worse than advertising none.
   */
  autoEnable: boolean;
  /** The export that would replace the composed loader, for the boot warning. */
  missingLoaderFor?: string;
}

const MATOMO_PERIODS = ["day", "week", "month", "year", "range"] as const;

/**
 * The catalog, in console order. The registry re-sorts by `alwaysOn`, then
 * `order`, then name, so this order is for readability, not correctness.
 */
const ENTRIES: CatalogEntry[] = [
  {
    manifest: zulipManifest,
    loader: zulipLoader,
    requiredEnv: ["ZULIP_URL", "ZULIP_EMAIL", "ZULIP_API_KEY"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "ZULIP_URL"),
        email: text(env, "ZULIP_EMAIL"),
        apiKey: text(env, "ZULIP_API_KEY"),
        timeoutMs: integer(env, "ZULIP_TIMEOUT_MS"),
        rateLimitFloor: integer(env, "ZULIP_RATE_LIMIT_FLOOR"),
        maxPages: integer(env, "ZULIP_MAX_PAGES"),
      }),
    autoEnable: true,
  },
  {
    manifest: forgejoManifest,
    loader: forgejoLoader,
    requiredEnv: ["FORGEJO_URL", "FORGEJO_TOKEN"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "FORGEJO_URL"),
        token: text(env, "FORGEJO_TOKEN"),
        timeoutMs: integer(env, "FORGEJO_TIMEOUT_MS"),
        concurrency: integer(env, "FORGEJO_CONCURRENCY"),
        cacheTtlMs: integer(env, "FORGEJO_CACHE_TTL_MS"),
        limit: integer(env, "FORGEJO_LIMIT"),
      }),
    autoEnable: true,
  },
  {
    manifest: langfuseManifest,
    // Service-only loader: `@loams-plugins/plugin-langfuse-adapter` exports a manifest but no
    // `PluginLoader`. Attaching the service keeps enable/disable real for it, and
    // `autoEnable: false` keeps it from advertising skills nothing answers.
    loader: { service: LangfuseAdapterService },
    requiredEnv: ["LANGFUSE_URL", "LANGFUSE_PUBLIC_KEY", "LANGFUSE_SECRET_KEY"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "LANGFUSE_URL"),
        publicKey: text(env, "LANGFUSE_PUBLIC_KEY"),
        secretKey: text(env, "LANGFUSE_SECRET_KEY"),
        timeoutMs: integer(env, "LANGFUSE_TIMEOUT_MS"),
      }),
    autoEnable: false,
    missingLoaderFor: "langfuseLoader",
  },
  {
    manifest: openPanelManifest,
    loader: { service: OpenPanelAdapterService },
    requiredEnv: ["OPENPANEL_URL", "OPENPANEL_CLIENT_ID", "OPENPANEL_CLIENT_SECRET"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "OPENPANEL_URL"),
        clientId: text(env, "OPENPANEL_CLIENT_ID"),
        clientSecret: text(env, "OPENPANEL_CLIENT_SECRET"),
        // `/api` by default; "" is the direct-container address. See
        // `resolveOpenPanelBaseUrl`.
        apiPrefix: text(env, "OPENPANEL_API_PREFIX"),
        funnelStepEncoding: text(env, "OPENPANEL_FUNNEL_STEP_ENCODING") as
          | "repeated"
          | "csv"
          | undefined,
        timeoutMs: integer(env, "OPENPANEL_TIMEOUT_MS"),
      }),
    autoEnable: false,
    missingLoaderFor: "openPanelLoader",
  },
  {
    manifest: glitchtipManifest,
    loader: { service: GlitchtipAdapterService },
    requiredEnv: ["GLITCHTIP_URL", "GLITCHTIP_TOKEN"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "GLITCHTIP_URL"),
        token: text(env, "GLITCHTIP_TOKEN"),
        timeoutMs: integer(env, "GLITCHTIP_TIMEOUT_MS"),
      }),
    autoEnable: false,
    missingLoaderFor: "glitchtipLoader",
  },
  {
    manifest: matomoManifest,
    loader: matomoLoader,
    requiredEnv: ["MATOMO_URL", "MATOMO_API_TOKEN"],
    buildConfig: (env) => {
      const period = text(env, "MATOMO_DEFAULT_PERIOD");
      return compact({
        baseUrl: text(env, "MATOMO_URL"),
        apiToken: text(env, "MATOMO_API_TOKEN"),
        timeoutMs: integer(env, "MATOMO_TIMEOUT_MS"),
        defaultRowLimit: integer(env, "MATOMO_DEFAULT_ROW_LIMIT"),
        // Forwarded only for the literals `MatomoPeriod` declares. An
        // unrecognised value is dropped so the service keeps its own default
        // rather than sending a period Matomo will reject.
        defaultPeriod: MATOMO_PERIODS.includes(period as (typeof MATOMO_PERIODS)[number])
          ? period
          : undefined,
        defaultDate: text(env, "MATOMO_DEFAULT_DATE"),
      });
    },
    autoEnable: true,
  },
  {
    manifest: itsaplanManifest,
    loader: itsaplanLoader,
    requiredEnv: ["ITSAPLAN_URL", "ITSAPLAN_API_KEY"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "ITSAPLAN_URL"),
        apiKey: text(env, "ITSAPLAN_API_KEY"),
        timeoutMs: integer(env, "ITSAPLAN_TIMEOUT_MS"),
      }),
    autoEnable: true,
  },
  {
    manifest: loamsManifest,
    loader: loamsLoader,
    // `LOAMS_NAMESPACE` is optional in `LoamsConfig` and left optional here:
    // Loams has no list-namespaces route, so the adapter fails with an explicit
    // message at call time rather than the server refusing to start.
    requiredEnv: ["LOAMS_URL"],
    buildConfig: (env) =>
      compact({
        baseUrl: text(env, "LOAMS_URL"),
        namespace: text(env, "LOAMS_NAMESPACE"),
        // Loams has no auth today; this is for a proxy-fronted deployment.
        token: text(env, "LOAMS_TOKEN"),
        // Raw SQL, off unless asked for. `allowSql` gates the capability only --
        // the adapter does not sanitise the statement, and Loams enforces
        // SELECT-only server side.
        allowSql: boolean(env, "LOAMS_ALLOW_SQL"),
        timeoutMs: integer(env, "LOAMS_TIMEOUT_MS"),
      }),
    autoEnable: true,
  },
];

/* -------------------------------------------------------------------------- */
/* Planning                                                                    */
/* -------------------------------------------------------------------------- */

export interface CatalogEntryPlan {
  manifest: PluginManifest;
  /** Absent only for a manifest that is always-on and attached outside the host. */
  loader?: PluginLoader;
  /** Environment variables still missing; empty when the adapter is usable. */
  missing: string[];
}

export interface CatalogPlan {
  entries: CatalogEntryPlan[];
}

function missingEnv(env: Env, required: readonly string[]): string[] {
  return required.filter((key) => text(env, key) === undefined);
}

function notConfiguredMessage(manifest: PluginManifest, missing: string[]): string {
  return `${manifest.name} is not configured: set ${missing.join(", ")} to enable it.`;
}

/**
 * A loader that exists only to refuse, with the variable names attached.
 *
 * `attach` is the first hook the host runs after the service, so throwing here
 * means an adapter with no configuration fails with "set ZULIP_URL…" rather than
 * with whatever the service constructor does when handed `undefined`. It only
 * ever runs if somebody explicitly enables the plugin, which is exactly when the
 * answer "you are missing a variable" is worth an `error`.
 */
function refusingLoader(message: string): PluginLoader {
  return {
    attach: () => {
      throw new Error(message);
    },
  };
}

/**
 * Resolve every adapter against an environment. Pure: it reads `env` and returns
 * what WOULD be registered, so a test can ask "what does a fresh checkout look
 * like?" without mutating `process.env`.
 */
export function planPluginCatalog(env: Env = process.env): CatalogPlan {
  const entries: CatalogEntryPlan[] = [
    // Always on and attached at boot, so there is no loader and no env gate.
    { manifest: CONTROL_PLANE_MANIFEST, missing: [] },
    ...ENTRIES.map((entry) => {
      const missing = missingEnv(env, entry.requiredEnv);
      const usable = missing.length === 0;
      // `defaultEnabled` is the manifest's own opinion; the deployment's is
      // "are the credentials there".
      const defaultEnabled =
        entry.manifest.alwaysOn === true
          ? true
          : usable && entry.autoEnable && entry.manifest.defaultEnabled === true;
      return {
        manifest: { ...entry.manifest, defaultEnabled },
        loader: usable
          ? { ...entry.loader, config: entry.buildConfig(env) }
          : refusingLoader(notConfiguredMessage(entry.manifest, missing)),
        missing,
      };
    }),
  ];
  return { entries };
}

/* -------------------------------------------------------------------------- */
/* Registration                                                                */
/* -------------------------------------------------------------------------- */

export interface CatalogRegistration {
  /** Every registered plugin, in console order. */
  statuses: PluginStatus[];
  plan: CatalogPlan;
}

/**
 * Register the whole catalog with the host, and return the resulting statuses.
 *
 * A plugin that cannot be configured is registered and then annotated, never
 * skipped: the console can only say WHICH variable is missing for a plugin that
 * is in the list.
 */
export async function registerPluginCatalog(
  ctx: Context,
  env: Env = process.env,
): Promise<CatalogRegistration> {
  const plan = planPluginCatalog(env);
  const host = ctx.coreHost;

  for (const entry of plan.entries) {
    if (!entry.loader) {
      await host.register(entry.manifest);
      continue;
    }
    await host.register(entry.manifest, entry.loader);
  }

  // Annotate AFTER registering: `markLoaded` clears the message, and a configured
  // plugin that loaded on boot must not still be described as not-configured.
  for (const entry of plan.entries) {
    if (entry.missing.length === 0) continue;
    ctx.coreRegistry.markNotConfigured(
      entry.manifest.id,
      notConfiguredMessage(entry.manifest, entry.missing),
    );
    // INFO, not WARN: a fresh checkout with no upstream credentials is a
    // supported configuration, not a fault.
    ctx.logger.info(
      "core: %s is in the catalog but off -- missing %s",
      entry.manifest.id,
      entry.missing.join(", "),
    );
  }

  for (const entry of ENTRIES) {
    if (!entry.missingLoaderFor) continue;
    ctx.logger.warn(
      "core: %s exports a manifest but no %s, so its agent skills are advertised without " +
        "handlers. It is registered and toggleable but never auto-enabled; swap in the " +
        "package loader when it lands.",
      entry.manifest.id,
      entry.missingLoaderFor,
    );
  }

  return { statuses: ctx.coreRegistry.list(), plan };
}
