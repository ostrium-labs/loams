// Small pieces every Data Studio page uses: an async loader, error text,
// the page header and the collection tabs.

import { ConnectError } from '@connectrpc/connect';
import { type Loaded, useLoad as useSharedLoad } from '@loams/desktop-ui';
import { Notice } from '@loams/ui';

export function errorText(e: unknown): string {
  if (e instanceof ConnectError) return e.rawMessage || e.message;
  return e instanceof Error ? e.message : String(e);
}

export type { Loaded } from '@loams/desktop-ui';
export { PageHead } from '@loams/desktop-ui';

/** Runs `load` on mount and whenever `deps` change; `reload` runs it again. */
export function useLoad<T>(load: () => Promise<T>, deps: unknown[]): [Loaded<T>, () => void] {
  return useSharedLoad(load, deps, { errorText });
}

export function ErrorNotice({ title, message }: { title: string; message: string }) {
  return (
    <Notice tone="danger" title={title}>
      {message}
    </Notice>
  );
}

export type Tab = 'documents' | 'search' | 'sql' | 'ingest' | 'schema';

const TABS: { id: Tab; label: string }[] = [
  { id: 'documents', label: 'Documents' },
  { id: 'search', label: 'Search' },
  { id: 'sql', label: 'SQL' },
  { id: 'ingest', label: 'Ingest' },
  { id: 'schema', label: 'Schema' },
];

export function tabPath(ns: string, coll: string | undefined, tab: Tab): string {
  const base = `/data/${encodeURIComponent(ns)}`;
  if (tab === 'sql') return `${base}/sql`;
  const c = `${base}/${encodeURIComponent(coll ?? '')}`;
  return tab === 'documents' ? c : `${c}/${tab}`;
}

export function Tabs({
  ns,
  coll,
  active,
  navigate,
}: {
  ns: string;
  coll?: string;
  active: Tab;
  navigate: (to: string) => void;
}) {
  return (
    <div className="ds-tabs" role="tablist" aria-label="Data views">
      {TABS.filter((t) => coll || t.id === 'sql').map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          aria-selected={t.id === active}
          className="ds-tab"
          onClick={() => navigate(tabPath(ns, coll, t.id))}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}
