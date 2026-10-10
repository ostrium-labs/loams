import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Stat, Stats } from '@loams/ui';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, label, mono, num, rel } from './common.js';
import type { PlaneIssue } from './dto.js';

export function PlanePanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const stats = useQuery<Record<string, number>>(desktop, 'plane', 'stats');
  const issues = useQuery<PlaneIssue[]>(desktop, 'plane', 'issues', { limit: 20 });
  return (
    <div className="flex flex-col gap-4">
      <QueryCard app="plane" title="Project stats" query={stats}>
        {(s) => {
          const entries = Object.entries(s);
          return entries.length === 0 ? (
            <p className="p-4 text-sm text-muted">No stats yet.</p>
          ) : (
            <Stats>
              {entries.map(([k, v]) => (
                <Stat key={k} label={label(k)} value={num(v)} />
              ))}
            </Stats>
          );
        }}
      </QueryCard>
      <QueryCard app="plane" title="Issues" query={issues}>
        {(rows) => (
          <DataTable
            caption="Issues"
            rows={rows}
            rowKey={(r, i) => r.id ?? String(i)}
            empty="No issues"
            columns={[
              { key: 'id', header: 'Id', cell: (r) => mono(r.id) },
              { key: 'title', header: 'Title', cell: (r) => r.title },
              { key: 'prio', header: 'Priority', cell: (r) => r.priority },
              { key: 'upd', header: 'Updated', cell: (r) => rel(r.updatedAt) },
            ]}
          />
        )}
      </QueryCard>
    </div>
  );
}
