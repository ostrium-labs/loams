import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, label, metricKeys, mono, rel } from './common.js';
import type { LangfuseDaily, LangfuseTrace } from './dto.js';

const usd = (n?: number) => (n === undefined ? '' : `$${n.toFixed(4)}`);

export function LangfusePanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const traces = useQuery<LangfuseTrace[]>(desktop, 'langfuse', 'traces', { limit: 20 });
  const daily = useQuery<LangfuseDaily>(desktop, 'langfuse', 'daily');
  return (
    <div className="flex flex-col gap-4">
      <QueryCard app="langfuse" title="Recent traces" query={traces}>
        {(rows) => (
          <DataTable
            caption="Recent traces"
            rows={rows}
            rowKey={(r, i) => r.id ?? String(i)}
            empty="No traces"
            columns={[
              { key: 'id', header: 'Id', cell: (r) => mono(r.id?.slice(0, 12)) },
              { key: 'name', header: 'Name', cell: (r) => r.name },
              { key: 'level', header: 'Level', cell: (r) => r.level },
              { key: 'cost', header: 'Cost', numeric: true, cell: (r) => usd(r.totalCost) },
              { key: 'start', header: 'Started', cell: (r) => rel(r.startTime) },
            ]}
          />
        )}
      </QueryCard>
      <QueryCard app="langfuse" title="Daily usage" query={daily}>
        {(d) => {
          if (!d.available) {
            return (
              <p className="p-4 text-sm text-muted">
                Daily metrics are not available on this Langfuse version.
              </p>
            );
          }
          const cols = metricKeys(d.days).slice(0, 5);
          return (
            <DataTable
              caption="Daily usage"
              rows={d.days}
              rowKey={(r, i) => String(r.date ?? i)}
              empty="No usage yet"
              columns={[
                { key: 'date', header: 'Date', cell: (r) => mono(r.date) },
                ...cols.map((k) => ({
                  key: k,
                  header: label(k),
                  numeric: true,
                  cell: (r: LangfuseDaily['days'][number]) => String(r[k] ?? ''),
                })),
              ]}
            />
          );
        }}
      </QueryCard>
    </div>
  );
}
