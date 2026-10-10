import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, label, metricKeys, mono, num } from './common.js';
import type { MatomoPage, MatomoVisit } from './dto.js';

export function MatomoPanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const visits = useQuery<MatomoVisit[]>(desktop, 'matomo', 'visits');
  const pages = useQuery<MatomoPage[]>(desktop, 'matomo', 'pages', { limit: 10 });
  return (
    <div className="flex flex-col gap-4">
      <QueryCard app="matomo" title="Visits" query={visits}>
        {(rows) => {
          const cols = metricKeys(rows).slice(0, 6);
          return (
            <DataTable
              caption="Visits, last 7 days"
              rows={rows}
              rowKey={(r, i) => String(r.date ?? i)}
              empty="No visits"
              columns={[
                { key: 'date', header: 'Date', cell: (r) => mono(r.date) },
                ...cols.map((k) => ({
                  key: k,
                  header: label(k),
                  numeric: true,
                  cell: (r: MatomoVisit) => (typeof r[k] === 'number' ? num(r[k] as number) : ''),
                })),
              ]}
            />
          );
        }}
      </QueryCard>
      <QueryCard app="matomo" title="Top pages" query={pages}>
        {(rows) => (
          <DataTable
            caption="Top pages today"
            rows={rows}
            rowKey={(r, i) => r.label ?? String(i)}
            empty="No page data"
            columns={[
              { key: 'label', header: 'Page', cell: (r) => mono(r.label) },
              { key: 'hits', header: 'Hits', numeric: true, cell: (r) => num(r.hits) },
              { key: 'visits', header: 'Visits', numeric: true, cell: (r) => num(r.visits) },
            ]}
          />
        )}
      </QueryCard>
    </div>
  );
}
