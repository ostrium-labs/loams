// The thread's Files tab (SD1 Task 21): the tree and viewer over a daemon's
// `FileService` (an in-memory Connect router), and "Mention in composer".

import { create } from '@bufbuild/protobuf';
import { createRouterTransport } from '@connectrpc/connect';
import { AgentdTransportProvider } from '@loams/console-host/query';
import {
  FileService,
  type WatchFilesResponse,
  WatchFilesResponseSchema,
  WorkspaceDirectoryPageSchema,
  WorkspaceEntryKind,
  WorkspaceFileChangeKind,
  WorkspaceFileTextSchema,
  WorkspaceImageChunkSchema,
  WorkspaceTextEncoding,
} from '@loams/proto/agentd';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import { fileLinkOf, routeOf, studioHref } from '../src/routes.js';
import { FilesTab } from '../src/thread/FilesTab.js';
import { agentdFilesSource } from '../src/thread/files-source.js';
import { createComposerMentions, mentionsFor } from '../src/thread/mentions.js';
import { webFake, wrap } from './fakes.js';
import { StudioInShell } from './shell.js';

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
  URL.createObjectURL = vi.fn(() => 'blob:http://localhost/1');
  URL.revokeObjectURL = vi.fn();
});
afterEach(cleanup);

/** A queue the watch stream reads from, so a test can push a change. */
function changeFeed() {
  const pending: WatchFilesResponse[] = [];
  let wake: (() => void) | null = null;
  return {
    push(changes: { kind: WorkspaceFileChangeKind; path: string }[]) {
      pending.push(create(WatchFilesResponseSchema, { changes: { sequence: 1n, changes } }));
      wake?.();
    },
    async *stream(signal: AbortSignal) {
      while (!signal.aborted) {
        const next = pending.shift();
        if (next) yield next;
        else await new Promise<void>((resolve) => (wake = resolve));
      }
    },
  };
}

function fakeFiles() {
  const files: Record<string, string> = {
    'README.md': '# Hello\n',
    'src/main.ts': 'export const main = "<b>1</b>";\n',
  };
  const calls: string[] = [];
  const feed = changeFeed();
  const transport = createRouterTransport(({ service }) => {
    service(FileService, {
      listDirectory: ({ target, directory }) => {
        calls.push(`ListDirectory:${target?.chatId}:${directory}`);
        const names = new Map<string, WorkspaceEntryKind>();
        for (const path of [...Object.keys(files), 'logo.png']) {
          if (directory && !path.startsWith(`${directory}/`)) continue;
          const rest = directory ? path.slice(directory.length + 1) : path;
          const [head, ...tail] = rest.split('/');
          if (head)
            names.set(
              directory ? `${directory}/${head}` : head,
              tail.length ? WorkspaceEntryKind.DIRECTORY : WorkspaceEntryKind.FILE,
            );
        }
        return {
          page: create(WorkspaceDirectoryPageSchema, {
            checkoutId: 'checkout-1',
            directory,
            entries: [...names].map(([path, kind]) => ({
              path,
              name: path.split('/').pop() ?? path,
              kind,
            })),
          }),
        };
      },
      readFile: ({ path }) => {
        calls.push(`ReadFile:${path}`);
        return {
          file: create(WorkspaceFileTextSchema, {
            checkoutId: 'checkout-1',
            path,
            text: files[path] ?? '',
            size: BigInt((files[path] ?? '').length),
            encoding: WorkspaceTextEncoding.UTF8,
            contentHash: 'h1',
          }),
        };
      },
      readImage: ({ path, offset, expectedCheckoutId }) => {
        calls.push(`ReadImage:${path}:${offset}:${expectedCheckoutId}`);
        const first = offset === 0n;
        return {
          chunk: create(WorkspaceImageChunkSchema, {
            checkoutId: expectedCheckoutId,
            contentHash: 'img',
            mimeType: 'image/png',
            // Two chunks, as the daemon sends a large image (base64 of 4 bytes each).
            data: first
              ? btoa(String.fromCharCode(137, 80, 78, 71))
              : btoa(String.fromCharCode(1, 2, 3, 4)),
            nextOffset: first ? 4n : 8n,
            size: 8n,
            done: !first,
          }),
        };
      },
      search: ({ query }) => {
        calls.push(`Search:${query}`);
        return {
          matches: Object.keys(files)
            .filter((p) => p.includes(query))
            .map((path) => ({
              path,
              name: path.split('/').pop() ?? path,
              kind: WorkspaceEntryKind.FILE,
              score: 10n,
            })),
        };
      },
      watchFiles: (_req, ctx) => feed.stream(ctx.signal),
    });
  });
  return { transport, files, calls, feed };
}

