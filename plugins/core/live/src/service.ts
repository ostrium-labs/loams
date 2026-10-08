/**
 * Loams' own live state, as a dashboard data source.
 *
 * The dashboard reads the BI backend through the control plane, which is a warehouse:
 * good for history, wrong for "what is happening right now". Loams already
 * publishes operations, approvals and notifications as server-streaming watch
 * RPCs, so this keeps one projected copy of each and serves it to widgets
 * through `data`'s provider registry.
 *
 * The stream is opened once per service and shared by every widget reading it,
 * because a watch is stateful: each connection costs a subscription and each
 * reconnection replays from a cursor, so one reader per stream is both cheaper
 * and the only way the cursors stay coherent.
 */

import { Context, Service } from "cordis";
// Side-effect imports: augment cordis Context with the `data` key we register into.
import "@loams-core/data";
import { watch, LoamsStreamError, type WatchHandle } from "./connect.js";
import { reduce, initialState, type LiveRecord, type LiveState } from "./store.js";

/** The mock's read token. Overridden per-widget by `data.token`. */
export const DEFAULT_TOKEN = "mock-access-usr_omar";
export const DEFAULT_BASE_URL = "http://127.0.0.1:8084";

export type LoamsStreamName = "operations" | "approvals" | "notifications";

const SERVICES: readonly LoamsStreamName[] = ["operations", "approvals", "notifications"];

/**
 * Where each stream actually lives, spelled out rather than derived.
 *
 * The service segment is capitalised but the package segment is not, and the two
 * do not pluralise together -- `loams.approvals.v1.ApprovalService` and
 * `loams.notifications.v1.NotificationService` are both singular. Any formula
 * derived from the stream name guesses wrong at least one of the three, and the
 * server answers a wrong path with a plain `unimplemented: method not found`,
 * which reads like a missing feature rather than a typo.
 */
const STREAM_RPCS: Record<LoamsStreamName, { path: string; method: string }> = {
  operations: { path: "loams.operations.v1.OperationsService", method: "WatchOperations" },
  approvals: { path: "loams.approvals.v1.ApprovalService", method: "WatchApprovals" },
  notifications: {
    path: "loams.notifications.v1.NotificationService",
    method: "WatchNotifications",
  },
};

export interface LoamsLiveConfig {
  /** Where the Loams server is. */
  baseUrl?: string;
  token?: string;
  /** Stop after this long without a message, then resume from the cursor. */
  idleTimeoutMs?: number;
  /** Backoff bounds for reconnect. */
  reconnectMinMs?: number;
  reconnectMaxMs?: number;
  /** Streams to keep open. Defaults to all three. */
  streams?: LoamsStreamName[];
}

interface Stream {
  handle?: WatchHandle;
  timer?: ReturnType<typeof setTimeout>;
  backoff: number;
}

export class LoamsLiveService extends Service {
  static inject = ["data"];

  private _config: Required<Omit<LoamsLiveConfig, "streams">> & { streams: LoamsStreamName[] };
  private _states = new Map<LoamsStreamName, LiveState>();
  private _streams = new Map<LoamsStreamName, Stream>();
  private _listeners = new Set<(stream: LoamsStreamName) => void>();
  private _stopped = false;

  constructor(ctx: Context, config: LoamsLiveConfig = {}) {
    super(ctx, "loamsLive");
    this._config = {
      baseUrl: config.baseUrl ?? DEFAULT_BASE_URL,
      token: config.token ?? DEFAULT_TOKEN,
      idleTimeoutMs: config.idleTimeoutMs ?? 60_000,
      reconnectMinMs: config.reconnectMinMs ?? 500,
      reconnectMaxMs: config.reconnectMaxMs ?? 10_000,
      streams: config.streams ?? [...SERVICES],
    };
    for (const name of SERVICES) {
      this._states.set(name, initialState());
    }

    // No `ctx.on("dispose")` here: cordis already calls `dispose()` when the
    // plugin is unloaded, and subscribing to it as well double-runs the teardown.

    // One provider for every stream, so a widget names `source: "loams"` and
    // its `stream` picks what it reads.
    ctx.data.registerProvider({
      source: "loams",
      live: true,
      query: (widget) => this.query(widget),
    });
  }

