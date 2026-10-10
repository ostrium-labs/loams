import { Button, Input } from '@loams/ui';
import { type FormEvent, useState } from 'react';

/** Recently opened namespaces under `storageKey` (newest first); empty when storage is unavailable. */
export function recentNamespaces(storageKey: string): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(storageKey) ?? '[]');
    return Array.isArray(raw) ? raw.filter((s): s is string => typeof s === 'string') : [];
  } catch {
    return [];
  }
}

export function rememberNamespace(storageKey: string, ns: string): void {
  try {
    const next = [ns, ...recentNamespaces(storageKey).filter((n) => n !== ns)].slice(0, 12);
    localStorage.setItem(storageKey, JSON.stringify(next));
  } catch {
    // storage is a convenience only
  }
}

/** Free-text namespace field with a recent list, as in Data Studio and Streams. */
export function NamespacePicker({
  ns,
  onOpen,
  storageKey,
  idPrefix,
}: {
  ns: string;
  onOpen: (ns: string) => void;
  /** Where the page keeps its recent namespaces. */
  storageKey: string;
  /** Unique per page, for the input and datalist ids. */
  idPrefix: string;
}) {
  const [draft, setDraft] = useState(ns);
  const go = (e: FormEvent) => {
    e.preventDefault();
    const next = draft.trim();
    if (next) onOpen(next);
  };
  const options = [...new Set(['default', ns, ...recentNamespaces(storageKey)])];
  return (
    <form className="flex items-center gap-2 pb-2" onSubmit={go}>
      <label htmlFor={`${idPrefix}-ns-input`} className="text-sm text-muted">
        Namespace
      </label>
      <Input
        id={`${idPrefix}-ns-input`}
        list={`${idPrefix}-ns-options`}
        className="w-48 font-mono"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        spellCheck={false}
      />
      <datalist id={`${idPrefix}-ns-options`}>
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
