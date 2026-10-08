// Small pieces every Streams page uses: an async loader, error text, the
// page header and the namespace picker (free text plus a recent list, as in
// Data Studio).

import { Button, Input, Notice } from '@loams/ui';
import { type FormEvent, type ReactNode, useCallback, useEffect, useRef, useState } from 'react';

export function errorText(e: unknown): string {
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
    <header className="flex flex-wrap items-end justify-between gap-4">
      <div>
        {crumbs && <nav className="mb-1 text-sm text-muted">{crumbs}</nav>}
        <h1 className="m-0">{title}</h1>
        {subtitle && <p className="lc-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex gap-2">{actions}</div>}
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

const RECENT_KEY = 'loams.streams.namespaces';

function recentNamespaces(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
    return Array.isArray(raw) ? raw.filter((s): s is string => typeof s === 'string') : [];
  } catch {
    return [];
  }
}

export function rememberNamespace(ns: string): void {
  try {
    const next = [ns, ...recentNamespaces().filter((n) => n !== ns)].slice(0, 12);
    localStorage.setItem(RECENT_KEY, JSON.stringify(next));
  } catch {
    // storage is a convenience only
  }
}

export function NamespacePicker({ ns, onOpen }: { ns: string; onOpen: (ns: string) => void }) {
  const [draft, setDraft] = useState(ns);
  const go = (e: FormEvent) => {
    e.preventDefault();
    const next = draft.trim();
    if (next) onOpen(next);
  };
  const options = [...new Set(['default', ns, ...recentNamespaces()])];
  return (
    <form className="flex items-center gap-2" onSubmit={go}>
      <label htmlFor="sp-ns-input" className="text-sm text-muted">
        Namespace
      </label>
      <Input
        id="sp-ns-input"
        list="sp-ns-options"
        className="w-48 font-mono"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        spellCheck={false}
      />
      <datalist id="sp-ns-options">
        {options.map((o) => (
          <option key={o} value={o} />
        ))}
      </datalist>
      <Button type="submit" size="sm">
        Open
      </Button>
    </form>
  );
}
