import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Select, Stat, Stats } from '@loams/ui';
import { useState } from 'react';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, mono, rel } from './common.js';
import type { GlitchtipIssue, GlitchtipOrg } from './dto.js';

export function GlitchtipPanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const orgs = useQuery<GlitchtipOrg[]>(desktop, 'glitchtip', 'organizations');
  const [picked, setPicked] = useState<string>();
  const list = orgs[0].state === 'ready' ? orgs[0].data : [];
  const orgSlug = picked ?? list[0]?.slug;
  const issues = useQuery<GlitchtipIssue[]>(
    desktop,
    'glitchtip',
    'issues',
    { orgSlug, limit: 20 },
    Boolean(orgSlug),
  );
  const loaded = issues[0].state === 'ready' ? issues[0].data : undefined;
  return (
    <div className="flex flex-col gap-4">
      <Stats>
        <Stat label="Organisations" value={orgs[0].state === 'ready' ? list.length : '…'} />
        <Stat label="Unresolved errors" value={loaded ? loaded.length : '…'} />
      </Stats>
      <QueryCard
        app="glitchtip"
        title="Unresolved issues"
        query={
          orgs[0].state === 'ready' && !orgSlug ? [{ state: 'ready', data: [] }, orgs[1]] : issues
        }
        actions={
          list.length > 0 && (
            <Select
              aria-label="Organisation"
              value={orgSlug ?? ''}
              onChange={(e) => setPicked(e.target.value)}
            >
              {list.map((o) => (
                <option key={o.slug} value={o.slug}>
                  {o.name ?? o.slug}
                </option>
              ))}
            </Select>
          )
        }
      >
        {(rows) => (
          <DataTable
            caption="Unresolved issues"
            rows={rows}
            rowKey={(r, i) => r.id ?? String(i)}
            empty="No unresolved issues"
            columns={[
              { key: 'id', header: 'Id', cell: (r) => mono(r.id) },
              { key: 'title', header: 'Issue', cell: (r) => r.title },
              { key: 'level', header: 'Level', cell: (r) => r.level },
              { key: 'count', header: 'Events', numeric: true, cell: (r) => r.count },
              { key: 'seen', header: 'Last seen', cell: (r) => rel(r.lastSeen) },
            ]}
          />
        )}
      </QueryCard>
    </div>
  );
}