function showFiles(
  daemon: ReturnType<typeof fakeFiles>,
  props: Partial<Parameters<typeof FilesTab>[0]> = {},
) {
  return render(
    wrap(
      <AgentdTransportProvider transport={daemon.transport}>
        <FilesTab threadId="chat-7" projectName="loams" {...props} />
      </AgentdTransportProvider>,
    ),
  );
}

describe('mention_from_tree_adds_chip', () => {
  it('a mention from the tree becomes a chip for the thread composer, and can be removed', async () => {
    const daemon = fakeFiles();
    const mentions = createComposerMentions();
    showFiles(daemon, { mentions });
    fireEvent.click(await screen.findByRole('button', { name: 'Mention README.md in composer' }));
    const chips = screen.getByRole('list', { name: 'Mentions for the composer' });
    expect(within(chips).getByText('@README.md')).toBeTruthy();
    expect(mentions.list()).toEqual(['README.md']);

    // A folder's file, from the keyboard: "@" on the focused row.
    fireEvent.click(screen.getByRole('treeitem', { name: 'src' }));
    const main = await screen.findByRole('treeitem', { name: 'main.ts' });
    fireEvent.keyDown(main, { key: '@' });
    expect(within(chips).getByText('@main.ts')).toBeTruthy();
    expect(mentions.list()).toEqual(['README.md', 'src/main.ts']);

    fireEvent.click(screen.getByRole('button', { name: 'Remove README.md' }));
    expect(mentions.list()).toEqual(['src/main.ts']);
  });

  it("a thread's queue is the one the composer reads", () => {
    expect(mentionsFor('chat-7')).toBe(mentionsFor('chat-7'));
    expect(mentionsFor('chat-7')).not.toBe(mentionsFor('chat-8'));
  });
});

describe('the Files tab over FileService', () => {
  it("lists the thread's checkout, opens a file as text and follows the daemon's changes", async () => {
    const daemon = fakeFiles();
    showFiles(daemon);
    fireEvent.click(await screen.findByRole('treeitem', { name: 'src' }));
    fireEvent.click(await screen.findByRole('treeitem', { name: 'main.ts' }));
    expect(daemon.calls).toContain('ListDirectory:chat-7:');
    expect(daemon.calls).toContain('ListDirectory:chat-7:src');
    await waitFor(() => {
      const host = document.querySelector('diffs-container');
      expect(host?.shadowRoot?.textContent).toContain('export const main = "<b>1</b>";');
    });
    expect(document.querySelector('diffs-container')?.shadowRoot?.querySelector('b')).toBeNull();

    daemon.files['NEW.md'] = 'new';
    act(() => daemon.feed.push([{ kind: WorkspaceFileChangeKind.CREATED, path: 'NEW.md' }]));
    expect(await screen.findByRole('treeitem', { name: 'NEW.md' })).toBeTruthy();
  });

  it('reads an image in chunks, naming the checkout, and shows it as a blob: URL', async () => {
    const daemon = fakeFiles();
    showFiles(daemon);
    fireEvent.click(await screen.findByRole('treeitem', { name: 'logo.png' }));
    const img = await screen.findByRole('img', { name: 'logo.png' });
    expect(img.getAttribute('src')).toMatch(/^blob:/);
    expect(daemon.calls).toContain('ReadImage:logo.png:0:checkout-1');
    expect(daemon.calls).toContain('ReadImage:logo.png:4:checkout-1');
    const blob = vi.mocked(URL.createObjectURL).mock.calls.at(-1)?.[0] as Blob;
    expect(blob.type).toBe('image/png');
    expect(blob.size).toBe(8);
  });

  it('Go to file searches the checkout; Search in files waits for Task 11', async () => {
    const daemon = fakeFiles();
    showFiles(daemon);
    fireEvent.click(await screen.findByRole('button', { name: 'Go to file' }));
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'main' } });
    await waitFor(() => expect(daemon.calls).toContain('Search:main'));
    fireEvent.click(await screen.findByRole('option', { name: /main\.ts/ }));
    await waitFor(() =>
      expect(screen.getByRole('navigation', { name: 'File path' }).textContent).toContain(
        'main.ts',
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Search in files' }));
    expect(await screen.findByText(/needs a newer loams-agentd/)).toBeTruthy();
  });

  it('keys the cache by transport and checkout', () => {
    const a = fakeFiles();
    const b = fakeFiles();
    expect(agentdFilesSource(a.transport, { chatId: 'x' }).key).not.toBe(
      agentdFilesSource(b.transport, { chatId: 'x' }).key,
    );
    expect(agentdFilesSource(a.transport, { chatId: 'x' }).key).toBe(
      agentdFilesSource(a.transport, { chatId: 'x' }).key,
    );
  });
});

