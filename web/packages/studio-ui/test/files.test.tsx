// The file tree and viewer (SD1 Task 21): the plan's named tests for @loams/studio-ui.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { StrictMode } from 'react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import {
  ContentSearchDialog,
  FileBrowser,
  FilePicker,
  FileViewer,
  pendingContentSearch,
  rankFileMatches,
} from '../src/files';
import { controlledContentSearch, fakeCheckout, installDomStubs, wrap } from './files-fakes';

beforeAll(installDomStubs);
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const CHECKOUT = {
  'README.md': '# Loams\n',
  'src/main.ts': 'export const main = 1;\n',
  'src/lib/util.ts': 'export const util = 2;\n',
};

/** The source view's text: @pierre/diffs renders it into an open shadow root. */
function sourceHost(): HTMLElement {
  const host = document.querySelector('diffs-container');
  if (!(host instanceof HTMLElement)) throw new Error('no source view');
  return host;
}

describe('tree_lists_and_expands', () => {
  it('lists the root, loads a folder when it is expanded, and opens a file', async () => {
    const co = fakeCheckout(CHECKOUT);
    const onOpenFile = vi.fn();
    render(
      wrap(
        <FileBrowser
          source={co.source}
          projectName="loams"
          selectedPath={null}
          onOpenFile={onOpenFile}
        />,
      ),
    );
    const tree = await screen.findByRole('tree', { name: 'loams files' });
    const src = await within(tree).findByRole('treeitem', { name: 'src' });
    expect(within(tree).getByRole('treeitem', { name: 'README.md' })).toBeTruthy();
    // Folders come first, then files.
    expect(
      within(tree)
        .getAllByRole('treeitem')
        .map((r) => r.getAttribute('data-path')),
    ).toEqual(['src', 'README.md']);
    expect(src.getAttribute('aria-expanded')).toBe('false');
    expect(co.calls).toEqual(['list:']);

    fireEvent.click(src);
    await within(tree).findByRole('treeitem', { name: 'main.ts' });
    expect(co.calls).toContain('list:src');
    expect(within(tree).getByRole('treeitem', { name: 'src' }).getAttribute('aria-expanded')).toBe(
      'true',
    );
    const lib = within(tree).getByRole('treeitem', { name: 'lib' });
    expect(lib.getAttribute('aria-level')).toBe('2');

    // The keyboard: Right on a closed folder opens it.
    lib.focus();
    fireEvent.keyDown(lib, { key: 'ArrowRight' });
    await within(tree).findByRole('treeitem', { name: 'util.ts' });

    fireEvent.click(within(tree).getByRole('treeitem', { name: 'main.ts' }));
    expect(onOpenFile).toHaveBeenCalledWith('src/main.ts');

    // Collapsing keeps the children cached: no second listing.
    fireEvent.click(within(tree).getByRole('treeitem', { name: 'src' }));
    expect(within(tree).queryByRole('treeitem', { name: 'main.ts' })).toBeNull();
    fireEvent.click(within(tree).getByRole('treeitem', { name: 'src' }));
    await within(tree).findByRole('treeitem', { name: 'main.ts' });
    expect(co.calls.filter((c) => c === 'list:src')).toHaveLength(1);
  });

  it('shows an error with a retry, and an empty checkout says so', async () => {
    const co = fakeCheckout({});
    render(
      wrap(
        <FileBrowser
          source={co.source}
          projectName="empty"
          selectedPath={null}
          onOpenFile={vi.fn()}
        />,
      ),
    );
    expect(await screen.findByText('This folder is empty.')).toBeTruthy();

    const broken = fakeCheckout(CHECKOUT);
    broken.source.listDirectory = () => Promise.reject(new Error('Workspace is not a checkout'));
    render(
      wrap(
        <FileBrowser
          source={{ ...broken.source, key: 'broken' }}
          projectName="broken"
          selectedPath={null}
          onOpenFile={vi.fn()}
        />,
      ),
    );
    expect(await screen.findByText(/Workspace is not a checkout/)).toBeTruthy();
    expect(screen.getByRole('button', { name: /Try again/ })).toBeTruthy();
  });
});

