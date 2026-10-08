import type { LoamsDesktopApi, PgTimeline, PgWalStatus } from '@loams/desktop/contracts';
import { Button, Dialog, Empty, Field, Input, Notice, Select } from '@loams/ui';
import { useCallback, useEffect, useState } from 'react';
import { flattenTimelines } from './tree.js';

type Pg = LoamsDesktopApi['pg'];
type Load =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | {
      state: 'ready';
      tenants: string[];
      timelines: PgTimeline[];
      wal: Record<string, PgWalStatus | undefined>;
    };

/** The Branches tab: timelines as a tree by ancestor, with a create-branch action. */
export function Branches({ pg }: { pg: Pg }) {
  const [tenant, setTenant] = useState<string>();
  const [load, setLoad] = useState<Load>({ state: 'loading' });
  const [from, setFrom] = useState<PgTimeline>();
  const [name, setName] = useState('');
  const [lsn, setLsn] = useState('');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(
    async (wanted?: string) => {
      try {
        const ts = await pg.tenants();
        if (!ts.ok) return setLoad({ state: 'error', message: ts.message });
        const t = wanted && ts.value.includes(wanted) ? wanted : ts.value[0];
        if (!t) return setLoad({ state: 'ready', tenants: [], timelines: [], wal: {} });
        setTenant(t);
        const tl = await pg.timelines(t);
        if (!tl.ok) return setLoad({ state: 'error', message: tl.message });
        const wal: Record<string, PgWalStatus | undefined> = {};
        await Promise.all(
          tl.value.map(async (x) => {
            const w = await pg.walStatus(t, x.timelineId).catch(() => undefined);
            wal[x.timelineId] = w?.ok ? w.value : undefined;
          }),
        );
        setLoad({ state: 'ready', tenants: ts.value, timelines: tl.value, wal });
      } catch (e) {
        setLoad({ state: 'error', message: e instanceof Error ? e.message : String(e) });
      }
    },
    [pg],
  );
  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function create() {
    if (!from || !tenant || !name.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      const r = await pg.createBranch(tenant, {
        name: name.trim(),
        ancestorTimelineId: from.timelineId,
        ...(lsn.trim() ? { ancestorStartLsn: lsn.trim() } : {}),
      });
      if (!r.ok) return setError(r.message);
      setFrom(undefined);
      setName('');
      setLsn('');
      await refresh(tenant);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  if (load.state === 'loading') return <p className="text-sm text-muted">Loading branches…</p>;
  if (load.state === 'error') {
    return (
      <Notice tone="danger" title="Could not load branches">
        {load.message}
      </Notice>
    );
  }
  const rows = flattenTimelines(load.timelines);
  if (rows.length === 0) {
    return (
      <Empty title="No timelines yet">
        The stack has no tenant or timeline. Start it, or check its logs.
      </Empty>
    );
  }
  return (
    <div className="flex flex-col gap-3">
      {load.tenants.length > 1 && (
        <Field label="Tenant">
          {(p) => (
            <Select {...p} value={tenant} onChange={(e) => void refresh(e.target.value)}>
              {load.tenants.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
          )}
        </Field>
      )}
      <div className="loams-table-wrap">
        <table className="loams-table">
          <caption className="sr-only">Timelines</caption>
          <thead>
            <tr>
              <th>Branch</th>
              <th>Timeline id</th>
              <th>Ancestor LSN</th>
              <th>Last record LSN</th>
              <th>WAL head</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map(({ timeline: t, depth }) => (
              <tr key={t.timelineId} data-depth={depth} data-timeline={t.timelineId}>
                <td>
                  <span
                    style={{ paddingLeft: `${depth * 1.25}rem` }}
                    className="inline-flex gap-2 whitespace-nowrap"
                  >
                    {depth > 0 && <span className="text-faint">└</span>}
                    <strong>{t.name ?? 'unnamed'}</strong>
                  </span>
                </td>
                <td className="font-mono text-xs">{t.timelineId}</td>
                <td className="font-mono text-xs">{t.ancestorLsn ?? '—'}</td>
                <td className="font-mono text-xs">{t.lastRecordLsn}</td>
                <td className="font-mono text-xs">{load.wal[t.timelineId]?.commitLsn ?? '—'}</td>
                <td>
                  <Button
                    size="sm"
                    onClick={() => {
                      setFrom(t);
                      setName('');
                      setLsn('');
                      setError(undefined);
                    }}
                  >
                    Create branch from here
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <Dialog
        open={from !== undefined}
        onClose={() => setFrom(undefined)}
        title={`New branch from ${from?.name ?? from?.timelineId ?? ''}`}
        footer={
          <>
            <Button onClick={() => setFrom(undefined)}>Cancel</Button>
            <Button variant="primary" disabled={busy || !name.trim()} onClick={create}>
              {busy ? 'Creating…' : 'Create branch'}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-3">
          <Field label="Branch name">
            {(p) => <Input {...p} value={name} onChange={(e) => setName(e.target.value)} />}
          </Field>
          <Field label="Start LSN (optional)" hint="Leave empty to branch from the current head.">
            {(p) => (
              <Input
                {...p}
                className="font-mono"
                placeholder="0/16B3748"
                value={lsn}
                onChange={(e) => setLsn(e.target.value)}
              />
            )}
          </Field>
          {error && (
            <Notice tone="danger" title="Could not create the branch">
              {error}
            </Notice>
          )}
        </div>
      </Dialog>
    </div>
  );
}
