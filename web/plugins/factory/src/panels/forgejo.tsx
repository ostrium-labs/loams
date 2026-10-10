import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Select, Stat, Stats } from '@loams/ui';
import { useState } from 'react';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, mono, num, rel } from './common.js';
import type { ForgejoIssue, ForgejoRepo } from './dto.js';

const issueColumns = [
  { key: 'id', header: 'Id', cell: (i: ForgejoIssue) => mono(i.id) },
  { key: 'title', header: 'Title', cell: (i: ForgejoIssue) => i.title },
  { key: 'state', header: 'State', cell: (i: ForgejoIssue) => i.state },
  { key: 'updated', header: 'Updated', cell: (i: ForgejoIssue) => rel(i.updatedAt) },
];

export function ForgejoPanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const repos = useQuery<ForgejoRepo[]>(desktop, 'forgejo', 'repos', { limit: 20 });
  const version = useQuery<{ version?: string }>(desktop, 'forgejo', 'version');
  const [picked, setPicked] = useState<string>();
  const list = repos[0].state === 'ready' ? repos[0].data : [];
  const full = picked ?? list.find((r) => r.fullName?.includes('/'))?.fullName;
  const [owner, repo] = full?.split('/') ?? [];
  const pulls = useQuery<ForgejoIssue[]>(
    desktop,
    'forgejo',
    'issues',
    { type: 'pulls', owner, repo, limit: 20 },
    repos[0].state === 'ready' && Boolean(owner && repo),
  );
  const issues = useQuery<ForgejoIssue[]>(desktop, 'forgejo', 'issues', { limit: 20 });
  return (
    <div className="flex flex-col gap-4">
      <Stats>
        <Stat label="Repositories" value={repos[0].state === 'ready' ? list.length : '…'} />
        <Stat
          label="Open issues"
          value={
            repos[0].state === 'ready'
              ? num(list.reduce((n, r) => n + (r.openIssues ?? 0), 0))
              : '…'
          }
        />
        <Stat
          label="Forgejo version"
          value={version[0].state === 'ready' ? (version[0].data.version ?? 'unknown') : '…'}
        />
      </Stats>
      <QueryCard app="forgejo" title="Repositories" query={repos}>
        {(rows) => (
          <DataTable
            caption="Repositories"
            rows={rows}
            rowKey={(r, i) => r.fullName ?? String(i)}
            empty="No repositories"
            columns={[
              { key: 'name', header: 'Repository', cell: (r) => mono(r.fullName) },
              { key: 'desc', header: 'Description', cell: (r) => r.description },
              { key: 'stars', header: 'Stars', numeric: true, cell: (r) => num(r.stars) },
              { key: 'forks', header: 'Forks', numeric: true, cell: (r) => num(r.forks) },
              { key: 'issues', header: 'Issues', numeric: true, cell: (r) => num(r.openIssues) },
              { key: 'upd', header: 'Updated', cell: (r) => rel(r.updatedAt) },
            ]}
          />
        )}
      </QueryCard>
      <QueryCard
        app="forgejo"
        title="Pull requests"
        query={
          repos[0].state === 'ready' && !owner ? [{ state: 'ready', data: [] }, repos[1]] : pulls
        }
        actions={
          list.length > 0 && (
            <Select
              aria-label="Repository"
              value={full ?? ''}
              onChange={(e) => setPicked(e.target.value)}
            >
              {list.map((r) => (
                <option key={r.fullName}>{r.fullName}</option>
              ))}
            </Select>
          )
        }
      >
        {(rows) => (
          <DataTable
            caption="Open pull requests"
            rows={rows}
            rowKey={(r, i) => r.id ?? String(i)}
            empty="No open pull requests"
            columns={issueColumns}
          />
        )}
      </QueryCard>
      <QueryCard app="forgejo" title="Issues" query={issues}>
        {(rows) => (
          <DataTable
            caption="Issues"
            rows={rows}
            rowKey={(r, i) => r.id ?? String(i)}
            empty="No issues"
            columns={issueColumns}
          />
        )}
      </QueryCard>
    </div>
  );
}
