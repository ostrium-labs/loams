import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Badge, Button, Card } from '@loams/ui';
import notice from '../../../../NOTICE?raw';

/** Settings > About: the version (from the main process), licences (NOTICE) and the logs folder. */
export function AboutSection({
  desktop,
  licences = notice,
}: {
  desktop: LoamsDesktopApi;
  licences?: string;
}) {
  return (
    <div className="flex flex-col gap-4">
      <h2 className="text-lg font-medium m-0">About</h2>
      <Card title="Loams Desktop">
        <dl className="grid grid-cols-[8rem_1fr] gap-y-2 text-sm">
          <dt className="text-muted">Version</dt>
          <dd>
            <Badge>{desktop.version}</Badge>
          </dd>
          <dt className="text-muted">Platform</dt>
          <dd>
            <code>{desktop.platform}</code>
          </dd>
        </dl>
        <div className="mt-4">
          <Button onClick={() => void desktop.engine.openLogs()}>Open logs folder</Button>
        </div>
      </Card>
      <Card title="Licences" flush>
        {/* biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be keyboard reachable */}
        <section aria-label="Licences" tabIndex={0} className="max-h-96 overflow-auto">
          <pre className="font-mono text-xs p-4 m-0 whitespace-pre-wrap">{licences}</pre>
        </section>
      </Card>
    </div>
  );
}
