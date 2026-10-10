/**
 * The current state of one Loams watch stream.
 *
 * A watch stream is snapshot-then-changes, not a table you can scan, so this is
 * the whole projection: the latest snapshot, the changes applied on top of it,
 * and the cursor needed to resume. It follows the same rules the console's
 * approvals inbox does, deliberately, so the two do not drift.
 */

export interface LiveRecord {
  id: string;
  [field: string]: unknown;
}

export interface LiveState<T extends LiveRecord = LiveRecord> {
  items: T[];
  cursor: string;
  connected: boolean;
  error?: string;
  /** Bumped on every applied change, so a subscriber can tell "no change". */
  revision: number;
}

type Change<T extends LiveRecord = LiveRecord> =
  | { kind: "snapshot"; items: T[]; cursor: string }
  | { kind: "upsert"; item: T; cursor: string }
  | { kind: "remove"; id: string; cursor: string }
  | { kind: "cursor"; cursor: string };

/**
 * Folds one `Watch*` message into state.
 *
 * Returns `undefined` when the message carried no change. A heartbeat is the
 * case that matters: it proves the stream is open, not that the list resynced,
 * so it must not clear a stale error or count as a revision.
 */
export function reduce<T extends LiveRecord>(state: LiveState<T>, message: any): LiveState<T> | undefined {
  if (!message || typeof message !== "object") return undefined;
  const cursor = typeof message.cursor === "string" ? message.cursor : state.cursor;
  const event = message.event ?? message;

  const snapshot = event.snapshot;
  if (snapshot) {
    const items = normalizeList<T>(snapshot);
    return { ...state, items, cursor, connected: true, error: undefined, revision: state.revision + 1 };
  }

  const upsert = event.upsert;
  if (upsert) {
    const item = normalize<T>(upsert);
    if (!item) return undefined;
    const at = state.items.findIndex((existing) => existing.id === item.id);
    const items = at === -1
      ? [...state.items, item]
      : state.items.map((existing, i) => (i === at ? item : existing));
    return { ...state, items, cursor, connected: true, error: undefined, revision: state.revision + 1 };
  }

  const remove = event.remove;
  if (remove) {
    const id = typeof remove === "string" ? remove : remove.id;
    if (typeof id !== "string") return undefined;
    const items = state.items.filter((existing) => existing.id !== id);
    if (items.length === state.items.length) return undefined;
    return { ...state, items, cursor, connected: true, revision: state.revision + 1 };
  }

  if (event.heartbeat) {
    // Connected, but not a change. Moving the cursor is the whole effect.
    return cursor === state.cursor ? undefined : { ...state, cursor, connected: true };
  }

  return undefined;
}

/**
 * Normalises one entity into a flat row.
 *
 * Flat because the dashboard's renderers read columns: `compileNativeWidget`
 * indexes rows by field name, and a React Flow node's `labelField` looks up a
 * top-level key. Nesting the proto shape would make every widget need to know
 * where in the tree its data lives.
 */
function normalize<T extends LiveRecord>(entity: any): T | undefined {
  if (!entity || typeof entity !== "object") return undefined;
  const id = entity.id;
  if (typeof id !== "string" || id === "") return undefined;

  const row: Record<string, unknown> = { id };
  for (const [key, value] of Object.entries(entity)) {
    if (key === "id") continue;
    row[key] = value ?? null;
  }
  return row as T;
}

/** A snapshot's payload varies by service, so take the first list it carries. */
function normalizeList<T extends LiveRecord>(snapshot: any): T[] {
  const list = Array.isArray(snapshot) ? snapshot : Object.values(snapshot ?? {}).find(Array.isArray);
  if (!Array.isArray(list)) return [];
  return list.map((entity) => normalize<T>(entity)).filter((row): row is T => row !== undefined);
}

export function initialState<T extends LiveRecord>(): LiveState<T> {
  return { items: [], cursor: "", connected: false, revision: 0 };
}

/** Applies a change, returning the same object when nothing moved. */
export function apply<T extends LiveRecord>(state: LiveState<T>, change: Change): LiveState<T> {
  const next = reduce<T>(state, {
    cursor: change.cursor,
    event:
      change.kind === "snapshot"
        ? { snapshot: change.items }
        : change.kind === "upsert"
          ? { upsert: change.item }
          : change.kind === "remove"
            ? { remove: change.id }
            : { heartbeat: {} },
  });
  return next ?? state;
}