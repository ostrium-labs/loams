// Small pieces every Data Studio page uses: an async loader, error text,
// the page header and the collection tabs.

import { ConnectError } from '@connectrpc/connect';
import { Notice } from '@loams/ui';
import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';

export function errorText(e: unknown): string {
  if (e instanceof ConnectError) return e.rawMessage || e.message;
  return e instanceof Error ? e.message : String(e);
}

export type Loaded<T> =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; data: T };

/** Runs `load` on mount and whenever `deps` change; `reload` runs it again. */
export function useLoad<T>(load: () => Promise<T>, deps: unknown[]): [Loaded<T>, () => void] {
  const [value, setValue] = useState<Loaded<T>>({ state: 'loading' });
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  // biome-ignore lint/correctness/useExhaustiveDependencies: deps are the caller's
  useEffect(() => {
    let live = true;
    setValue({ state: 'loading' });
    loadRef
      .current()
      .then((data) => live && setValue({ state: 'ready', data }))
      .catch((e) => live && setValue({ state: 'error', message: errorText(e) }));
    return () => {
      live = false;
    };
  }, [...deps, tick]);
  return [value, useCallback(() => setTick((t) => t + 1), [])];
}

export function PageHead({
  title,
  subtitle,
  actions,
  crumbs,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  actions?: ReactNode;
  crumbs?: ReactNode;
}) {
  return (
    <header className="ds-head">
      <div>
        {crumbs && <nav className="ds-crumbs">{crumbs}</nav>}
        <h1>{title}</h1>
        {subtitle && <p className="lc-muted">{subtitle}</p>}
      </div>
      {actions && <div className="ds-head-actions">{actions}</div>}
    </header>
  );
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
