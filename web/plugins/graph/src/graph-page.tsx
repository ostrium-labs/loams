import type { FlagsService } from '@loams/console-host';
import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { Badge, Button, Empty, StatusTag } from '@loams/ui';

export const GRAPH_API = 'loams.graph.v1';
export const DOCS_URL = 'https://loams.dev/docs';

/** `/graph`: the Graph page. No editor yet (GR1a); it says what is coming and lights up when served. */
export function GraphPage({
  desktop,
  flags,
}: {
  desktop: LoamsDesktopApi;
  flags: Pick<FlagsService, 'has'>;
}) {
  const served = flags.has(GRAPH_API);
  const docs = (
    <Button variant="secondary" onClick={() => void desktop.shell.openExternal(DOCS_URL)}>
      Read the docs
    </Button>
  );
  return (
    <div className="lc-page">
      <header className="lc-page-head flex flex-wrap items-start justify-between gap-4">
        <div>
          <h1>Graph</h1>
          <p>
            <Badge>{GRAPH_API}</Badge>{' '}
            <StatusTag status={served ? 'done' : 'planned'}>
              {served ? 'Served' : 'Not served'}
            </StatusTag>
          </p>
        </div>
      </header>
      {served ? (
        <Empty title="Available — editor coming in GR1a" actions={docs} seed={21}>
          This engine serves {GRAPH_API}. The GQL editor and graph browser arrive with GR1a.
        </Empty>
      ) : (
        <Empty title="Loams Graph is coming" actions={docs} seed={21}>
          GQL over {GRAPH_API}, served by the loams engine. Once the engine you are connected to
          serves it, this page lights up.
        </Empty>
      )}
    </div>
  );
}
