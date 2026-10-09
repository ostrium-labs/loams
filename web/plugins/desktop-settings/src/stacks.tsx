import type { IpcResult, LoamsDesktopApi, StackId, StackState } from '@loams/desktop/contracts';
import { Button, Card, Notice, StatusTag, Table } from '@loams/ui';
import { useCallback, useEffect, useState } from 'react';
import { LiveStoreCard } from './live-store.js';

export const STACKS: { id: StackId; label: string; note: string }[] = [
  { id: 'postgres', label: 'Postgres', note: 'Branchable Postgres' },
  { id: 'wesql', label: 'WeSQL', note: 'MySQL-compatible, S3-backed' },
  { id: 'tikv', label: 'TiKV', note: 'Distributed key-value store' },
];

export const STACK_LABEL: Record<StackState['phase'], string> = {
  unavailable: 'Unavailable',
  stopped: 'Stopped',
  starting: 'Starting',
  running: 'Running',
  error: 'Error',
};
export const STACK_TONE: Record<StackState['phase'], 'done' | 'progress' | 'planned' | 'failed'> = {
  unavailable: 'planned',
  stopped: 'planned',
  starting: 'progress',
  running: 'done',
  error: 'failed',
};

/** The three stacks' states, live. A stack whose state cannot be read stays `undefined`. */
export function useStacks(desktop: LoamsDesktopApi) {
  const [states, setStates] = useState<Partial<Record<StackId, StackState>>>({});
  const [error, setError] = useState<string>();
  useEffect(() => {
    let live = true;
    const off = desktop.stacks.onState((id, s) => live && setStates((c) => ({ ...c, [id]: s })));
    for (const { id } of STACKS) {
      desktop.stacks
        .state(id)
        .then((s) => live && setStates((c) => ({ [id]: s, ...c })))
        .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    }
    return () => {
      live = false;
      off();
    };
  }, [desktop]);
  return { states, error };
}

/** Settings > Local stacks: the container runtime, and each stack's state with start and stop. */
export function StacksSection({ desktop }: { desktop: LoamsDesktopApi }) {
  const { states, error } = useStacks(desktop);
  const [actionError, setActionError] = useState<string>();
  const [busy, setBusy] = useState<StackId>();
  const read = Object.values(states);
  // Say nothing about the runtime until every stack's state is read.
  const allRead = read.length === STACKS.length;
  const noRuntime =
    allRead && read.every((s) => s.phase === 'unavailable' && s.reason === 'no_container_runtime');

  const run = useCallback(async (id: StackId, op: () => Promise<IpcResult<void>>) => {
    setBusy(id);
    setActionError(undefined);
    try {
      const res = await op();
      if (!res.ok) setActionError(`${id}: ${res.message}`);
    } catch (e) {
      setActionError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(undefined);
    }
  }, []);

  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-lg font-medium m-0">Local stacks</h2>
      {error && (
        <Notice tone="danger" title="Could not read the stacks">
          {error}
        </Notice>
      )}
      {actionError && (
        <Notice tone="danger" title="That did not work">
          {actionError}
        </Notice>
      )}
      {noRuntime ? (
        <Notice tone="warn" title="No container runtime found">
          Postgres, WeSQL and TiKV run in containers. Install Docker or Podman, then reopen this
          page.
        </Notice>
      ) : (
        allRead && <p className="text-sm text-muted m-0">Container runtime: found.</p>
      )}
      <Card title="Stacks" flush>
        <Table
          caption="Local stacks"
          rows={STACKS}
          rowKey={(s) => s.id}
          columns={[
            {
              key: 'stack',
              header: 'Stack',
              cell: (s) => (
                <>
                  <strong>{s.label}</strong>
                  <div className="text-xs text-muted">{s.note}</div>
                </>
              ),
            },
            {
              key: 'state',
              header: 'State',
              cell: (s) => {
                const st = states[s.id];
                if (!st) return <span className="text-muted">Reading…</span>;
                return (
                  <>
                    <StatusTag status={STACK_TONE[st.phase]}>{STACK_LABEL[st.phase]}</StatusTag>
                    {st.phase === 'running' && st.services.length > 0 && (
                      <div className="text-xs text-muted font-mono">
                        {st.services
                          .map((x) => `${x.name} ${x.ports.join(',')}`.trim())
                          .join(' · ')}
                      </div>
                    )}
                    {st.phase === 'error' && (
                      <div className="text-xs text-danger">{st.message}</div>
                    )}
                  </>
                );
              },
            },
            {
              key: 'actions',
              header: '',
              cell: (s) => {
                const st = states[s.id];
                if (!st || st.phase === 'unavailable') return null;
                const on = st.phase === 'running' || st.phase === 'starting';
                return on ? (
                  <Button
                    size="sm"
                    aria-label={`Stop ${s.label}`}
                    disabled={busy === s.id}
                    onClick={() => void run(s.id, () => desktop.stacks.stop(s.id))}
                  >
                    Stop
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    variant="primary"
                    aria-label={`Start ${s.label}`}
                    disabled={busy === s.id}
                    onClick={() => void run(s.id, () => desktop.stacks.start(s.id))}
                  >
                    Start
                  </Button>
                );
              },
            },
          ]}
        />
      </Card>
      <LiveStoreCard desktop={desktop} />
    </div>
  );
}
