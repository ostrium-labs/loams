import type { LoamsDesktopApi } from '@loams/desktop/contracts';
import { ConnectPanel, PageHead, SqlConsole, StackCard, Tabs, useStack } from '@loams/desktop-ui';
import { Badge, Notice } from '@loams/ui';
import { Schemas } from './schemas.js';

/** `/wesql`: the local WeSQL (MySQL) dev stack. */
export function WesqlPage({ desktop }: { desktop: LoamsDesktopApi }) {
  const state = useStack(desktop.stacks, 'wesql');
  const running = state?.phase === 'running';
  return (
    <div className="lc-page flex flex-col gap-4">
      <PageHead
        title="WeSQL (local dev stack)"
        badge={<Badge>Local stack</Badge>}
        subtitle={
          <>
            A Loams control plane will manage this on remote servers.
            <span className="block text-xs text-faint">
              Loams SQL (MySQL 8.4) arrives with the SQ1 control plane.
            </span>
          </>
        }
      />
      <StackCard desktop={desktop} id="wesql" />
      {running ? (
        <Tabs
          tabs={[
            { id: 'schemas', label: 'Schemas', content: <Schemas wesql={desktop.wesql} /> },
            {
              id: 'connect',
              label: 'Connect',
              content: (
                <ConnectPanel
                  dialect="mysql"
                  connection={desktop.wesql.connection}
                  revealPassword={desktop.wesql.revealPassword}
                  copy={desktop.shell.clipboardWrite}
                />
              ),
            },
            { id: 'sql', label: 'SQL', content: <SqlConsole run={desktop.wesql.query} /> },
          ]}
        />
      ) : (
        state &&
        state.phase !== 'unavailable' && (
          <Notice title="Start the stack to browse schemas and run SQL">
            Schemas, connection details and the SQL console appear once the local stack is running.
          </Notice>
        )
      )}
    </div>
  );
}
