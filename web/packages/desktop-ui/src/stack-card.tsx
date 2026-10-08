import type { IpcResult, LoamsDesktopApi, StackId } from '@loams/desktop/contracts';
import { Button, Card, Notice, StatusTag } from '@loams/ui';
import { useState } from 'react';
import { useStack } from './use-stack.js';

const LABEL: Record<StackId, string> = {
  postgres: 'Postgres (Neon)',
  wesql: 'WeSQL',
  tikv: 'TiKV',
};

/** Where to get a container runtime; opened through `shell.openExternal`. */
export const RUNTIME_LINKS = [
  { label: 'Install Docker', url: 'https://docs.docker.com/engine/install/' },
  { label: 'Install Podman', url: 'https://podman.io/docs/installation' },
] as const;

/**
 * The local stack's card: phase, Start / Stop and Open logs. With no
 * container runtime it shows install guidance instead. `openLogs` is optional
 * because the desktop API has no per-stack log opener yet; without it the
 * button is shown disabled.
 */
export function StackCard({
  desktop,
  id,
  openLogs,
}: {
  desktop: Pick<LoamsDesktopApi, 'stacks' | 'shell'>;
  id: StackId;
  openLogs?: () => unknown;
}) {
  const state = useStack(desktop.stacks, id);
  // `stacks.openLogs` arrives with the main-process stacks work; feature-detect it.
  const apiOpenLogs = (desktop.stacks as { openLogs?: (id: StackId) => unknown }).openLogs;
  const logs = openLogs ?? (apiOpenLogs ? () => apiOpenLogs.call(desktop.stacks, id) : undefined);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string>();

  async function act(fn: () => Promise<IpcResult<void>>) {
    setBusy(true);
    setFailure(undefined);
    try {
      const r = await fn();
      if (!r.ok) setFailure(r.message);
    } catch (e) {
      setFailure(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  const title = `Local stack: ${LABEL[id]}`;
  if (!state) {
    return (
      <Card title={title}>
        <p className="text-sm text-muted">Checking the stack…</p>
      </Card>
    );
  }

  if (state.phase === 'unavailable') {
    return (
      <Card title={title} actions={<StatusTag status="planned">no container runtime</StatusTag>}>
        <div className="flex flex-col gap-3">
          <p className="m-0 text-sm text-muted">
            The local {LABEL[id]} stack runs in containers, and no container runtime was found on
            this computer. Install Docker or Podman, restart Loams Desktop, and the stack can be
            started from here.
          </p>
          <div className="flex flex-wrap gap-2">
            {RUNTIME_LINKS.map((l) => (
              <Button key={l.url} size="sm" onClick={() => void desktop.shell.openExternal(l.url)}>
                {l.label}
              </Button>
            ))}
          </div>
        </div>
      </Card>
    );
  }

  const tone =
    state.phase === 'running'
      ? 'done'
      : state.phase === 'starting'
        ? 'progress'
        : state.phase === 'error'
          ? 'failed'
          : 'planned';
  const text =
    state.phase === 'running'
      ? 'Running'
      : state.phase === 'starting'
        ? 'Starting'
        : state.phase === 'error'
          ? 'Error'
          : 'Stopped';

  return (
    <Card
      title={title}
      actions={
        <div className="flex items-center gap-2">
          <StatusTag status={tone}>{text}</StatusTag>
          {state.phase === 'running' ? (
            <Button size="sm" disabled={busy} onClick={() => act(() => desktop.stacks.stop(id))}>
              Stop
            </Button>
          ) : (
            <Button
              size="sm"
              variant="primary"
              disabled={busy || state.phase === 'starting'}
              onClick={() => act(() => desktop.stacks.start(id))}
            >
              {state.phase === 'error' ? 'Retry' : 'Start'}
            </Button>
          )}
          <Button
            size="sm"
            variant="quiet"
            disabled={!logs}
            title={logs ? undefined : 'Stack logs are not exposed by the desktop API yet'}
            onClick={() => void logs?.()}
          >
            Open logs
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-3">
        {state.phase === 'stopped' && (
          <p className="m-0 text-sm text-muted">The stack is stopped. Start it to use this page.</p>
        )}
        {state.phase === 'starting' && (
          <p className="m-0 text-sm text-muted">Starting containers. This can take a minute.</p>
        )}
        {state.phase === 'error' && (
          <Notice tone="danger" title="The stack failed">
            {state.message}
          </Notice>
        )}
        {state.phase === 'running' && (
          <ul className="m-0 flex list-none flex-col gap-1 p-0">
            {state.services.map((s) => (
              <li key={s.name} className="flex flex-wrap items-center gap-2 text-sm">
                <span className="font-mono">{s.name}</span>
                <span className="text-muted">{s.state}</span>
                {s.ports.length > 0 && (
                  <span className="font-mono text-xs text-faint">{s.ports.join(', ')}</span>
                )}
              </li>
            ))}
          </ul>
        )}
        {failure && (
          <Notice tone="danger" title="Could not change the stack">
            {failure}
          </Notice>
        )}
      </div>
    </Card>
  );
}
