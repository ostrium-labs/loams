// A checkout in memory for the file tree and viewer's tests (SD1 Task 21).

import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { vi } from 'vite-plus/test';
import type {
  ContentMatch,
  ContentSearchQuery,
  ContentSearchSource,
  FileChanges,
  FileEntry,
  FileMatch,
  FilesSource,
} from '../src/files/source';

export function wrap(children: ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

/** jsdom has neither ResizeObserver nor object URLs. */
export function installDomStubs() {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  let n = 0;
  URL.createObjectURL = vi.fn(() => `blob:http://localhost/${++n}`);
  URL.revokeObjectURL = vi.fn();
}

export interface FakeCheckout {
  source: FilesSource;
  files: Map<string, string>;
  images: Map<string, { mimeType: string; bytes: Uint8Array }>;
  calls: string[];
  /** Sends a batch of changes to every watcher. */
  emit(changes: FileChanges): void;
  watchers: number;
}

const parentOf = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf('/')));

/** A checkout made of `files` (path → text); directories follow from the paths. */
export function fakeCheckout(
  initial: Record<string, string>,
  o: { search?: (query: string) => FileMatch[] } = {},
): FakeCheckout {
  const files = new Map(Object.entries(initial));
  const images = new Map<string, { mimeType: string; bytes: Uint8Array }>();
  const calls: string[] = [];
  const listeners = new Set<(c: FileChanges) => void>();
  const entriesOf = (directory: string): FileEntry[] => {
    const seen = new Map<string, FileEntry>();
    for (const path of [...files.keys(), ...images.keys()]) {
      if (directory && !path.startsWith(`${directory}/`)) continue;
      const rest = directory ? path.slice(directory.length + 1) : path;
      const [head, ...tail] = rest.split('/');
      if (!head) continue;
      const child = directory ? `${directory}/${head}` : head;
      seen.set(child, { path: child, name: head, kind: tail.length ? 'directory' : 'file' });
    }
    return [...seen.values()];
  };
  const checkout: FakeCheckout = {
    files,
    images,
    calls,
    watchers: 0,
    emit: (changes) => {
      for (const l of listeners) l(changes);
    },
    source: {
      key: 'fake',
      listDirectory: async (directory) => {
        calls.push(`list:${directory}`);
        return { directory, entries: entriesOf(directory), truncated: false };
      },
      readFile: async (path) => {
        calls.push(`read:${path}`);
        const text = files.get(path);
        if (text === undefined) throw new Error('The file could not be read.');
        return { path, text, size: text.length, truncated: false };
      },
      readImage: async (path) => {
        calls.push(`image:${path}`);
        const image = images.get(path);
        if (!image) throw new Error('not an image');
        return image;
      },
      searchFiles: async (query, limit) => {
        calls.push(`search:${query}`);
        if (o.search) return o.search(query).slice(0, limit);
        return [...files.keys()]
          .filter((p) => p.toLowerCase().includes(query.toLowerCase()))
          .map((p) => ({
            path: p,
            name: p.split('/').pop() ?? p,
            kind: 'file' as const,
            score: 1,
          }));
      },
      watch: (listener) => {
        listeners.add(listener);
        checkout.watchers = listeners.size;
        return () => {
          listeners.delete(listener);
          checkout.watchers = listeners.size;
        };
      },
    },
  };
  return checkout;
}

export { parentOf };

/** A content search whose matches the test releases one at a time. */
export function controlledContentSearch() {
  let push: ((m: ContentMatch) => void) | null = null;
  let finish: ((r: { truncated: boolean }) => void) | null = null;
  const queries: ContentSearchQuery[] = [];
  const source: ContentSearchSource = {
    availability: { available: true },
    search: (query, onMatch, signal) => {
      queries.push(query);
      push = onMatch;
      return new Promise((resolve) => {
        finish = resolve;
        signal.addEventListener('abort', () => resolve({ truncated: false }));
      });
    },
  };
  return {
    source,
    queries,
    match: (m: ContentMatch) => push?.(m),
    end: (truncated = false) => finish?.({ truncated }),
  };
}
