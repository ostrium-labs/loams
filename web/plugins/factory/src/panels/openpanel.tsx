import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Stat, Stats } from '@loams/ui';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, label, metricKeys, mono, num } from './common.js';
import type { OpenPanelInsights } from './dto.js';

export function OpenPanelPanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const insights = useQuery<OpenPanelInsights>(desktop, 'openpanel', 'insights', { limit: 10 });
  return (
    <div className="flex flex-col gap-4">
      <QueryCard app="openpanel" title="Overview" query={insights}>
        {(d) => {
          const summary = Object.entries(d.summary ?? {});
          const cols = metricKeys(d.series ?? []);
          return (
            <div className="flex flex-col gap-4 pb-2">
              {summary.length > 0 && (
                <Stats>
                  {summary.map(([k, v]) => (
                    <Stat key={k} label={label(k)} value={num(v)} />
                  ))}
                </Stats>
              )}
              <DataTable
                caption="Top pages"
                rows={d.topPages ?? []}
                rowKey={(r, i) => r.path ?? String(i)}
                empty="No page data"
                columns={[
                  { key: 'path', header: 'Page', cell: (r) => mono(r.path) },
                  { key: 's', header: 'Sessions', numeric: true, cell: (r) => num(r.sessions) },
                  { key: 'p', header: 'Pageviews', numeric: true, cell: (r) => num(r.pageviews) },
                ]}
              />
              {cols.length > 0 && (
                <DataTable
                  caption="Daily series"
                  rows={d.series}
                  rowKey={(r, i) => String(r.date ?? i)}
                  empty="No series"
                  columns={[
                    { key: 'date', header: 'Date', cell: (r) => mono(r.date) },
                    ...cols.map((k) => ({
                      key: k,
                      header: label(k),
                      numeric: true,
                      cell: (r: OpenPanelInsights['series'][number]) =>
                        typeof r[k] === 'number' ? num(r[k] as number) : '',
                    })),
                  ]}
                />
              )}
            </div>
          );
        }}
      </QueryCard>
    </div>
  );
}
