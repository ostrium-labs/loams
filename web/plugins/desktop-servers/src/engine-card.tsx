import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Notice, StatusTag } from '@loams/ui';
import { Copy } from 'lucide-react';
import { PHASE_LABEL, PHASE_TONE, useEngine } from './state.js';

/** The managed local engine: phase, its URLs, start/stop and logs. */
export function EngineCard({ desktop }: { desktop: LoamsDesktopApi }) {
  const engine = useEngine(desktop);
  if (!engine) {
    return (
      <Card title="Engine">
        <p className="lc-muted">Reading engine state…</p>
      </Card>
    );
  }
  const urls =
    engine.phase === 'ready'
      ? [
          ['HTTP', engine.url],
          ['Elasticsearch', engine.esUrl],
          ['Flight', engine.flightUrl],
        ]
      : [];
  const running = engine.phase === 'ready' || engine.phase === 'starting';
  return (
    <Card
      title="Engine"
      actions={<StatusTag status={PHASE_TONE[engine.phase]}>{PHASE_LABEL[engine.phase]}</StatusTag>}
    >
      {engine.phase === 'starting' && (
        <p className="lc-muted">Starting the local engine (attempt {engine.attempt})…</p>
      )}
      {engine.phase === 'stopped' && <p className="lc-muted">The local engine is not running.</p>}
      {engine.phase === 'failed' && (
        <Notice tone="danger" title="The local engine failed">
          {engine.reason}
        </Notice>
      )}
      {urls.length > 0 && (
        <dl className="lc-engine-urls">
          {urls.map(([label, url]) => (
            <div key={label}>
              <dt>{label}</dt>
              <dd>
                <code>{url}</code>
                <Button
                  size="icon"
                  variant="quiet"
                  aria-label={`Copy ${label} URL`}
                  title={`Copy ${label} URL`}
                  onClick={() => void desktop.shell.clipboardWrite(url ?? '')}
                >
                  <Copy aria-hidden="true" size={14} />
                </Button>
              </dd>
            </div>
          ))}
        </dl>
      )}
      <div className="lc-actions">
        {running ? (
          <Button onClick={() => void desktop.engine.stop()}>Stop</Button>
        ) : (
          <Button variant="primary" onClick={() => void desktop.engine.start()}>
            Start
          </Button>
        )}
        <Button variant="quiet" onClick={() => void desktop.engine.openLogs()}>
          Open logs
        </Button>
      </div>
    </Card>
  );
}
