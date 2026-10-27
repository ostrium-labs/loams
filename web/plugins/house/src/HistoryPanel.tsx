// Query history (§49 §18.3): `ListQueryHistory(scope = MINE)` with status and text
// filters. History is the server's, shared across devices; the page keeps none of its
// own. "Open" fetches the full text with `GetQuery` (the list's is cut at 4 KiB) and
// loads it into the editor. A House that keeps no query log answers `unimplemented`:
// the panel then says history is not available there, which is not an error.

import { Button, Input, isNotServed, Select } from '@loams/ui';
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { type HistoryEntry, type HouseClient, type QueryStatus, toFailure } from './client.js';

const STATUSES: QueryStatus[] = ['FINISHED', 'FAILED', 'CANCELLED', 'RUNNING'];
const fmt = new Intl.NumberFormat('en');

export function HistoryPanel({
  client,
  refresh,
  onOpen,
}: {
  client: HouseClient;
  /** Bumped after each run, so the list shows it. */
  refresh: number;
  onOpen: (sql: string) => void;
}) {
  const id = useId();
  const [status, setStatus] = useState<QueryStatus | ''>('');
  const [text, setText] = useState('');
  const [items, setItems] = useState<HistoryEntry[]>([]);
  const [next, setNext] = useState('');
  const [error, setError] = useState<string>();
  const [unavailable, setUnavailable] = useState(false);
  const [loading, setLoading] = useState(false);
  // Only the latest request may touch the list: a slow response for an older filter
  // (or a "More" page from one) is dropped rather than shown under the new filter.
  const seq = useRef(0);

  const load = useCallback(
    async (pageToken?: string) => {
      const mine = ++seq.current;
      setLoading(true);
      setError(undefined);
      try {
        const page = await client.listQueryHistory({
          scope: 'MINE',
          status: status || undefined,
          textContains: text.trim() || undefined,
          pageSize: 20,
          pageToken,
        });
        if (mine !== seq.current) return;
        setItems((prev) => (pageToken ? [...prev, ...page.items] : page.items));
        setNext(page.nextPageToken);
      } catch (e) {
        if (mine !== seq.current) return;
        const f = toFailure(e);
        if (isNotServed(f)) setUnavailable(true);
        else setError(f.message);
      } finally {
        if (mine === seq.current) setLoading(false);
      }
    },
    [client, status, text],
  );

  // oxlint-disable-next-line react/exhaustive-deps -- refresh reloads on purpose
  useEffect(() => {
    void load();
  }, [load, refresh]);

  const open = async (h: HistoryEntry) => {
    try {
      const full = await client.getQuery(h.queryId);
      onOpen(full.query);
    } catch (e) {
      setError(toFailure(e).message);
    }
  };

  const title = (
    <h3 id={`${id}-title`} className="m-0 text-body">
      History
    </h3>
  );
  if (unavailable)
    return (
      <section aria-labelledby={`${id}-title`} className="flex flex-col gap-2">
        {title}
        <p className="m-0 text-body text-muted" role="status">
          Query history is not available on this server.
        </p>
      </section>
    );

  return (
    <section aria-labelledby={`${id}-title`} className="flex flex-col gap-2">
      {title}
      <div className="flex flex-wrap items-center gap-2">
        <Input
          aria-label="Filter history by text"
          placeholder="Contains…"
          className="min-w-0 flex-1"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <Select
          aria-label="Filter history by status"
          value={status}
          onChange={(e) => setStatus(e.target.value as QueryStatus | '')}
        >
          <option value="">Any status</option>
          {STATUSES.map((s) => (
            <option key={s} value={s}>
              {s.charAt(0) + s.slice(1).toLowerCase()}
            </option>
          ))}
        </Select>
      </div>
      {error && (
        <p className="m-0 text-body text-danger" role="alert">
          {error}
        </p>
      )}
      {!loading && !error && items.length === 0 && (
        <p className="m-0 text-body text-muted">No queries match.</p>
      )}
      <ul className="m-0 flex list-none flex-col gap-1 p-0" aria-label="Past queries">
        {items.map((h) => (
          <li
            key={h.queryId}
            className="flex flex-col gap-1 rounded-md border border-solid border-rule p-2"
          >
            <code className="line-clamp-3 whitespace-pre-wrap break-words text-[12px]">
              {h.query}
            </code>
            <div className="flex items-center gap-2 text-small text-muted">
              <span data-status={h.status}>
                {h.status.charAt(0) + h.status.slice(1).toLowerCase()}
                {h.errorCode ? ` (code ${h.errorCode})` : ''}
              </span>
              <span>{fmt.format(h.durationMs)} ms</span>
              <span>{fmt.format(h.resultRows)} rows</span>
              <Button
                size="sm"
                variant="quiet"
                className="ml-auto"
                aria-label={`Open in editor: ${h.query.slice(0, 60)}`}
                onClick={() => void open(h)}
              >
                Open
              </Button>
            </div>
          </li>
        ))}
      </ul>
      {next && (
        <Button size="sm" disabled={loading} onClick={() => void load(next)}>
          More
        </Button>
      )}
      {loading && (
        <p className="m-0 text-small text-muted" role="status">
          Loading history…
        </p>
      )}
    </section>
  );
}
