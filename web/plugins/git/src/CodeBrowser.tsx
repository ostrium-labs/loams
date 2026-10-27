// A code browser over a workspace at `@n` (or its latest snapshot). Like the client
// (§19.5.1, D830), it never fetches the whole tree: a directory's entries load when it is
// first expanded (GetTrees), and a file's content loads when it is opened (ReadFiles).
// Sizes come from tree aux data, so the tree shows them without fetching blobs.
//
// The tree follows the WAI-ARIA tree pattern with a roving tabindex: ArrowUp/ArrowDown
// move, ArrowRight expands or enters, ArrowLeft collapses or goes to the parent,
// Home/End jump, Enter opens a file or toggles a directory.

import { Badge, formatBytes, Notice } from '@loams/ui';
import { type KeyboardEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  type ChangeKind,
  type FileContent,
  type GitClient,
  type GitFailure,
  isAbort,
  MAX_FILE_BYTES,
  type TreeEntry,
  toFailure,
  type WsRef,
} from './client.js';

export const CHANGE_LABEL: Record<ChangeKind, string> = {
  modified: 'M',
  added: 'A',
  deleted: 'D',
  untracked: '?',
  type_changed: 'T',
};

export const CHANGE_TEXT: Record<ChangeKind, string> = {
  modified: 'modified',
  added: 'added',
  deleted: 'deleted',
  untracked: 'untracked',
  type_changed: 'type changed',
};

type DirState =
  | { state: 'loading' }
  | { state: 'error'; failure: GitFailure }
  | { state: 'ready'; entries: TreeEntry[] };

interface Row {
  entry: TreeEntry;
  depth: number;
  parent?: string;
}

