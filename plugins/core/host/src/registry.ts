/**
 * The plugin catalog: which plugins exist, and whether each one is on.
 *
 * SCOPE
 * The registry owns *declarative* state only — the manifest, the enabled flag,
 * and whether a plugin is in an error state. It deliberately does not load
 * anything; `PluginHost` does that. Keeping the two apart means the console can
 * list and persist preferences without any of them being able to break the
 * server, and it means a loader that throws cannot corrupt the catalog.
 *
 * PERSISTENCE
 * The enabled flag is written to the store so it survives a restart. `defaultEnabled`
 * is only consulted for a plugin that has never been toggled — otherwise
 * flipping a switch would look like it worked and silently revert on the next
 * boot, which is the classic way this feature gets reported as broken.
 */

import { Context, Service } from "cordis";
import type { PluginLoader, PluginManifest, PluginStatus } from "./types.js";
import { evaluateScopeGate, type AuthPrincipal } from "./auth/scopes.js";

export interface PluginChangedEvent {
  id: string;
  enabled: boolean;
}

export interface CoreRegistryEvents {
  "plugin:changed"(change: PluginChangedEvent): void;
}

declare module "cordis" {
  interface Events extends CoreRegistryEvents {}
}

declare module "cordis" {
  interface Context {
    coreRegistry: PluginRegistry;
  }
}

/**
 * The slice of `StoreService` the registry needs.
 *
 * Declared structurally rather than as `StoreService` so the registry keeps
 * working — in memory — when no store plugin is loaded at all.
 */
export interface PluginStateStore {
  getPluginState(id: string): Promise<boolean | undefined>;
  setPluginState(id: string, enabled: boolean): Promise<void>;
}

export interface PluginRegistryConfig {
  /** Override the store. Defaults to `ctx.store` when it provides one. */
  store?: PluginStateStore | null;
}

interface RegistryEntry {
  manifest: PluginManifest;
  loader?: PluginLoader;
  enabled: boolean;
  state: PluginStatus["state"];
  error?: string;
  changedAt?: number;
  /** In-flight persistence lookup, so a late `register` still resolves. */
  resolving?: Promise<void>;
}

/** Console ordering: always-on first, then `order`, then name. */
function compare(a: PluginManifest, b: PluginManifest): number {
  const alwaysA = a.alwaysOn ? 0 : 1;
  const alwaysB = b.alwaysOn ? 0 : 1;
  if (alwaysA !== alwaysB) return alwaysA - alwaysB;
  const orderA = a.order ?? 100;
  const orderB = b.order ?? 100;
  if (orderA !== orderB) return orderA - orderB;
  return a.name.localeCompare(b.name);
}

export class PluginRegistry extends Service {
  static inject = [];

  private readonly _entries = new Map<string, RegistryEntry>();
  public config: PluginRegistryConfig;

  constructor(ctx: Context, config?: PluginRegistryConfig) {
    super(ctx, "coreRegistry");
    this.config = config ?? {};
  }

  /* ---------------------------------------------------------------------- */
  /* Registration                                                            */
  /* ---------------------------------------------------------------------- */

  /**
   * Add a manifest to the catalog. Safe before or after service start.
   *
   * The enabled flag starts from `alwaysOn` / `defaultEnabled` and is then
   * corrected asynchronously from the store, because a plugin registered after
   * `[Service.init]` cannot have been part of the boot-time read.
   */
  register(manifest: PluginManifest, loader?: PluginLoader): PluginStatus {
    if (this._entries.has(manifest.id)) {
      throw new Error(`Plugin already registered: ${manifest.id}`);
    }
    const entry: RegistryEntry = {
      manifest,
      loader,
      enabled: manifest.alwaysOn ? true : (manifest.defaultEnabled ?? true),
      state: "unloaded",
    };
    this._entries.set(manifest.id, entry);
    void this._resolve(entry);
    return this._status(entry);
  }

  /** Remove a manifest entirely. Only meaningful before anything is loaded. */
  unregister(id: string): boolean {
    return this._entries.delete(id);
  }

  /* ---------------------------------------------------------------------- */
  /* Lifecycle                                                               */
  /* ---------------------------------------------------------------------- */

  async [Service.init]() {
    await this.resolveAll();
  }

