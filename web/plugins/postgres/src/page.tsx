import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { ConnectPanel, PageHead, SqlConsole, StackCard, Tabs, useStack } from '@loams/desktop-ui';
import { Badge, Notice } from '@loams/ui';
import { Branches } from './branches.js';

/** `/postgres`: the local Neon stack. */
export function PostgresPage({ desktop }: { desktop: LoamsDesktopApi }) {
  const state = useStack(desktop.stacks, 'postgres');
  const running = state?.phase === 'running';
  return (
    <div className="lc-page flex flex-col gap-4">
      <PageHead
        title="Postgres"
        badge={<Badge>Local stack</Badge>}
        subtitle={
          <>
            A Loams control plane will manage this on remote servers.
            <span className="block text-xs text-faint">
              Branches via the local Neon stack; the Loams control plane takes over on servers that
              offer loams.postgres.v1.
            </span>
          </>
        }
      />
      <StackCard desktop={desktop} id="postgres" />
      {running ? (
        <Tabs
          tabs={[
            { id: 'branches', label: 'Branches', content: <Branches pg={desktop.pg} /> },
            {
              id: 'connect',
              label: 'Connect',
              content: (
                <ConnectPanel
                  dialect="postgres"
                  connection={desktop.pg.connection}
                  revealPassword={desktop.pg.revealPassword}
                  copy={desktop.shell.clipboardWrite}
                />
              ),
            },
            { id: 'sql', label: 'SQL', content: <SqlConsole run={desktop.pg.query} /> },
          ]}
        />
      ) : (
        state &&
        state.phase !== 'unavailable' && (
          <Notice title="Start the stack to browse branches and run SQL">
            Branches, connection details and the SQL console appear once the local stack is running.
          </Notice>
        )
      )}
    </div>
  );
}
