import type { ConnectorSummary, LoamsDesktopApi } from '@loams/desktop/contracts';
import {
  Badge,
  Button,
  Card,
  Empty,
  LoadingSwap,
  Notice,
  SearchInput,
  SkeletonList,
  StaggerItem,
  StaggerList,
  StatusTag,
  Toggle,
} from '@loams/ui';
import type { ReactNode } from 'react';
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

/** Native = first-party on `loams-connector-sdk` (CN2 OR-1); anything else is a declared fallback
 * (Camel, Debezium Server, an Iggy upstream plugin, OR-4). Planned stubs have no runtime class yet. */
export const runtimeClass = (c: ConnectorSummary): 'native' | 'fallback' =>
  c.runtime.kind === 'native' ? 'native' : 'fallback';

export type DirectionFilter = '' | 'source' | 'sink';
export type RuntimeFilter = '' | 'native' | 'fallback';

export interface CatalogFilter {
  q: string;
  family: string;
  status: string;
  direction: DirectionFilter;
  runtime: RuntimeFilter;
}

/** The catalog's filter, as a pure function (tested without rendering). */
export function filterCatalog(all: ConnectorSummary[], f: CatalogFilter): ConnectorSummary[] {
  const needle = f.q.trim().toLowerCase();
  return all
    .filter(
      (c) =>
        (!f.family || c.category === f.family) &&
        (!f.status || c.status === f.status) &&
        (!f.direction || (f.direction === 'source' ? c.source !== null : c.sink !== null)) &&
        (!f.runtime || runtimeClass(c) === f.runtime) &&
        (!needle ||
          c.id.includes(needle) ||
          c.name.toLowerCase().includes(needle) ||
          c.category.includes(needle)),
    )
    .sort((a, b) => Number(a.stub) - Number(b.stub) || a.name.localeCompare(b.name));
}

/** A filter chip: a quiet toggle, pressed for the active choice (not a solid accent fill). */
function Chip({ on, onClick, children }: { on: boolean; onClick: () => void; children: string }) {
  return (
    <Toggle
      size="sm"
      pressed={on}
      onPressedChange={onClick}
      className="data-pressed:border-control-border data-pressed:bg-raised!"
    >
      {children}
    </Toggle>
  );
}

/** One labelled row of chips; the fieldset is the group assistive technology hears. */
function ChipRow({ label, name, children }: { label: string; name: string; children: ReactNode }) {
  return (
    <fieldset className="m-0 flex min-w-0 items-start gap-2 border-0 p-0" aria-label={name}>
      <span aria-hidden="true" className="w-16 shrink-0 pt-1.5 text-small text-faint">
        {label}
      </span>
      <div className="flex min-w-0 flex-wrap gap-1">{children}</div>
    </fieldset>
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
  const [direction, setDirection] = useState<DirectionFilter>('');
  const [runtime, setRuntime] = useState<RuntimeFilter>('');
  const all = catalog.state === 'ready' ? catalog.data : [];
  const categories = useMemo(() => [...new Set(all.map((c) => c.category))].sort(), [all]);
  const statuses = useMemo(() => [...new Set(all.map((c) => c.status))].sort(), [all]);
  const shown = useMemo(
    () => filterCatalog(all, { q, family: category, status, direction, runtime }),
    [all, q, category, status, direction, runtime],
  );

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
          <Button variant="primary" onClick={() => navigate('/connectors/instances')}>
            Running instances
          </Button>
        }
      />
      {catalog.state === 'error' ? (
        <Notice tone="danger" title="Could not load the connector catalog">
          {catalog.message}
        </Notice>
      ) : (
        <LoadingSwap
          loading={catalog.state === 'loading'}
          skeleton={<SkeletonList label="Loading the connector catalog" layout="cards" rows={6} />}
        >
          <div className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <SearchInput
                label="Search connectors"
                placeholder="Search by name, id or category"
                value={q}
                onValueChange={setQ}
                className="max-w-xl"
              />
              <ChipRow label="Family" name="Filter by family">
                <Chip on={!category} onClick={() => setCategory('')}>
                  All families
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
              </ChipRow>
              <div className="flex flex-wrap gap-x-6 gap-y-2">
                <ChipRow label="Direction" name="Filter by direction">
                  <Chip on={!direction} onClick={() => setDirection('')}>
                    Source or sink
                  </Chip>
                  {(['source', 'sink'] as const).map((d) => (
                    <Chip
                      key={d}
                      on={direction === d}
                      onClick={() => setDirection(direction === d ? '' : d)}
                    >
                      {d}
                    </Chip>
                  ))}
                </ChipRow>
                <ChipRow label="Runtime" name="Filter by runtime">
                  <Chip on={!runtime} onClick={() => setRuntime('')}>
                    Any runtime
                  </Chip>
                  {(['native', 'fallback'] as const).map((r) => (
                    <Chip
                      key={r}
                      on={runtime === r}
                      onClick={() => setRuntime(runtime === r ? '' : r)}
                    >
                      {r}
                    </Chip>
                  ))}
                </ChipRow>
                <ChipRow label="Status" name="Filter by status">
                  <Chip on={!status} onClick={() => setStatus('')}>
                    Any status
                  </Chip>
                  {statuses.map((s) => (
                    <Chip
                      key={s}
                      on={status === s}
                      onClick={() => setStatus(status === s ? '' : s)}
                    >
                      {s}
                    </Chip>
                  ))}
                </ChipRow>
              </div>
            </div>
            {shown.length === 0 ? (
              <Empty
                title="No connectors match"
                actions={
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => {
                      setQ('');
                      setCategory('');
                      setStatus('');
                      setDirection('');
                      setRuntime('');
                    }}
                  >
                    Clear filters
                  </Button>
                }
              >
                Nothing in the catalog fits the search and these filters.
              </Empty>
            ) : (
              <StaggerList className="m-0 grid list-none grid-cols-1 gap-3 p-0 md:grid-cols-2 xl:grid-cols-3">
                {shown.map((c, i) => (
                  <StaggerItem key={c.id} index={i} className="min-w-0">
                    <article aria-label={c.name} className="h-full">
                      <Card
                        title={c.name}
                        headingLevel={3}
                        className="h-full"
                        actions={<StatusTag status={statusTone(c.status)}>{c.status}</StatusTag>}
                      >
                        <div className="flex flex-col gap-3">
                          <p
                            className={/* mono-ok: id */ 'truncate font-mono text-small text-faint'}
                          >
                            {c.id}
                          </p>
                          <div className="flex flex-wrap gap-1">
                            <Badge>{c.category}</Badge>
                            <Badge title={`runtime ${c.runtime.kind}`}>{runtimeClass(c)}</Badge>
                            {c.source && <Badge>source</Badge>}
                            {c.sink && <Badge>sink</Badge>}
                            {c.modes.map((m) => (
                              <Badge key={m}>{m}</Badge>
                            ))}
                          </div>
                          <div className="flex gap-2">
                            <Button size="sm" onClick={() => navigate(`/connectors/${c.id}`)}>
                              Details
                            </Button>
                            {!c.stub && (
                              <Button
                                size="sm"
                                variant="secondary"
                                aria-label={`Create ${c.name} instance`}
                                onClick={() => navigate(`/connectors/${c.id}/new`)}
                              >
                                Create
                              </Button>
                            )}
                          </div>
                        </div>
                      </Card>
                    </article>
                  </StaggerItem>
                ))}
              </StaggerList>
            )}
          </div>
        </LoadingSwap>
      )}
    </div>
  );
}