  /** Await every pending store lookup, so callers see final enabled flags. */
  async resolveAll(): Promise<void> {
    await Promise.all([...this._entries.values()].map((entry) => this._resolve(entry)));
  }

  private _resolve(entry: RegistryEntry): Promise<void> {
    entry.resolving ??= (async () => {
      const store = this._store();
      if (!store) return;
      try {
        const persisted = await store.getPluginState(entry.manifest.id);
        if (persisted !== undefined && !entry.manifest.alwaysOn) entry.enabled = persisted;
      } catch (err) {
        this.ctx.logger.warn(
          "core: could not read persisted state for plugin %s, using default: %s",
          entry.manifest.id,
          err instanceof Error ? err.message : String(err),
        );
      }
    })();
    return entry.resolving;
  }

  /**
   * The store, if one is loaded.
   *
   * Read through a cast and a try/catch on purpose: `ctx.store` is a legal
   * lookup that throws when the key was never provided, and the registry must
   * degrade to in-memory rather than refuse to boot.
   */
  private _store(): PluginStateStore | null {
    if (this.config.store !== undefined) return this.config.store;
    try {
      const store = (this.ctx as unknown as { store?: PluginStateStore }).store;
      return store ?? null;
    } catch {
      return null;
    }
  }

  private async _persist(id: string, enabled: boolean): Promise<void> {
    const store = this._store();
    if (!store) return;
    try {
      await store.setPluginState(id, enabled);
    } catch (err) {
      this.ctx.logger.warn(
        "core: could not persist state for plugin %s: %s",
        id,
        err instanceof Error ? err.message : String(err),
      );
    }
  }

  /* ---------------------------------------------------------------------- */
  /* Reads                                                                   */
  /* ---------------------------------------------------------------------- */

  /** All plugins in console order. */
  list(): PluginStatus[] {
    return [...this._entries.values()]
      .map((entry) => this._status(entry))
      .sort((a, b) => compare(a, b));
  }

  /** One plugin, or `undefined` when it is not in the catalog. */
  find(id: string): PluginStatus | undefined {
    const entry = this._entries.get(id);
    return entry ? this._status(entry) : undefined;
  }

  /** One plugin. Throws when unknown, for call sites that require it. */
  get(id: string): PluginStatus {
    const status = this.find(id);
    if (!status) throw new Error(`Unknown plugin: ${id}`);
    return status;
  }

  has(id: string): boolean {
    return this._entries.has(id);
  }

  manifest(id: string): PluginManifest {
    return this._entry(id).manifest;
  }

  loader(id: string): PluginLoader | undefined {
    return this._entries.get(id)?.loader;
  }

  /* ---------------------------------------------------------------------- */
  /* Writes                                                                  */
  /* ---------------------------------------------------------------------- */

  /**
   * Turn a plugin on and persist it. Enabling an already-enabled plugin is a
   * no-op that returns the current status — the console toggles optimistically
   * and re-clicks, and a toggle must never be an error.
   */
  async enable(id: string): Promise<PluginStatus> {
    const entry = this._entry(id);
    await this._resolve(entry);
    if (entry.manifest.alwaysOn) entry.enabled = true;
    if (entry.enabled) return this._status(entry);
    entry.enabled = true;
    entry.changedAt = Date.now();
    await this._persist(id, true);
    this.ctx.emit("plugin:changed", { id, enabled: true });
    return this._status(entry);
  }

  /**
   * Turn a plugin off and persist it.
   *
   * Refuses for `alwaysOn` plugins: the dashboard is the thing every other
   * route lives inside, so an "off" dashboard is a server with no front door,
   * and the console toggle for it is already disabled.
   */
  async disable(id: string): Promise<PluginStatus> {
    const entry = this._entry(id);
    if (entry.manifest.alwaysOn) {
      throw new Error(
        `Plugin "${id}" is always on and cannot be disabled: ${entry.manifest.name} is required by the platform.`,
      );
    }
    await this._resolve(entry);
    if (!entry.enabled) return this._status(entry);
    entry.enabled = false;
    entry.changedAt = Date.now();
    await this._persist(id, false);
    this.ctx.emit("plugin:changed", { id, enabled: false });
    return this._status(entry);
  }

  /* ---------------------------------------------------------------------- */
  /* Load state, written by the host                                         */
  /* ---------------------------------------------------------------------- */

