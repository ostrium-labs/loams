// The sidebar's Factory group (§53 §5.9, D916; SD1 Task 27): Forgejo, Zulip, Plane,
// GlitchTip, OpenPanel, Matomo (Q813), Langfuse and SigNoz, each with its
// configured state. On the desktop each opens on the factory's pages under
// `/studio/factory`; a links-only source (the web, until SF1: D922) links to the
// apps' own UIs in a new tab, and lists an app it cannot link without a link.
//
// Owner feedback 2026-10-17 (T10-6): the rows match the thread rows (28px, `text-small`),
// and the group is collapsed by default, a one-line header with how many apps are
// connected, so the eight apps never push the Pages out of view. The choice is per device.

import { cx } from '@loams/ui';
import { useQuery } from '@tanstack/react-query';
import { ChevronRight, ExternalLink, Factory } from 'lucide-react';
import { useEffect, useId, useState } from 'react';
import {
  FACTORY_HOME,
  type FactoryApp,
  type FactoryState,
  factoryHref,
  httpUrl,
  STATE_LABEL,
  type StudioFactory,
} from '../factory.js';

/** The thread row's box and type (Sidebar.tsx `ROW`), without the selected chip. */
const ROW =
  'flex h-7 min-w-0 items-center gap-2 rounded-md px-2 text-small text-muted no-underline ' +
  'outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent';

/** Whether the group is expanded, saved per device; collapsed when nothing is saved. */
export const FACTORY_OPEN_KEY = 'loams.studio.factory.open';

function readOpen(): boolean {
  try {
    return localStorage.getItem(FACTORY_OPEN_KEY) === '1';
  } catch {
    return false;
  }
}

function writeOpen(open: boolean) {
  try {
    localStorage.setItem(FACTORY_OPEN_KEY, open ? '1' : '0');
  } catch {
    // Storage blocked: the group keeps its state for this page.
  }
}

const DOT: Record<FactoryState, string> = {
  ok: 'bg-grow',
  auth_failed: 'bg-danger',
  unreachable: 'bg-danger',
  locked: 'bg-danger',
  unconfigured: 'border border-faint bg-transparent',
};

/** The states drawn in the danger colour: the collapsed header marks them too. */
const FAILING = new Set<FactoryState>(['auth_failed', 'unreachable', 'locked']);

function State({ state }: { state: FactoryState }) {
  return (
    <span className="ml-auto flex shrink-0 items-center gap-1.5 text-micro text-faint">
      <span aria-hidden="true" className={cx('inline-block size-1.5 rounded-pill', DOT[state])} />
      {STATE_LABEL[state]}
    </span>
  );
}

function Row({ app, kind }: { app: FactoryApp; kind: StudioFactory['kind'] }) {
  const label = (
    <span className="min-w-0 flex-1 truncate" title={app.preview}>
      {app.label}
      {/* A preview app says so (SigNoz on the House, D1666); the tooltip has the full label. */}
      {app.preview && (
        <>
          {' '}
          <span className="text-micro text-faint">Preview</span>
        </>
      )}
    </span>
  );
  if (kind === 'pages') {
    return (
      <a href={`#${factoryHref(app)}`} className={cx(ROW, 'hover:bg-row-hover hover:text-ink')}>
        {label}
        <State state={app.state} />
      </a>
    );
  }
  const url = app.state === 'ok' ? httpUrl(app.url) : undefined;
  if (!url) {
    return (
      <span className={ROW}>
        {label}
        <State state={app.state} />
      </span>
    );
  }
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className={cx(ROW, 'hover:bg-row-hover hover:text-ink')}
    >
      {label}
      <ExternalLink size={12} aria-hidden="true" className="shrink-0 text-faint" />
      <State state={app.state} />
    </a>
  );
}

export function FactoryGroup({ factory }: { factory: StudioFactory }) {
  const apps = useQuery({
    queryKey: ['studio', 'factory', factory.kind, 'apps'],
    queryFn: () => factory.list(),
  });
  // The group stays mounted while the factory's pages configure or remove an app, and
  // the bridge has no change event: re-read the (local, cheap) list on every navigation.
  const { refetch } = apps;
  useEffect(() => {
    const onNav = () => void refetch();
    window.addEventListener('hashchange', onNav);
    return () => window.removeEventListener('hashchange', onNav);
  }, [refetch]);
  const [open, setOpen] = useState(readOpen);
  const listId = useId();
  const toggle = () => {
    writeOpen(!open);
    setOpen(!open);
  };
  const connected = apps.data?.filter((a) => a.state === 'ok').length ?? 0;
  const failing = apps.data?.some((a) => FAILING.has(a.state)) ?? false;
  return (
    <nav aria-label="Factory" className="shrink-0 pt-1">
      <div className="flex h-7 items-center gap-1 pr-1">
        <button
          type="button"
          onClick={toggle}
          aria-expanded={open}
          aria-controls={listId}
          className={cx(
            'flex h-7 min-w-0 flex-1 cursor-pointer items-center gap-1.5 rounded-md border-0 bg-transparent px-2',
            'text-left font-[inherit] text-micro font-medium text-faint hover:bg-row-hover hover:text-ink',
            'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
          )}
        >
          <ChevronRight
            size={12}
            strokeWidth={2}
            aria-hidden="true"
            className={cx('shrink-0', open && 'rotate-90')}
          />
          <span>Factory</span>
          {apps.isSuccess && apps.data.length > 0 && !open && (
            <span className="ml-auto flex items-center gap-1.5 font-normal">
              {failing && (
                <>
                  <span
                    aria-hidden="true"
                    className="inline-block size-1.5 rounded-pill bg-danger"
                  />
                  <span className="sr-only">Some apps need attention.</span>
                </>
              )}
              {connected} of {apps.data.length} connected
            </span>
          )}
        </button>
        {factory.kind === 'pages' && (
          <a
            href={`#${FACTORY_HOME}`}
            aria-label="All factory apps"
            title="All factory apps"
            className="inline-grid size-6 shrink-0 place-items-center rounded-sm text-faint hover:bg-row-hover hover:text-ink"
          >
            <Factory size={13} strokeWidth={1.75} aria-hidden="true" />
          </a>
        )}
      </div>
      {apps.isError && (
        <p role="alert" className="m-0 px-2 py-1 text-small text-danger">
          Could not list the factory apps: {apps.error.message}
        </p>
      )}
      <div id={listId} hidden={!open}>
        {open && apps.isPending && <p className="m-0 px-2 py-1 text-small text-faint">Loading…</p>}
        {open && apps.isSuccess && apps.data.length === 0 && (
          <p className="m-0 px-2 py-1 text-small text-faint">No factory apps.</p>
        )}
        {open && apps.isSuccess && apps.data.length > 0 && (
          <ul className="m-0 flex list-none flex-col gap-px p-0">
            {apps.data.map((app) => (
              <li key={app.id}>
                <Row app={app} kind={factory.kind} />
              </li>
            ))}
          </ul>
        )}
      </div>
    </nav>
  );
}
