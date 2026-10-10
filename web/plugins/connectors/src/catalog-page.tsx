import type { ConnectorSummary, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Badge, Button, Card, Empty, Input, Notice, StatusTag } from '@loams/ui';
import { useEffect, useMemo, useState } from 'react';
import { PageHead } from './page-head.js';

export type Load<T> =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; data: T };

export function useCatalog(desktop: LoamsDesktopApi): Load<ConnectorSummary[]> {
  const [v, setV] = useState<Load<ConnectorSummary[]>>({ state: 'loading' });
  useEffect(() => {
    let live = true;
    desktop.connectors
      .catalog()
      .then((data) => live && setV({ state: 'ready', data }))
      .catch(
        (e) =>
          live && setV({ state: 'error', message: e instanceof Error ? e.message : String(e) }),
      );
    return () => {
      live = false;
    };
  }, [desktop]);
  return v;
}

const STATUS_TONE = {
  preview: 'progress',
  planned: 'planned',
  ga: 'done',
  stable: 'done',
} as const;
export const statusTone = (s: string) => STATUS_TONE[s as keyof typeof STATUS_TONE] ?? 'neutral';

function Chip({ on, onClick, children }: { on: boolean; onClick: () => void; children: string }) {
  return (
    <Button size="sm" variant={on ? 'primary' : 'secondary'} aria-pressed={on} onClick={onClick}>
      {children}
    </Button>
  );
}

/** `/connectors`: search, filter chips and a card grid of the catalog. */
export function CatalogPage({
  desktop,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  navigate: (to: string) => void;
}) {
  const catalog = useCatalog(desktop);
  const [q, setQ] = useState('');
  const [category, setCategory] = useState('');
  const [status, setStatus] = useState('');
  const all = catalog.state === 'ready' ? catalog.data : [];
  const categories = useMemo(() => [...new Set(all.map((c) => c.category))].sort(), [all]);
  const statuses = useMemo(() => [...new Set(all.map((c) => c.status))].sort(), [all]);
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return all
      .filter(
        (c) =>
          (!category || c.category === category) &&
          (!status || c.status === status) &&
          (!needle ||
            c.id.includes(needle) ||
            c.name.toLowerCase().includes(needle) ||
            c.category.includes(needle)),
      )
      .sort((a, b) => Number(a.stub) - Number(b.stub) || a.name.localeCompare(b.name));
  }, [all, q, category, status]);

  return (
    <div className="lc-page">
      <PageHead
        title="Connectors"
        subtitle={
          catalog.state === 'ready'
            ? `${all.length} connectors, ${all.filter((c) => !c.stub).length} with a config schema.`
            : 'Sources and sinks for the Loams fabric.'
        }
        actions={
          <Button variant="primary" disabled title="Connector runtime not yet available (CN1)">
            New instance
          </Button>
        }
      />
      {catalog.state === 'loading' && <p className="text-sm text-muted">Loading…</p>}
      {catalog.state === 'error' && (
        <Notice tone="danger" title="Could not load the connector catalog">
          {catalog.message}
        </Notice>
      )}
      {catalog.state === 'ready' && (
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-3">
            <Input
              type="search"
              aria-label="Search connectors"
              placeholder="Search by name, id or category"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
            <fieldset
              className="m-0 flex flex-wrap gap-2 border-0 p-0"
              aria-label="Filter by category"
            >
              <Chip on={!category} onClick={() => setCategory('')}>
                All categories
              </Chip>
              {categories.map((c) => (
                <Chip
                  key={c}
                  on={category === c}
                  onClick={() => setCategory(category === c ? '' : c)}
                >
                  {c}
                </Chip>
              ))}
            </fieldset>
            <fieldset
              className="m-0 flex flex-wrap gap-2 border-0 p-0"
              aria-label="Filter by status"
            >
              <Chip on={!status} onClick={() => setStatus('')}>
                Any status
              </Chip>
              {statuses.map((s) => (
                <Chip key={s} on={status === s} onClick={() => setStatus(status === s ? '' : s)}>
                  {s}
                </Chip>
              ))}
            </fieldset>
          </div>
          {shown.length === 0 ? (
            <Empty title="No connectors match">Clear the search or a filter.</Empty>
          ) : (
            <ul className="m-0 grid list-none grid-cols-1 gap-3 p-0 md:grid-cols-2 xl:grid-cols-3">
              {shown.map((c) => (
                <li key={c.id} className="min-w-0">
                  <article aria-label={c.name} className="h-full">
                    <Card
                      title={c.name}
                      headingLevel={3}
                      className="h-full"
                      actions={<StatusTag status={statusTone(c.status)}>{c.status}</StatusTag>}
                    >
                      <div className="flex flex-col gap-3">
                        <p className="truncate font-mono text-xs text-faint">{c.id}</p>
                        <div className="flex flex-wrap gap-1">
                          <Badge>{c.category}</Badge>
                          <Badge>{c.runtime.kind}</Badge>
                          {c.modes.map((m) => (
                            <Badge key={m}>{m}</Badge>
                          ))}
                        </div>
                        <div>
                          <Button size="sm" onClick={() => navigate(`/connectors/${c.id}`)}>
                            Details
                          </Button>
                        </div>
                      </div>
                    </Card>
                  </article>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