  markLoaded(id: string): PluginStatus {
    const entry = this._entry(id);
    entry.state = "loaded";
    delete entry.error;
    entry.changedAt = Date.now();
    return this._status(entry);
  }

  markUnloaded(id: string): PluginStatus {
    const entry = this._entry(id);
    entry.state = "unloaded";
    delete entry.error;
    entry.changedAt = Date.now();
    return this._status(entry);
  }

  /**
   * Record that a plugin is OFF because its configuration is missing.
   *
   * Deliberately NOT `markError`. `error` is the console's word for "this
   * broke", and a fresh checkout has broken nothing: eight adapters with no
   * credentials are a supported configuration, not a fault. Reporting them as
   * errors would open the console on a wall of red and train the reader to
   * ignore the one row that IS broken.
   *
   * So the plugin keeps `state: "unloaded"` -- nothing was loaded and nothing
   * failed -- and the actionable half rides in the same message field the
   * console already renders, naming the variables that would configure it. The
   * next `markLoaded` clears it, because a plugin that has since loaded is no
   * longer waiting for configuration.
   *
   * A proper `notConfigured` state would be cleaner than overloading `error`,
   * but `PluginStatus.state` is frozen contract that other packages code
   * against (`core/src/ui/pluginState.ts` switches on it), so widening it is not
   * a change this file can make on its own.
   */
  markNotConfigured(id: string, message: string): PluginStatus {
    const entry = this._entry(id);
    entry.error = message;
    return this._status(entry);
  }

  /**
   * Record a load failure. The plugin stays `enabled` (the user did ask for it)
   * and moves to `error`, which is what lets the console offer a retry and what
   * keeps one broken plugin from looking like a missing one.
   */
  markError(id: string, error: string): PluginStatus {
    const entry = this._entry(id);
    entry.state = "error";
    entry.error = error;
    entry.changedAt = Date.now();
    return this._status(entry);
  }

  /* ---------------------------------------------------------------------- */
  /* Scope view, per request                                                 */
  /* ---------------------------------------------------------------------- */

  /**
   * The catalog as ONE session sees it.
   *
   * `blocked` is derived here rather than stored, because it is a property of the
   * (plugin, session) pair and not of the plugin. Storing it would make one under-privileged
   * user's view of the console bleed into everybody else's, and would leave a stale
   * `blocked` behind after the user re-authenticates.
   *
   * The live load state is preserved underneath: a plugin that is genuinely `error` stays
   * `error`, because "your session lacks a scope" is not a more useful thing to say than
   * "this plugin failed to load".
   */
  listFor(principal: AuthPrincipal | undefined): PluginStatus[] {
    return this.list().map((status) => this._withScopeVerdict(status, principal));
  }

  /** One plugin as one session sees it. */
  findFor(id: string, principal: AuthPrincipal | undefined): PluginStatus | undefined {
    const status = this.find(id);
    return status ? this._withScopeVerdict(status, principal) : undefined;
  }

  private _withScopeVerdict(
    status: PluginStatus,
    principal: AuthPrincipal | undefined,
  ): PluginStatus {
    const verdict = evaluateScopeGate(status.requiredScopes, principal);
    if (verdict.ok) return status;
    // `blocked` replaces the load state rather than qualifying it. "This session cannot use
    // this plugin" is the actionable fact whether or not the plugin is running, and
    // reporting `unloaded` for a plugin that is loaded would send the console looking for
    // a load failure that did not happen.
    return { ...status, state: "blocked", missingScopes: verdict.missing };
  }

  /** The `requiredScopes` of a plugin that this principal does not satisfy. */
  missingScopesFor(id: string, principal: AuthPrincipal | undefined): string[] {
    const entry = this._entries.get(id);
    if (!entry) return [];
    return evaluateScopeGate(entry.manifest.requiredScopes, principal).missing;
  }

  private _entry(id: string): RegistryEntry {
    const entry = this._entries.get(id);
    if (!entry) throw new Error(`Unknown plugin: ${id}`);
    return entry;
  }

  private _status(entry: RegistryEntry): PluginStatus {
    const status: PluginStatus = {
      ...entry.manifest,
      enabled: entry.manifest.alwaysOn ? true : entry.enabled,
      state: entry.state,
    };
    if (entry.changedAt !== undefined) status.changedAt = entry.changedAt;
    if (entry.error !== undefined) status.error = entry.error;
    return status;
  }
}