export function CodeBrowser(p: { client: GitClient; ws: WsRef; n?: number }) {
  const { client, n } = p;
  const { namespace, repoId, wsId } = p.ws;
  // Primitive parts, so a parent's fresh object literal does not restart the tree.
  const ws = useMemo(() => ({ namespace, repoId, wsId }), [namespace, repoId, wsId]);
  const [dirs, setDirs] = useState<Record<string, DirState>>({});
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [focus, setFocus] = useState<string>();
  const [file, setFile] = useState<{ path: string; state: 'loading' | 'ready' | 'error' }>();
  const [content, setContent] = useState<FileContent>();
  const [fileError, setFileError] = useState<GitFailure>();
  const refs = useRef(new Map<string, HTMLElement>());
  const fileAbort = useRef<AbortController | null>(null);
  const live = useRef(new AbortController());

  const loadDir = useCallback(
    async (path: string) => {
      setDirs((d) => ({ ...d, [path]: { state: 'loading' } }));
      try {
        const entries = await client.getTree({ ...ws, n, path }, live.current.signal);
        setDirs((d) => ({ ...d, [path]: { state: 'ready', entries } }));
      } catch (e) {
        if (isAbort(e)) return;
        setDirs((d) => ({ ...d, [path]: { state: 'error', failure: toFailure(e) } }));
      }
    },
    [client, ws, n],
  );

  // A different workspace or snapshot starts the tree over.
  useEffect(() => {
    live.current = new AbortController();
    setDirs({});
    setOpen(new Set());
    setFocus(undefined);
    setFile(undefined);
    setContent(undefined);
    void loadDir('');
    return () => {
      live.current.abort();
      fileAbort.current?.abort();
    };
  }, [loadDir]);

  const rows = useMemo(() => {
    const out: Row[] = [];
    const walk = (path: string, depth: number) => {
      const d = dirs[path];
      if (d?.state !== 'ready') return;
      for (const entry of d.entries) {
        out.push({ entry, depth, parent: path || undefined });
        if (entry.kind === 'dir' && open.has(entry.path)) walk(entry.path, depth + 1);
      }
    };
    walk('', 0);
    return out;
  }, [dirs, open]);

  const focused = focus ?? rows[0]?.entry.path;

  const toggle = (entry: TreeEntry, to?: boolean) => {
    const isOpen = open.has(entry.path);
    const want = to ?? !isOpen;
    if (want === isOpen) return;
    setOpen((s) => {
      const next = new Set(s);
      if (want) next.add(entry.path);
      else next.delete(entry.path);
      return next;
    });
    if (want && !dirs[entry.path]) void loadDir(entry.path);
  };

  const openFile = async (entry: TreeEntry) => {
    fileAbort.current?.abort();
    const ac = new AbortController();
    fileAbort.current = ac;
    setFile({ path: entry.path, state: 'loading' });
    setContent(undefined);
    setFileError(undefined);
    try {
      const c = await client.readFile({ ...ws, n, path: entry.path }, ac.signal);
      setContent(c);
      setFile({ path: entry.path, state: 'ready' });
    } catch (e) {
      if (isAbort(e)) return;
      setFileError(toFailure(e));
      setFile({ path: entry.path, state: 'error' });
    }
  };

  const activate = (entry: TreeEntry) => {
    if (entry.kind === 'dir') toggle(entry);
    else void openFile(entry);
  };

  const moveTo = (path: string | undefined) => {
    if (!path) return;
    setFocus(path);
    refs.current.get(path)?.focus();
  };

  const onKeyDown = (e: KeyboardEvent, row: Row, i: number) => {
    const { entry } = row;
    const isDir = entry.kind === 'dir';
    const handlers: Record<string, () => void> = {
      ArrowDown: () => moveTo(rows[i + 1]?.entry.path),
      ArrowUp: () => moveTo(rows[i - 1]?.entry.path),
      Home: () => moveTo(rows[0]?.entry.path),
      End: () => moveTo(rows.at(-1)?.entry.path),
      ArrowRight: () => {
        if (!isDir) return;
        if (!open.has(entry.path)) toggle(entry, true);
        else if (rows[i + 1]?.parent === entry.path) moveTo(rows[i + 1]!.entry.path);
      },
      ArrowLeft: () => {
        if (isDir && open.has(entry.path)) toggle(entry, false);
        else moveTo(row.parent);
      },
      Enter: () => activate(entry),
      ' ': () => activate(entry),
    };
    const fn = handlers[e.key];
    if (fn) {
      e.preventDefault();
      fn();
    }
  };

  const root = dirs[''];

  return (
    <div className="grid min-w-0 items-start gap-4 lg:grid-cols-[minmax(220px,300px)_minmax(0,1fr)]">
      <div className="flex min-w-0 flex-col gap-2">
        {root?.state === 'loading' && (
          <p className="m-0 text-sm text-muted" role="status">
            Loading files…
          </p>
        )}
        {root?.state === 'error' && (
          <Notice tone="danger" title="Could not read the tree">
            {root.failure.message}
          </Notice>
        )}
        {rows.length > 0 && (
          <div
            role="tree"
            aria-label={`Files${n !== undefined ? ` at snapshot ${n}` : ''}`}
            className={
              /* mono-ok: path */ 'm-0 max-h-[560px] overflow-y-auto p-0 font-mono text-sm'
            }
          >
            {rows.map((row, i) => {
              const { entry, depth } = row;
              const isDir = entry.kind === 'dir';
              const expanded = isDir ? open.has(entry.path) : undefined;
              const d = dirs[entry.path];
              return (
                <div
                  key={entry.path}
                  ref={(el) => {
                    if (el) refs.current.set(entry.path, el);
                    else refs.current.delete(entry.path);
                  }}
                  role="treeitem"
                  aria-level={depth + 1}
                  aria-expanded={expanded}
                  aria-selected={file?.path === entry.path}
                  aria-busy={isDir && d?.state === 'loading' ? true : undefined}
                  tabIndex={entry.path === focused ? 0 : -1}
                  className={`flex cursor-pointer items-center gap-1 rounded px-1 py-0.5 focus-visible:outline-2 focus-visible:outline-accent ${
                    file?.path === entry.path ? 'bg-accent-soft' : ''
                  }`}
                  style={{ paddingLeft: 4 + depth * 14 }}
                  onFocus={() => setFocus(entry.path)}
                  onClick={() => {
                    setFocus(entry.path);
                    activate(entry);
                  }}
                  onKeyDown={(e) => onKeyDown(e, row, i)}
                >
                  <span aria-hidden="true" className="w-3 text-muted">
                    {isDir ? (expanded ? '▾' : '▸') : ''}
                  </span>
                  <span
                    className={`truncate ${entry.change === 'deleted' ? 'text-muted line-through' : 'text-ink'}`}
                  >
                    {entry.name}
                    {isDir ? '/' : ''}
                  </span>
                  {entry.env && <Badge title="Env file: values are redacted">env</Badge>}
                  {entry.change && (
                    <span
                      className="ml-auto text-xs text-accent-fg"
                      title={CHANGE_TEXT[entry.change]}
                    >
                      <span aria-hidden="true">{CHANGE_LABEL[entry.change]}</span>
                      <span className="sr-only">, {CHANGE_TEXT[entry.change]}</span>
                    </span>
                  )}
                  {isDir && d?.state === 'error' && (
                    <span className="ml-auto text-xs text-danger">could not load</span>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
      <FileView file={file} content={content} error={fileError} />
    </div>
  );
}

function FileView(p: {
  file?: { path: string; state: 'loading' | 'ready' | 'error' };
  content?: FileContent;
  error?: GitFailure;
}) {
  if (!p.file) {
    return <p className="m-0 text-sm text-muted">Choose a file to read it. Files load on open.</p>;
  }
  const isEnv = /^env\/.*\.env$/.test(p.file.path);
  return (
    <section className="flex min-w-0 flex-col gap-2" aria-label={`File ${p.file.path}`}>
      <h3 className={/* mono-ok: path */ 'm-0 flex items-center gap-2 font-mono text-sm'}>
        {p.file.path}
        {p.content && <span className="text-xs text-muted">{formatBytes(p.content.size)}</span>}
      </h3>
      {p.file.state === 'loading' && (
        <p className="m-0 text-sm text-muted" role="status">
          Loading {p.file.path}…
        </p>
      )}
      {p.file.state === 'error' && p.error && (
        <Notice tone="danger" title={`Could not read ${p.file.path}`}>
          {p.error.message}
        </Notice>
      )}
      {p.content?.binary && <Notice title="Binary file">Not shown.</Notice>}
      {p.content?.truncated && (
        <Notice tone="warn" title="Large file">
          Showing the first {formatBytes(MAX_FILE_BYTES)} of {formatBytes(p.content.size)}.
        </Notice>
      )}
      {isEnv && p.content && (
        <Notice title="Env file">
          Values are sealed in the vault. This view shows key names and fingerprints only.
        </Notice>
      )}
      {p.content && !p.content.binary && <Source text={p.content.text} />}
    </section>
  );
}

function Source({ text }: { text: string }) {
  const lines = text.replace(/\n$/, '').split('\n');
  return (
    // A named, focusable region, so the scrollable contents are reachable by keyboard.
    <section
      className="max-h-[560px] overflow-auto rounded-md border border-solid border-rule p-2"
      // oxlint-disable-next-line jsx-a11y/no-noninteractive-tabindex -- a scroll container
      tabIndex={0}
      aria-label="File contents"
    >
      <pre className="m-0 text-xs">
        <code>
          {lines.map((line, i) => (
            // oxlint-disable-next-line react/no-array-index-key -- lines are positional
            <span key={i} className="block">
              <span
                aria-hidden="true"
                className="inline-block w-10 select-none pr-3 text-right text-muted"
              >
                {i + 1}
              </span>
              {line}
            </span>
          ))}
        </code>
      </pre>
    </section>
  );
}
