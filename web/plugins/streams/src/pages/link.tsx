import { Badge, Button, Card, Empty, Table } from '@loams/ui';
import type { LinkDetail, StreamsClient } from '../client.js';
import { ErrorNotice, PageHead, useLoad } from '../shared.js';
import { StatusBadge } from './list.js';

/** Lag per partition as inline SVG bars; the tallest bar fills the chart. */
export function LagChart({ lag }: { lag: { partition: number; records: string }[] }) {
  if (lag.length === 0) return <p className="lc-muted">No partitions.</p>;
  const big = lag.map((l) => ({ ...l, n: BigInt(l.records) }));
  const max = big.reduce((m, l) => (l.n > m ? l.n : m), 1n);
  const H = 120;
  const slot = 56;
  const width = lag.length * slot + 8;
  return (
    <svg
      role="img"
      aria-label="Lag per partition"
      width={width}
      height={H + 36}
      viewBox={`0 0 ${width} ${H + 36}`}
      className="max-w-full"
    >
      {big.map((l, i) => {
        const h = l.n === 0n ? 1 : Math.max(2, Number((l.n * BigInt(H)) / max));
        const x = 8 + i * slot;
        return (
          <g key={l.partition} data-testid={`lag-bar-${l.partition}`}>
            <title>{`Partition ${l.partition}: ${l.records} records behind`}</title>
            <rect
              x={x}
              y={H - h + 16}
              width={slot - 14}
              height={h}
              data-records={l.records}
              className={l.n === 0n ? 'fill-grow' : 'fill-accent'}
            />
            <text
              x={x + (slot - 14) / 2}
              y={12}
              textAnchor="middle"
              className="fill-ink text-[11px]"
            >
              {l.records}
            </text>
            <text
              x={x + (slot - 14) / 2}
              y={H + 32}
              textAnchor="middle"
              className="fill-muted text-[11px]"
            >
              {`p${l.partition}`}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

export function LinkPage({
  client,
  ns,
  name,
  navigate,
}: {
  client: StreamsClient;
  ns: string;
  name: string;
  navigate: (to: string) => void;
}) {
  const [link, reload] = useLoad<LinkDetail>(
    () => client.describeLink(ns, name),
    [client, ns, name],
  );
  const back = `/streams/${encodeURIComponent(ns)}/links`;
  return (
    <div className="lc-page max-w-[1180px]">
      <PageHead
        crumbs={
          <>
            <a href={`#${back}`}>Streams & Links</a> / <code>{ns}</code>
          </>
        }
        title={<code>{name}</code>}
        subtitle={link.state === 'ready' ? <StatusBadge status={link.data.status} /> : undefined}
        actions={
          <>
            <Button onClick={() => navigate(back)}>Back</Button>
            <Button onClick={reload}>Refresh</Button>
          </>
        }
      />
      {link.state === 'loading' && <p className="lc-muted">Loading link…</p>}
      {link.state === 'error' && (
        <ErrorNotice title="Could not load the link" message={link.message} />
      )}
      {link.state === 'ready' && (
        <>
          <Card title="Link">
            <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2">
              <dt className="text-muted">Source</dt>
              <dd className="m-0">
                <a
                  className="ds-link"
                  href={`#/streams/${encodeURIComponent(ns)}/stream/${encodeURIComponent(link.data.source)}`}
                >
                  <code>{link.data.source}</code>
                </a>
              </dd>
              <dt className="text-muted">Target</dt>
              <dd className="m-0">
                <code>
                  {link.data.target.kind}/{link.data.target.name}
                </code>
              </dd>
              {link.data.version !== undefined && (
                <>
                  <dt className="text-muted">Version</dt>
                  <dd className="m-0">
                    <Badge>{String(link.data.version)}</Badge>
                  </dd>
                </>
              )}
              <dt className="text-muted">Options</dt>
              <dd className="m-0">
                <code>{JSON.stringify(link.data.options)}</code>
              </dd>
            </dl>
          </Card>
          <Card title="Lag (records behind the source)">
            <LagChart lag={link.data.lag} />
          </Card>
          <Card title="Applied offsets" flush>
            <Table<{ partition: number; offset: string }>
              caption="Applied offsets"
              rows={link.data.applied ?? []}
              rowKey={(a) => String(a.partition)}
              empty={<Empty title="Nothing applied yet">No partition has been applied.</Empty>}
              columns={[
                { key: 'p', header: 'Partition', numeric: true, cell: (a) => String(a.partition) },
                {
                  key: 'o',
                  header: 'Applied offset',
                  numeric: true,
                  cell: (a) => <code>{a.offset}</code>,
                },
              ]}
            />
          </Card>
        </>
      )}
    </div>
  );
}
