// In-memory ports for tests and the local dry run. `throttleEvery` makes every
// nth call fail with Throttled, to test the 429 path.
import { Throttled } from "../app.mjs";

export function memoryEnv({ throttleEvery = 0, onSend } = {}) {
  const rows = new Map(); // `${pk}\u0000${sk}` -> { value, version }
  let calls = 0;
  const maybeThrottle = () => {
    calls += 1;
    if (throttleEvery && calls % throttleEvery === 0) throw new Throttled("memory throttle");
  };
  const sent = [];
  return {
    target: "local",
    sent,
    items: {
      async get({ pk, sk }) {
        maybeThrottle();
        const row = rows.get(`${pk}\u0000${sk}`);
        return row ? { key: { pk, sk }, value: row.value, version: row.version } : null;
      },
      async put({ pk, sk }, value) {
        maybeThrottle();
        const version = String(Date.now()) + Math.random().toString(16).slice(2, 6);
        rows.set(`${pk}\u0000${sk}`, { value, version });
        return { version };
      },
      async list(pk, { after, limit = 100 } = {}) {
        maybeThrottle();
        const keys = [...rows.keys()].filter((k) => k.startsWith(`${pk}\u0000`)).map((k) => k.slice(pk.length + 1)).sort();
        const start = after ? keys.findIndex((k) => k > after) : 0;
        const page = start < 0 ? [] : keys.slice(start, start + limit);
        const items = page.map((sk) => ({ key: { pk, sk }, ...rows.get(`${pk}\u0000${sk}`) }));
        const next = start >= 0 && start + limit < keys.length ? page.at(-1) : undefined;
        return { items, next };
      },
    },
    queue: {
      async send(queue, event) {
        sent.push({ queue, event });
        if (onSend) await onSend(queue, event);
      },
    },
  };
}
