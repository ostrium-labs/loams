/**
 * Loading and UNLOADING plugins for real.
 *
 * "Off" here means the service is disposed, its routes are gone, and its
 * skills no longer answer. Not "hidden from a list". The user was explicit
 * about this, and it is also the only reading that makes an enable/disable
 * button mean anything: an adapter that keeps polling the BI backend after you turned
 * it off is a plugin platform that does not work.
 *
 * The observable proof is the 404. A disabled plugin's route is removed from
 * the router table, so the request falls all the way through to the server's
 * catch-all 404 instead of hitting a handler nobody is listening for anymore.
 * `core/host/tests/host.spec.ts` asserts exactly that.
 *
 * IDEMPOTENCE
 * Every teardown resource is retained per plugin (the fiber disposer, the bus
 * unsubscribers, the route remover) and every step tolerates being run twice.
 * Enable → disable → enable therefore produces one set of handlers, not two;
 * that test exists because the failure mode it catches is invisible until the
 * second or third toggle.
 *
 * ISOLATION
 * A loader that throws marks its own plugin `error` and is re-thrown to the
 * caller only as a status. Boot continues with every other plugin.
 */

import { Context, Service } from "cordis";
import type {
  AgentMessage,
  PluginLoader,
  PluginManifest,
  PluginRuntime,
  PluginStatus,
} from "./types.js";
import type { AgentBus } from "./bus.js";
import type { HttpRouter } from "./router.js";
import type { PluginRegistry } from "./registry.js";
import type { RouteSpec } from "./router.js";
import { ForbiddenError } from "./auth/errors.js";
import { evaluateScopeGate, type AuthPrincipal } from "./auth/scopes.js";
import type { AuthService } from "./auth/service.js";

declare module "cordis" {
  interface Context {
    coreHost: PluginHost;
  }
}

/** Everything a loaded plugin owns and must give back on unload. */
interface LoadedRecord {
  /** cordis fiber disposer, when the loader declared a service. */
  disposeService?: () => void | Promise<void>;
  /** Free-form disposer from `attach`. */
  disposeAttach?: () => void;
  /** Removes the plugin's routes from the router. */
  removeRoutes: () => void;
  /** Bus unsubscribers, one per skill. */
  unsubscribe: (() => void)[];
}

export interface PluginHostConfig {
  /**
   * Load every plugin whose resolved state is enabled during `[Service.init]`.
   * Defaults to true; turn it off in tests that register plugins afterwards.
   */
  autoLoadOnStart?: boolean;
}

export class PluginHost extends Service {
  /**
   * `store` is in this list on purpose, and it is the only platform key here.
   *
   * A skill handler is handed `this.ctx` (see `AgentSkillContext.ctx`) and reads
   * platform services off it. Cordis only resolves an inherited key on a context
   * that has declared it in `inject`, so without this a skill asking for
   * `ctx.store` throws `cannot get property "store" without inject` -- which is
   * exactly the failure a settings-backed plugin hits first. `store` is the one
   * service every plugin is entitled to: it is where enable flags live, so a
   * plugin that cannot read it cannot know its own state.
   */
  static inject = ["coreRegistry", "router", "agentBus", "store"];

  private readonly _loaded = new Map<string, LoadedRecord>();
  /** Guards against re-entering load/unload from the registry event. */
  private _applying = 0;
  public config: PluginHostConfig;

  constructor(ctx: Context, config?: PluginHostConfig) {
    super(ctx, "coreHost");
    this.config = config ?? {};
    // Registered with `global: false` (default) on this context, so it goes away
    // with the host rather than leaking into every other service's event bus.
    this.ctx.on("plugin:changed", (change) => {
      if (this._applying > 0) return;
      void this._react(change.id, change.enabled);
    });
  }

  async [Service.init]() {
    const registry = this._registry();
    await registry.resolveAll();
    if (this.config.autoLoadOnStart === false) return;
    await this.sync();
  }

  /* ---------------------------------------------------------------------- */
  /* Registration                                                            */
  /* ---------------------------------------------------------------------- */

  /**
   * Register a plugin and, if it resolves to enabled, load it immediately.
   * This is the normal entry point for a plugin that comes up after boot.
   */
  async register(manifest: PluginManifest, loader?: PluginLoader): Promise<PluginStatus> {
    this._registry().register(manifest, loader);
    return this.syncOne(manifest.id);
  }