  /** Starts reading. Called from the boot sequence, not the constructor. */
  start(): void {
    this._stopped = false;
    for (const name of this._config.streams) {
      if (!this._streams.has(name)) {
        this._streams.set(name, { backoff: this._config.reconnectMinMs });
        void this._open(name);
      }
    }
  }

  /** The current projection of one stream. */
  state<T extends LiveRecord = LiveRecord>(stream: LoamsStreamName): LiveState<T> {
    return (this._states.get(stream) ?? initialState()) as LiveState<T>;
  }

  /** Notified after every applied change, for a UI that wants to refetch. */
  onChange(listener: (stream: LoamsStreamName) => void): () => void {
    this._listeners.add(listener);
    return () => this._listeners.delete(listener);
  }

  /**
   * The provider's `query`.
   *
   * Applies `data.filter` as an equality projection so a tile can be scoped --
   * "pending approvals only" is the common case -- without the server needing
   * to know what a dashboard is.
   */
  async query(widget: any): Promise<{ data: Record<string, unknown>[]; rowcount: number }> {
    const source = widget?.data ?? {};
    const stream = source.stream as LoamsStreamName;
    if (!SERVICES.includes(stream)) {
      throw new Error(
        `unknown loams stream "${stream}": expected one of ${SERVICES.join(", ")}`,
      );
    }
    const state = this.state(stream);
    const filter = (source.filter ?? {}) as Record<string, unknown>;
    const rows = state.items.filter((item) =>
      Object.entries(filter).every(([key, want]) => {
        if (want === undefined || want === null || want === "") return true;
        const have = item[key];
        // The protos carry repeated and enum-ish values as arrays/scalars; a
        // loose comparison keeps `state == ["pending"]` working.
        if (Array.isArray(have)) return have.includes(want as never);
        return String(have) === String(want);
      }),
    );
    return { data: rows as Record<string, unknown>[], rowcount: rows.length };
  }

  private _notify(stream: LoamsStreamName): void {
    for (const listener of this._listeners) listener(stream);
  }

  private _apply(stream: LoamsStreamName, message: unknown): void {
    // `reduce`, not `apply`: `apply` wraps an explicit Change for callers that
    // already know what changed, and a raw wire message has no `kind`, so it
    // would fall through every branch and be folded in as a heartbeat.
    const next = reduce(this.state(stream), message as any);
    if (!next) return;
    this._states.set(stream, next);
    this._notify(stream);
  }

  private async _open(name: LoamsStreamName): Promise<void> {
    if (this._stopped) return;
    const stream = this._streams.get(name);
    if (!stream) return;

    // A server can hang up at any point -- that is what `/mock/drop-streams` is
    // for -- so the loop is the unit, not the connection.
    try {
      const state = this.state(name);
      const rpc = STREAM_RPCS[name];
      stream.handle = await watch(
        this._config.baseUrl,
        rpc.path,
        rpc.method,
        this._config.token,
        (message) => this._apply(name, message),
        { resumeCursor: state.cursor || undefined },
      );
      stream.backoff = this._config.reconnectMinMs;
      await stream.handle.done;
    } catch (error) {
      const detail =
        error instanceof LoamsStreamError
          ? `${error.code}: ${error.message}`
          : error instanceof Error
            ? error.message
            : String(error);
      // The stream stays usable while down: a tile shows the last known state
      // and an error rather than going blank.
      this._states.set(name, { ...this.state(name), connected: false, error: detail });
      this._notify(name);
    }

    if (this._stopped) return;
    const current = this._streams.get(name);
    if (!current) return;
    const wait = current.backoff;
    current.backoff = Math.min(current.backoff * 2, this._config.reconnectMaxMs);
    current.timer = setTimeout(() => void this._open(name), wait);
    current.timer.unref?.();
  }

  dispose(): void {
    this._stopped = true;
    for (const stream of this._streams.values()) {
      if (stream.timer) clearTimeout(stream.timer);
      stream.handle?.stop();
    }
    this._streams.clear();
    this._listeners.clear();
  }
}

declare module "cordis" {
  interface Context {
    loamsLive: LoamsLiveService;
  }
}