// The Seed Exchange's frame: its heading, the tabs, and the agentd it talks to.
//
// Which agentd: Studio's, when Studio is loaded (its `studio.agentd.scope` slot wraps
// the page in Studio's AgentdScope: on the web, the host picked in Studio's Settings ›
// Agentd hosts), else the console's `agentd` (the desktop's daemon).

import type { AgentdService } from '@loams/console-host';
import { AgentdScope } from '@loams/console-host/query';
import { Slot } from '@loams/slots';
import { Badge, cx, Empty } from '@loams/ui';
import type { ReactNode } from 'react';
import { useApi } from './data.js';
import { PAGE_VIEWS, type PageProps } from './pages.js';
import { href, type Page } from './paths.js';

const TABS: readonly (readonly [Page, string])[] = [
  ['browse', 'Browse'],
  ['installed', 'Installed'],
  ['sources', 'Sources'],
];

function Body({ page, ...props }: PageProps & { page: Page }) {
  const api = useApi();
  if (!api) {
    return (
      <Empty title="Not connected">
        The Seed Exchange installs into loams-agentd. On the desktop, start it from Settings ›
        Background agents; in a browser, add a host in Studio › Settings › Agentd hosts.
      </Empty>
    );
  }
  const View = PAGE_VIEWS[page];
  return <View {...props} api={api} />;
}

export interface MarketplaceViewProps extends PageProps {
  page: Page;
}

/** One page of the Seed Exchange, with its heading and tabs. */
export function MarketplaceView({ page, base, params, navigate }: MarketplaceViewProps) {
  const active: Page = page === 'listing' || page === 'install' ? 'browse' : page;
  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-4 p-6">
      <header className="flex flex-col gap-1">
        <h1 className="m-0 flex items-center gap-2 text-title font-semibold text-ink">
          The Seed Exchange
          <Badge variant="warning">Experimental</Badge>
        </h1>
        <p className="m-0 text-small text-muted">
          The Loams marketplace: agents, harnesses and MCP servers that agentd installs and runs. It
          lists first-party listings only for now. Every install shows what the listing will be
          allowed to do before anything is created.
        </p>
      </header>
      <nav aria-label="The Seed Exchange" className="flex gap-1 border-rule-soft border-b">
        {TABS.map(([p, label]) => (
          <a
            key={p}
            href={`#${href(base, p)}`}
            aria-current={p === active ? 'page' : undefined}
            onClick={(e) => {
              e.preventDefault();
              navigate(href(base, p));
            }}
            className={cx(
              '-mb-px border-b-2 px-3 py-1.5 text-small',
              p === active ? 'border-accent text-ink' : 'border-transparent text-muted',
            )}
          >
            {label}
          </a>
        ))}
      </nav>
      <Body page={page} base={base} params={params} navigate={navigate} />
    </div>
  );
}

/**
 * `children` under Studio's agentd when Studio provides one, else under `agentd`. The
 * slot re-renders when Studio loads, so a page opened first in Console follows.
 */
export function MarketplaceScope({
  agentd,
  children,
}: {
  agentd: AgentdService;
  children: ReactNode;
}) {
  return (
    <Slot
      name="studio.agentd.scope"
      props={{ children }}
      fallback={<AgentdScope agentd={agentd}>{children}</AgentdScope>}
    />
  );
}