describe('tree_updates_on_watch', () => {
  it('lists a folder again when the daemon reports a change in it, and unsubscribes on unmount', async () => {
    const co = fakeCheckout(CHECKOUT);
    const view = render(
      wrap(<FileViewer source={co.source} projectName="loams" path={null} onOpenFile={vi.fn()} />),
    );
    const tree = await screen.findByRole('tree', { name: 'loams files' });
    await within(tree).findByRole('treeitem', { name: 'README.md' });
    expect(co.watchers).toBe(1);

    co.files.set('NOTES.md', 'new');
    act(() => co.emit({ resyncRequired: false, changes: [{ kind: 'created', path: 'NOTES.md' }] }));
    await within(tree).findByRole('treeitem', { name: 'NOTES.md' });

    co.files.delete('README.md');
    act(() => co.emit({ resyncRequired: true, changes: [] }));
    await waitFor(() =>
      expect(within(tree).queryByRole('treeitem', { name: 'README.md' })).toBeNull(),
    );

    view.unmount();
    expect(co.watchers).toBe(0);
  });

  it('reads an open file again when it changes', async () => {
    const co = fakeCheckout(CHECKOUT);
    render(
      wrap(
        <FileViewer
          source={co.source}
          projectName="loams"
          path="src/main.ts"
          onOpenFile={vi.fn()}
        />,
      ),
    );
    await waitFor(() => expect(sourceHost().shadowRoot?.textContent).toContain('main = 1'));
    co.files.set('src/main.ts', 'export const main = 2;\n');
    act(() =>
      co.emit({ resyncRequired: false, changes: [{ kind: 'modified', path: 'src/main.ts' }] }),
    );
    await waitFor(() => expect(sourceHost().shadowRoot?.textContent).toContain('main = 2'));
  });
});