  /** Load every enabled plugin that is not currently loaded. */
  async sync(): Promise<PluginStatus[]> {
    const registry = this._registry();
    await registry.resolveAll();
    const results: PluginStatus[] = [];
    for (const status of registry.list()) {
      if (!status.enabled) continue;
      results.push(await this.syncOne(status.id));
    }
    return results;
  }

  /** Load one plugin if it is enabled and not already loaded. */
  async syncOne(id: string): Promise<PluginStatus> {
    const registry = this._registry();
    if (!registry.has(id)) return registry.get(id);
    await registry.resolveAll();
    const status = registry.get(id);
    if (!status.enabled || this.isLoaded(id)) return status;
    return this.enable(id);
  }

  /* ---------------------------------------------------------------------- */
  /* Load / unload                                                           */
  /* ---------------------------------------------------------------------- */

  isLoaded(id: string): boolean {
    return this._loaded.has(id);
  }

  /** Ids of the plugins whose services are currently attached. */
  loadedPlugins(): string[] {
    return [...this._loaded.keys()];
  }

  /**
   * Enable a plugin: flip the persisted flag, then actually load it.
   *
   * Returns the resulting status rather than throwing on a loader failure —
   * `state: "error"` with the message is the answer, and the caller (the REST
   * layer) turns that into a 200 with an error field so one broken adapter
   * cannot cascade into the UI reporting the whole console as broken.
   */
  async enable(id: string, principal?: AuthPrincipal): Promise<PluginStatus> {
    const registry = this._registry();

    // A plugin the caller is not entitled to must not be switched ON. Returning the
    // blocked status (rather than throwing) is what lets the console render the plugin
    // with its missing scopes named instead of a bare error toast.
    if (principal) {
      const missing = registry.missingScopesFor(id, principal);
      if (missing.length > 0) {
        const status = registry.get(id);
        this.ctx.logger.warn(
          "core: refusing to enable %s for a principal lacking %s",
          id,
          missing.join(", "),
        );
        return { ...status, state: "blocked", missingScopes: missing };
      }
    }

    this._applying += 1;
    try {
      await registry.enable(id);
    } finally {
      this._applying -= 1;
    }
    return this._load(id);
  }

  /**
   * Disable a plugin: flip the persisted flag, then tear the plugin down.
   * Idempotent — disabling something already unloaded returns its status.
   */
  async disable(id: string): Promise<PluginStatus> {
    const registry = this._registry();
    this._applying += 1;
    try {
      await registry.disable(id);
    } finally {
      this._applying -= 1;
    }
    return this._unload(id);
  }

  private async _react(id: string, enabled: boolean): Promise<void> {
    try {
      if (enabled) await this._load(id);
      else await this._unload(id);
    } catch (err) {
      this.ctx.logger.error(
        "core: reacting to plugin:changed for %s failed: %s",
        id,
        err instanceof Error ? err.message : String(err),
      );
    }
  }

  private async _load(id: string): Promise<PluginStatus> {
    const registry = this._registry();
    if (!registry.has(id)) return registry.get(id);
    if (this._loaded.has(id)) return registry.get(id);

    const manifest = registry.manifest(id);
    const loader = registry.loader(id);
    const record: LoadedRecord = { removeRoutes: () => {}, unsubscribe: [] };

    try {
      const runtime: PluginRuntime = {
        id,
        manifest,
        ctx: this.ctx,
        router: this._router(),
        bus: this._bus(),
      };

      if (loader?.service) {
        const fiber = await this.ctx.plugin(loader.service, loader.config);
        record.disposeService = () => fiber.dispose();
      }

      if (loader?.attach) {
        const disposer = loader.attach(runtime);
        if (typeof disposer === "function") record.disposeAttach = disposer;
      }

      if (loader?.routes) {
        // Every route a plugin registers is wrapped in a guard built from the manifest's
        // `requiredScopes`. This is the enforcement point, and it runs PER REQUEST: the
        // decision is taken against the principal presenting credentials right now.
        //
        // Enforcing only when the plugin is enabled would be wrong, and wrong in a way
        // that looks correct: a session created before this plugin was deployed predates
        // it, was never granted its scopes, and would sail straight through.
        const gate = this._scopeGuard(manifest);
        const specs: RouteSpec[] = loader.routes(runtime);
        record.removeRoutes = this._router().addAll(
          gate ? specs.map((spec) => ({ ...spec, guard: gate })) : specs,
          id,
        );
      }

      if (loader?.skills) {
        record.unsubscribe = loader.skills(runtime).map((skill) =>
          this._bus().subscribe(id, (message) =>
            // Filter by skill id. Every skill of a plugin shares ONE agent id,
            // so without this the bus fans a `describeWidget` message to the
            // `listDashboards` handler as well and `request` takes the first
            // reply. The A2A layer validates the skill against the manifest,
            // but validation cannot stop the wrong handler from answering.
            message.skill === skill.id ? this._invoke(skill.handle, message) : undefined,
          ),
        );
      }

      this._loaded.set(id, record);
      const status = registry.markLoaded(id);
      this.ctx.logger.info("core: plugin loaded: %s (%s)", manifest.name, id);
      return status;
    } catch (err) {
      // Undo whatever did succeed, so a half-loaded plugin leaves nothing behind.
      await this._teardown(record);
      const message = err instanceof Error ? err.message : String(err);
      this.ctx.logger.error("core: plugin %s failed to load: %s", id, message);
      return registry.markError(id, message);
    }
  }

