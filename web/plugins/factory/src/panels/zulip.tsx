import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Select, Stat, Stats } from '@loams/ui';
import { useState } from 'react';
import { QueryCard, useQuery } from '../model.js';
import { DataTable, mono } from './common.js';
import type { ZulipMessage, ZulipStream } from './dto.js';

export function ZulipPanel({ desktop }: { desktop: LoamsDesktopApi }) {
  const streams = useQuery<ZulipStream[]>(desktop, 'zulip', 'streams');
  const server = useQuery<{ name?: string; email?: string }>(desktop, 'zulip', 'server');
  const [picked, setPicked] = useState<string>();
  const list = streams[0].state === 'ready' ? streams[0].data : [];
  const channel = picked ?? list[0]?.name;
  const messages = useQuery<ZulipMessage[]>(
    desktop,
    'zulip',
    'messages',
    { channel, limit: 20 },
    Boolean(channel),
  );
  return (
    <div className="flex flex-col gap-4">
      <Stats>
        <Stat label="Channels" value={streams[0].state === 'ready' ? list.length : '…'} />
        <Stat
          label="Signed in as"
          value={server[0].state === 'ready' ? (server[0].data.name ?? 'unknown') : '…'}
          detail={server[0].state === 'ready' ? server[0].data.email : undefined}
        />
      </Stats>
      <QueryCard app="zulip" title="Channels" query={streams}>
        {(rows) => (
          <DataTable
            caption="Channels"
            rows={rows}
            rowKey={(r, i) => String(r.id ?? i)}
            empty="No channels"
            columns={[
              { key: 'name', header: 'Channel', cell: (r) => mono(r.name) },
              { key: 'desc', header: 'Description', cell: (r) => r.description },
              {
                key: 'priv',
                header: 'Visibility',
                cell: (r) => (r.private ? 'Private' : 'Public'),
              },
            ]}
          />
        )}
      </QueryCard>
      <QueryCard
        app="zulip"
        title="Recent messages"
        query={
          streams[0].state === 'ready' && !channel
            ? [{ state: 'ready', data: [] }, streams[1]]
            : messages
        }
        actions={
          list.length > 0 && (
            <Select
              aria-label="Channel"
              value={channel ?? ''}
              onChange={(e) => setPicked(e.target.value)}
            >
              {list.map((s) => (
                <option key={s.id ?? s.name}>{s.name}</option>
              ))}
            </Select>
          )
        }
      >
        {(rows) => (
          <DataTable
            caption="Recent messages"
            rows={rows}
            rowKey={(r, i) => String(r.id ?? i)}
            empty="No messages"
            columns={[
              { key: 'sender', header: 'From', cell: (r) => r.sender },
              { key: 'topic', header: 'Topic', cell: (r) => r.topic },
              { key: 'content', header: 'Message', cell: (r) => r.content },
              {
                key: 'time',
                header: 'Sent',
                cell: (r) => (r.timestamp ? new Date(r.timestamp * 1000).toLocaleString() : ''),
              },
            ]}
          />
        )}
      </QueryCard>
    </div>
  );
}
