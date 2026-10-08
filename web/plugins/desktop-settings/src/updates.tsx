import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Notice, StatusTag } from '@loams/ui';
import { useCallback, useEffect, useState } from 'react';

type UpdateState = Awaited<ReturnType<LoamsDesktopApi['update']['state']>>;

const PHASE_LABEL: Record<UpdateState['phase'], string> = {
  disabled: 'Off',
  idle: 'Up to date',
  checking: 'Checking',
  available: 'Update available',
  downloading: 'Downloading',
  ready: 'Ready to install',
  error: 'Error',
};
const PHASE_TONE: Record<UpdateState['phase'], 'done' | 'progress' | 'planned' | 'failed'> = {
  disabled: 'planned',
  idle: 'done',
  checking: 'progress',
  available: 'progress',
  downloading: 'progress',
  ready: 'done',
  error: 'failed',
};

/** Settings > Updates: the update state, check now, download (or the release page) and install. */
export function UpdatesSection({ desktop }: { desktop: LoamsDesktopApi }) {
  const [state, setState] = useState<UpdateState>();
  const [error, setError] = useState<string>();
  const refresh = useCallback(async () => {
    try {
      setState(await desktop.update.state());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [desktop]);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  // The state moves while a check or a download runs; poll only then.
  const moving = state?.phase === 'checking' || state?.phase === 'downloading';
  useEffect(() => {
    if (!moving) return;
    const t = setInterval(() => void refresh(), 1000);
    return () => clearInterval(t);
  }, [moving, refresh]);

  const act = async (op: () => Promise<void>) => {
    setError(undefined);
    try {
      await op();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    await refresh();
  };

  const phase = state?.phase;
  const manual = state?.mode === 'manual';
  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-lg font-medium m-0">Updates</h2>
      {error && (
        <Notice tone="danger" title="That did not work">
          {error}
        </Notice>
      )}
      <Card
        title="Loams Desktop"
        actions={phase && <StatusTag status={PHASE_TONE[phase]}>{PHASE_LABEL[phase]}</StatusTag>}
      >
        {!state && !error && <p className="text-muted">Reading the update state…</p>}
        {phase === 'disabled' && (
          <p className="text-muted">
            Updates are off: no update feed is configured for this build, so nothing is checked and
            nothing leaves this computer.
          </p>
        )}
        {phase === 'idle' && <p className="text-muted">You are on the latest version.</p>}
        {phase === 'checking' && <p className="text-muted">Looking for an update…</p>}
        {phase === 'available' && (
          <p>
            Version <code>{state?.version}</code> is available.
            {manual && ' On macOS and package installs, updates come from the release page.'}
          </p>
        )}
        {phase === 'downloading' && (
          <p>
            Downloading <code>{state?.version}</code>
            {state?.percent !== undefined ? `: ${Math.round(state.percent)}%` : '…'}
          </p>
        )}
        {phase === 'ready' && (
          <p>
            Version <code>{state?.version}</code> is downloaded. Restart to install it.
          </p>
        )}
        {phase === 'error' && (
          <Notice tone="danger" title="The update check failed">
            {state?.message ?? 'Unknown error.'}
          </Notice>
        )}
        <div className="flex gap-2 mt-4">
          <Button
            disabled={
              !phase || phase === 'disabled' || phase === 'checking' || phase === 'downloading'
            }
            onClick={() => void act(() => desktop.update.check())}
          >
            Check now
          </Button>
          {phase === 'available' && (
            <Button variant="primary" onClick={() => void act(() => desktop.update.download())}>
              {manual ? `Download v${state?.version}` : 'Download update'}
            </Button>
          )}
          {phase === 'ready' && (
            <Button
              variant="primary"
              onClick={() => void act(() => desktop.update.installAndRestart())}
            >
              Restart and install
            </Button>
          )}
        </div>
      </Card>
    </div>
  );
}