  private async _unload(id: string): Promise<PluginStatus> {
    const registry = this._registry();
    if (!registry.has(id)) return registry.get(id);
    const record = this._loaded.get(id);
    if (!record) {
      // Already unloaded. Clearing a stale `error` matters here: a plugin that
      // failed to load and was then disabled should read as off, not broken.
      return registry.get(id).state === "error" ? registry.markUnloaded(id) : registry.get(id);
    }
    this._loaded.delete(id);
    try {
      await this._teardown(record);
    } catch (err) {
      // Teardown failures are logged, not thrown: a plugin that refuses to
      // unload cleanly must not block the others from being disabled.
      this.ctx.logger.error(
        "core: plugin %s did not tear down cleanly: %s",
        id,
        err instanceof Error ? err.message : String(err),
      );
    }
    const status = registry.markUnloaded(id);
    this.ctx.logger.info("core: plugin unloaded: %s", id);
    return status;
  }

  /**
   * Reverse order of load: routes first (stop new work arriving), then skills,
   * then the service fiber, then any free-form disposer.
   */
  private async _teardown(record: LoadedRecord): Promise<void> {
    record.removeRoutes();
    for (const unsubscribe of record.unsubscribe) {
      try {
        unsubscribe();
      } catch (err) {
        this.ctx.logger.warn(
          "core: skill unsubscribe threw during teardown: %s",
          err instanceof Error ? err.message : String(err),
        );
      }
    }
    record.unsubscribe = [];
    if (record.disposeService) await record.disposeService();
    if (record.disposeAttach) record.disposeAttach();
  }

  private _invoke(
    handle: (params: Record<string, unknown>, api: any) => unknown,
    message: AgentMessage,
  ): unknown {
    return handle(message.params ?? {}, {
      ctx: this.ctx,
      message,
      // Reported as a bus event rather than accumulated here: the A2A layer
      // collects progress into `Task.artifacts`, but a skill invoked directly
      // over the bus has nobody to collect it, and an event serves both.
      progress: (name: string, data: unknown) => {
        this.ctx.emit("agent/progress", { from: message.from, skill: message.skill, name, data });
      },
    });
  }

  private _registry(): PluginRegistry {
    return this.ctx.coreRegistry;
  }

  private _router(): HttpRouter {
    return this.ctx.router;
  }

  private _bus(): AgentBus {
    return this.ctx.agentBus;
  }

  /**
   * The auth service, when one is loaded.
   *
   * Read defensively for the same reason the registry reads `ctx.store` that way: the key
   * may never have been provided, and a plugin platform must boot without auth so that a
   * developer can run it against no IdP at all.
   */
  private _auth(): AuthService | undefined {
    try {
      const auth = (this.ctx as unknown as { auth?: AuthService }).auth;
      return auth && typeof auth.principal === "function" ? auth : undefined;
    } catch {
      return undefined;
    }
  }

  /**
   * Build the per-request guard for one plugin, or `undefined` when it declares no
   * `requiredScopes` (in which case wrapping every route would be pure overhead).
   */
  private _scopeGuard(manifest: PluginManifest): RouteSpec["guard"] | undefined {
    const required = manifest.requiredScopes;
    if (!required || required.length === 0) return undefined;
    return async (request) => {
      const auth = this._auth();
      if (!auth) return true;
      const principal = await auth.principal(request);
      const verdict = evaluateScopeGate(required, principal);
      if (verdict.ok) return true;
      throw new ForbiddenError(
        `Plugin "${manifest.id}" requires scope${required.length === 1 ? "" : "s"} ` +
          `${verdict.missing.join(", ")}, which this session does not hold.`,
        { code: "insufficient_scope", missing: verdict.missing },
      );
    };
  }
}