describe('file_viewer_renders_text', () => {
  it('shows markup in a file as text, highlighted, never as elements', async () => {
    const hostile =
      '<script>window.__pwned = 1</script>\n<img src="https://evil.example/x.png" onerror="window.__pwned = 2">\n<a href="javascript:alert(1)">x</a>\n';
    const co = fakeCheckout({ 'evil.html': hostile, 'src/a.ts': 'const a = 1;\n' });
    render(
      wrap(
        <FileViewer source={co.source} projectName="loams" path="evil.html" onOpenFile={vi.fn()} />,
      ),
    );
    await waitFor(() =>
      expect(sourceHost().shadowRoot?.textContent).toContain('window.__pwned = 1'),
    );
    const shadow = sourceHost().shadowRoot as ShadowRoot;
    expect(shadow.textContent).toContain('<img src="https://evil.example/x.png"');
    expect(shadow.querySelector('script, img, a, iframe')).toBeNull();
    expect(document.querySelector('script, img, iframe')).toBeNull();
    expect((window as { __pwned?: number }).__pwned).toBeUndefined();
    // Breadcrumbs name the project and the file.
    expect(screen.getByRole('navigation', { name: 'File path' }).textContent).toContain('loams');
    expect(screen.getByRole('navigation', { name: 'File path' }).textContent).toContain(
      'evil.html',
    );
  });

  it("renders under React's development remount (Strict Mode), as the console runs it", async () => {
    const co = fakeCheckout({ 'src/a.ts': 'const strict = 1;\n' });
    render(
      <StrictMode>
        {wrap(
          <FileViewer source={co.source} projectName="p" path="src/a.ts" onOpenFile={vi.fn()} />,
        )}
      </StrictMode>,
    );
    await waitFor(() => expect(sourceHost().shadowRoot?.textContent).toContain('strict = 1'));
    expect(document.querySelectorAll('diffs-container')).toHaveLength(1);
  });

  it('says why a binary file is not shown, and that a large one is cut short', async () => {
    const co = fakeCheckout({});
    co.source.readFile = async (path) =>
      path === 'a.bin'
        ? { path, text: null, size: 10, truncated: false, readOnlyReason: 'binary' }
        : { path, text: 'x\n', size: 4_000_000, truncated: true };
    const view = render(
      wrap(<FileViewer source={co.source} projectName="p" path="a.bin" onOpenFile={vi.fn()} />),
    );
    expect(await screen.findByText(/binary/i)).toBeTruthy();
    view.unmount();
    render(
      wrap(
        <FileViewer
          source={{ ...co.source, key: 'big' }}
          projectName="p"
          path="big.log"
          onOpenFile={vi.fn()}
        />,
      ),
    );
    expect(await screen.findByText(/first part of a 4,000,000 byte file/)).toBeTruthy();
  });

  it('shows CSV as a table of text cells, with the source a toggle away', async () => {
    const co = fakeCheckout({ 'data.csv': 'name,html\nada,"<b>bold</b>"\n' });
    render(
      wrap(<FileViewer source={co.source} projectName="p" path="data.csv" onOpenFile={vi.fn()} />),
    );
    const table = await screen.findByRole('table', { name: 'data.csv' });
    expect(within(table).getByText('<b>bold</b>')).toBeTruthy();
    expect(table.querySelector('b')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show source' }));
    await waitFor(() => expect(sourceHost().shadowRoot?.textContent).toContain('name,html'));
  });
});

describe('markdown_preview_safe', () => {
  const md =
    '# Title\n\n<script>window.__md = 1</script>\n\n![x](https://tracker.example/p.png)\n\n[click](javascript:alert(1))\n';

  it('without a renderer, markdown is shown as its source text', async () => {
    const co = fakeCheckout({ 'README.md': md });
    render(
      wrap(<FileViewer source={co.source} projectName="p" path="README.md" onOpenFile={vi.fn()} />),
    );
    await waitFor(() => expect(sourceHost().shadowRoot?.textContent).toContain('window.__md = 1'));
    expect(screen.queryByRole('button', { name: /rendered markdown/ })).toBeNull();
    expect(document.querySelector('script, img, iframe')).toBeNull();
    expect(sourceHost().shadowRoot?.querySelector('script, img, a')).toBeNull();
    expect((window as { __md?: number }).__md).toBeUndefined();
  });

  it("the markdown renderer (Task 18's rules) gets the text as a string, and the source is a toggle away", async () => {
    const co = fakeCheckout({ 'README.md': md });
    const renderMarkdown = vi.fn((text: string) => (
      <article data-testid="rendered">{text}</article>
    ));
    render(
      wrap(
        <FileViewer
          source={co.source}
          projectName="p"
          path="README.md"
          onOpenFile={vi.fn()}
          renderMarkdown={renderMarkdown}
        />,
      ),
    );
    const rendered = await screen.findByTestId('rendered');
    expect(renderMarkdown).toHaveBeenCalledWith(md, 'README.md');
    expect(rendered.textContent).toBe(md);
    expect(document.querySelector('script, img, iframe')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Show markdown source' }));
    await waitFor(() => expect(sourceHost().shadowRoot?.textContent).toContain('# Title'));
  });
});

describe('image_preview_local_only', () => {
  it('shows a checkout image from its bytes as a blob: URL, and fetches nothing', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    const co = fakeCheckout({});
    co.images.set('assets/logo.png', { mimeType: 'image/png', bytes: new Uint8Array([137, 80]) });
    render(
      wrap(
        <FileViewer
          source={co.source}
          projectName="p"
          path="assets/logo.png"
          onOpenFile={vi.fn()}
        />,
      ),
    );
    const img = await screen.findByRole('img', { name: 'assets/logo.png' });
    expect(img.getAttribute('src')).toMatch(/^blob:/);
    expect(co.calls).toContain('image:assets/logo.png');
    expect(co.calls).not.toContain('read:assets/logo.png');
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(URL.createObjectURL).toHaveBeenCalledWith(expect.any(Blob));
  });

  it('refuses bytes that are not an image type it shows', async () => {
    const co = fakeCheckout({});
    co.images.set('x.svg', { mimeType: 'text/html', bytes: new Uint8Array([60]) });
    render(
      wrap(<FileViewer source={co.source} projectName="p" path="x.svg" onOpenFile={vi.fn()} />),
    );
    expect(await screen.findByText(/not an image this viewer shows/)).toBeTruthy();
    expect(document.querySelector('img')).toBeNull();
  });
});

describe('quick_open_ranks', () => {
  const matches = [
    { path: 'src/lib/util.ts', name: 'util.ts', kind: 'file' as const, score: 40 },
    { path: 'src', name: 'src', kind: 'directory' as const, score: 90 },
    { path: 'src/main.ts', name: 'main.ts', kind: 'file' as const, score: 80 },
    { path: 'docs/main.md', name: 'main.md', kind: 'file' as const, score: 80 },
  ];

  it("ranks by the daemon's score, keeps its order on ties, lists files only, and marks the matched letters", () => {
    const rows = rankFileMatches(matches, 'mai');
    expect(rows.map((r) => r.path)).toEqual(['src/main.ts', 'docs/main.md', 'src/lib/util.ts']);
    expect(rows[0]?.nameMatchIndices).toEqual([0, 1, 2]);
    expect(rows[2]?.nameMatchIndices).toEqual([]);
  });

  it('the picker searches the checkout and opens the top row', async () => {
    const co = fakeCheckout({}, { search: () => matches });
    const onOpenFile = vi.fn();
    const onOpenChange = vi.fn();
    render(
      wrap(
        <FilePicker
          open
          onOpenChange={onOpenChange}
          source={co.source}
          projectName="loams"
          onOpenFile={onOpenFile}
        />,
      ),
    );
    const input = screen.getByRole('combobox');
    fireEvent.change(input, { target: { value: 'mai' } });
    await waitFor(() => expect(co.calls).toContain('search:mai'));
    const options = await screen.findAllByRole('option');
    expect(options.map((o) => o.textContent)).toEqual([
      expect.stringContaining('src/main.ts'),
      expect.stringContaining('docs/main.md'),
      expect.stringContaining('src/lib/util.ts'),
    ]);
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(onOpenFile).toHaveBeenCalledWith('src/main.ts');
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});

describe('content_search_streams_results', () => {
  it('shows matches as they arrive, grouped by file, and opens one at its line', async () => {
    const search = controlledContentSearch();
    const onOpenMatch = vi.fn();
    render(
      <ContentSearchDialog
        open
        onOpenChange={vi.fn()}
        source={search.source}
        projectName="loams"
        onOpenMatch={onOpenMatch}
      />,
    );
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search in loams' }), {
      target: { value: 'main' },
    });
    await waitFor(() => expect(search.queries.at(-1)?.query).toBe('main'));
    expect(screen.getByText(/Searching/)).toBeTruthy();

    act(() =>
      search.match({ path: 'src/main.ts', line: 1, column: 14, preview: 'export const main = 1;' }),
    );
    expect(await screen.findByText('main.ts')).toBeTruthy();
    expect(screen.getByText(/Searching/)).toBeTruthy();

    act(() =>
      search.match({ path: 'docs/guide.md', line: 7, column: 3, preview: '<b>main</b> entry' }),
    );
    await screen.findByText('guide.md');
    // A match is text: markup in a line stays text, and the match is marked.
    expect(document.querySelector('b')).toBeNull();
    expect(screen.getAllByText('main', { selector: 'mark' }).length).toBe(2);

    await act(async () => search.end());
    expect(await screen.findByText('2 results in 2 files')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: /guide\.md, line 7/ }));
    expect(onOpenMatch).toHaveBeenCalledWith(
      expect.objectContaining({ path: 'docs/guide.md', line: 7 }),
    );
  });

  it('passes the options, and says when the daemon cannot search contents yet', async () => {
    const search = controlledContentSearch();
    const view = render(
      <ContentSearchDialog
        open
        onOpenChange={vi.fn()}
        source={search.source}
        projectName="p"
        onOpenMatch={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Match case' }));
    fireEvent.click(screen.getByRole('button', { name: 'Use regular expression' }));
    fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'ma.n' } });
    await waitFor(() =>
      expect(search.queries.at(-1)).toMatchObject({
        query: 'ma.n',
        caseSensitive: true,
        regex: true,
      }),
    );
    view.unmount();

    render(
      <ContentSearchDialog
        open
        onOpenChange={vi.fn()}
        source={pendingContentSearch()}
        projectName="p"
        onOpenMatch={vi.fn()}
      />,
    );
    expect(screen.getByText(/needs a newer loams-agentd/)).toBeTruthy();
    expect((screen.getByRole('searchbox') as HTMLInputElement).disabled).toBe(true);
  });
});

describe("mention_from_tree_adds_chip (the tree's half)", () => {
  it('offers Mention in composer on a row and reports the path', async () => {
    const co = fakeCheckout(CHECKOUT);
    const onMention = vi.fn();
    render(
      wrap(
        <FileBrowser
          source={co.source}
          projectName="loams"
          selectedPath={null}
          onOpenFile={vi.fn()}
          onMention={onMention}
        />,
      ),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Mention README.md in composer' }));
    expect(onMention).toHaveBeenCalledWith('README.md');
  });
});