describe('file links in the route (T22-7)', () => {
  it('routeOf reads path, line and column on the Files panel, and studioHref writes them back', () => {
    const route = routeOf('thread', {
      threadId: 'chat-7',
      panel: 'files',
      path: 'src/main.ts',
      line: '12',
      column: '4',
    });
    expect(route).toEqual({
      kind: 'thread',
      threadId: 'chat-7',
      panel: 'files',
      file: { path: 'src/main.ts', line: 12, column: 4 },
    });
    expect(studioHref(route)).toBe(
      '/studio/t/chat-7?panel=files&path=src%2Fmain.ts&line=12&column=4',
    );
    // Only on the Files panel; other tabs never carry it.
    expect(routeOf('thread', { threadId: 't', panel: 'diff', path: 'a.ts' })).toEqual({
      kind: 'thread',
      threadId: 't',
      panel: 'diff',
    });
    expect(studioHref({ kind: 'thread', threadId: 't', panel: 'files' })).toBe(
      '/studio/t/t?panel=files',
    );
  });

  it('drops unsafe paths and bad numbers', () => {
    const file = (params: Record<string, string>) => fileLinkOf({ panel: 'files', ...params });
    expect(file({ path: '/etc/passwd' })).toBeUndefined();
    expect(file({ path: '../x' })).toBeUndefined();
    expect(file({ path: 'a/../../x' })).toBeUndefined();
    expect(file({ path: 'a\\b' })).toBeUndefined();
    expect(file({ path: 'a\nb' })).toBeUndefined();
    expect(file({ path: 'a//b' })).toBeUndefined();
    expect(file({ path: 'a.ts', line: '0' })).toEqual({ path: 'a.ts' });
    expect(file({ path: 'a.ts', line: '-3', column: '2' })).toEqual({ path: 'a.ts' });
    expect(file({ path: 'a.ts', line: '3', column: 'x' })).toEqual({ path: 'a.ts', line: 3 });
  });

  it('a link opens its file in the viewer', async () => {
    const daemon = fakeFiles();
    showFiles(daemon, { link: { path: 'src/main.ts', line: 1 } });
    await waitFor(() =>
      expect(screen.getByRole('navigation', { name: 'File path' }).textContent).toContain(
        'main.ts',
      ),
    );
    expect(daemon.calls).toContain('ReadFile:src/main.ts');
  });
});

describe('the thread route', () => {
  it('?panel=files shows the Files tab', async () => {
    window.matchMedia = ((query: string) => ({
      matches: query.includes('min-width'),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
    const { agentd } = webFake({ host: { id: 'h1', name: 'Host' } });
    render(
      wrap(
        <StudioInShell
          agentd={agentd}
          route={{ kind: 'thread', threadId: 'chat-7', panel: 'files' }}
          navigate={vi.fn()}
        />,
      ),
    );
    expect(await screen.findByRole('searchbox', { name: 'Search Worktree files' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Go to file' })).toBeTruthy();
  });
});
