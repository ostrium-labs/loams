// Statement history (§48 §18.2): the last 100 statements per server, in the renderer's
// local storage. Only the statement text is kept, never its parameters: parameter
// values are where people put secrets and personal data. Storage is a convenience, so
// every read and write tolerates its absence.

export const HISTORY_LIMIT = 100;
const PREFIX = 'loams.graph.history.v1:';

export interface StatementHistory {
  /** Newest first. */
  list(): string[];
  add(statement: string): string[];
  clear(): void;
}

function defaultStorage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/** `server` names the server the statements ran on (its GetInstance `instance_id`). */
export function statementHistory(
  server: string,
  storage: Storage | undefined = defaultStorage(),
): StatementHistory {
  const key = PREFIX + (server || 'unknown');
  const read = (): string[] => {
    try {
      const raw: unknown = JSON.parse(storage?.getItem(key) ?? '[]');
      return Array.isArray(raw)
        ? raw.filter((s): s is string => typeof s === 'string').slice(0, HISTORY_LIMIT)
        : [];
    } catch {
      return [];
    }
  };
  return {
    list: read,
    add(statement) {
      const text = statement.trim();
      if (!text) return read();
      const next = [text, ...read().filter((s) => s !== text)].slice(0, HISTORY_LIMIT);
      try {
        storage?.setItem(key, JSON.stringify(next));
      } catch {
        // full or blocked storage: the history is a convenience only
      }
      return next;
    },
    clear() {
      try {
        storage?.removeItem(key);
      } catch {
        // as above
      }
    },
  };
}
