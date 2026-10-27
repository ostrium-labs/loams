// The diff panel and review comments (SD1 Task 20): the plan's named tests, over an
// in-memory DiffSource, plus upstream's unit cases for the vendored logic.

import type { FileDiffMetadata } from '@pierre/diffs';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeAll, beforeEach, describe, expect, it } from 'vite-plus/test';
import { DiffPanel, defaultBaseRef, useDiffPanelStore } from '../src/diff';
import { DIFF_TOKEN_VARIABLES, diffSurfaceCss } from '../src/diff/diffTheme';
import { isCommentSubmitShortcut } from '../src/diff/review/commentSubmitShortcut';
import { DIFF_VIEW_UNSAFE_CSS } from '../src/diff/review/StyledDiffCodeView';
import {
  anchorToSelection,
  formatAnchorLabel,
  selectionToAnchor,
} from '../src/diff/reviewCommentContext';
import { diffFileTreeRows } from '../src/diff/tree/DiffFileTree';
import {
  buildDiffFileTreeUpdates,
  collectDirectoryPaths,
  diffFileTreeEntries,
} from '../src/diff/tree/diffFileTree.logic';
import { checkoutDiff, comment, type FakeDiff, fakeDiff, WORKING_PATCH } from './diff-fakes';
import { installDomStubs, wrap } from './files-fakes';

beforeAll(installDomStubs);
beforeEach(() => {
  useDiffPanelStore.setState({
    byThread: {},
    branchBaseRefByThread: {},
    viewedByThread: {},
    view: {
      layout: 'unified',
      wrap: false,
      ignoreWhitespace: false,
      wordDiff: true,
      treeOpen: false,
    },
  });
});
afterEach(cleanup);

function mount(fake: FakeDiff, onOpenFile?: (path: string) => void) {
  return render(
    wrap(
      <DiffPanel
        threadId="thread-1"
        source={fake.source}
        {...(onOpenFile ? { onOpenFile } : {})}
      />,
    ),
  );
}

/** Each file's viewer: @pierre/diffs renders it into an open shadow root. */
function viewers(): HTMLElement[] {
  return [...document.querySelectorAll<HTMLElement>('diffs-container')];
}

function viewerFor(path: string): HTMLElement {
  const section = document.querySelector<HTMLElement>(`[data-diff-file="${path}"]`);
  const host = section?.querySelector<HTMLElement>('diffs-container');
  if (!host) throw new Error(`no viewer for ${path}`);
  return host;
}

describe('diff_renders_text_not_html', () => {
  it('shows a patch’s markup as text inside the shadow root, never as elements', async () => {
    mount(fakeDiff());
    await waitFor(() =>
      expect(viewerFor('src/app.ts').shadowRoot?.textContent).toContain(
        '<script>window.__pwned = 1</script>',
      ),
    );
    await waitFor(() =>
      expect(viewerFor('README.md').shadowRoot?.textContent).toContain(
        '<a href="javascript:alert(1)">link</a>',
      ),
    );
    for (const host of viewers()) {
      const shadow = host.shadowRoot as ShadowRoot;
      expect(shadow.querySelector('script, img, iframe, a[href]')).toBeNull();
    }
    expect(document.querySelector('script, img')).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it('keeps a comment body as text', async () => {
    mount(fakeDiff({ comments: [comment({ body: '<img src=x onerror=alert(1)> **bold**' })] }));
    await screen.findByText('<img src=x onerror=alert(1)> **bold**');
    expect(document.querySelector('img')).toBeNull();
  });
});

describe('diff_sources_switch', () => {
  it('asks the daemon for the branch, commit and latest-turn diffs', async () => {
    const fake = fakeDiff();
    mount(fake);
    await waitFor(() => expect(viewerFor('src/app.ts')).toBeTruthy());
    expect(fake.getDiff).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Branch' }));
    await waitFor(() =>
      expect(fake.getDiff).toHaveBeenCalledWith(
        { kind: 'branch', baseRef: 'main' },
        expect.anything(),
      ),
    );
    await waitFor(() => expect(viewerFor('src/branch.ts')).toBeTruthy());
    fireEvent.change(screen.getByLabelText('Compare against'), { target: { value: 'feature' } });
    await waitFor(() =>
      expect(fake.getDiff).toHaveBeenCalledWith(
        { kind: 'branch', baseRef: 'feature' },
        expect.anything(),
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Commit' }));
    await waitFor(() =>
      expect(fake.getDiff).toHaveBeenCalledWith(
        { kind: 'commit', commitSha: 'abcdef0123456789' },
        expect.anything(),
      ),
    );
    await waitFor(() => expect(viewerFor('src/commit.ts')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Latest turn' }));
    await waitFor(() =>
      expect(fake.getDiff).toHaveBeenCalledWith({ kind: 'turn' }, expect.anything()),
    );
    await waitFor(() => expect(viewerFor('src/turn.ts')).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: 'Working tree' }));
    await waitFor(() => expect(viewerFor('src/app.ts')).toBeTruthy());
  });

  it('prefers a main branch as the default base', () => {
    expect(defaultBaseRef(['feature', 'main'])).toBe('main');
    expect(defaultBaseRef(['feature', 'master'])).toBe('master');
    expect(defaultBaseRef(['only'])).toBe('only');
    expect(defaultBaseRef([])).toBeNull();
  });
});

describe('split_and_unified', () => {
  it('renders one column by default and two after "Split"', async () => {
    mount(fakeDiff());
    await waitFor(() =>
      expect(viewerFor('src/app.ts').shadowRoot?.querySelector('[data-diff-type]')).not.toBeNull(),
    );
    const type = () =>
      viewerFor('src/app.ts')
        .shadowRoot?.querySelector('[data-diff-type]')
        ?.getAttribute('data-diff-type');
    expect(type()).toBe('single');
    fireEvent.click(screen.getByRole('button', { name: 'Split' }));
    await waitFor(() => expect(type()).toBe('split'));
    fireEvent.click(screen.getByRole('button', { name: 'Unified' }));
    await waitFor(() => expect(type()).toBe('single'));
  });
});

describe('comment_anchor_round_trip', () => {
  it('maps a selection to agentd’s anchor and back', () => {
    for (const range of [
      { start: 3, side: 'additions', end: 3, endSide: 'additions' },
      { start: 4, side: 'additions', end: 7, endSide: 'additions' },
      { start: 9, side: 'deletions', end: 9, endSide: 'deletions' },
    ] as const) {
      expect(anchorToSelection(selectionToAnchor(range))).toEqual(range);
    }
    expect(
      selectionToAnchor({ start: 7, side: 'additions', end: 4, endSide: 'additions' }),
    ).toEqual({ side: 'new', line: 4, endLine: 7 });
    // A range across sides keeps its last line, on that side.
    expect(
      selectionToAnchor({ start: 2, side: 'deletions', end: 5, endSide: 'additions' }),
    ).toEqual({ side: 'new', line: 5 });
    expect(formatAnchorLabel({ side: 'new', line: 4, endLine: 7 })).toBe('4 to 7');
    expect(formatAnchorLabel({ side: 'old', line: 9 })).toBe('9 (old)');
  });

  it('shows a stored comment at its line and saves an edit with the same anchor', async () => {
    const fake = fakeDiff({ comments: [comment({ endLine: 3 })] });
    mount(fake);
    const body = await screen.findByText('Why a <script> here?');
    // The comment sits in the slot the viewer left under its last line, on its side.
    const slot = body.closest('[slot]');
    expect(slot?.getAttribute('slot')).toContain('additions');
    expect(slot?.getAttribute('slot')).toContain('3');
    expect(screen.getByText('Line 2 to 3')).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Edit comment' }));
    const box = await screen.findByRole('textbox', { name: 'Comment on line 2 to 3' });
    fireEvent.change(box, { target: { value: 'Use textContent instead.' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(fake.upsert).toHaveLength(1));
    expect(fake.upsert[0]).toMatchObject({
      id: 'c1',
      checkoutId: 'co-1',
      path: 'src/app.ts',
      side: 'new',
      line: 2,
      endLine: 3,
      body: 'Use textContent instead.',
      diffChecksum: 'sum-1',
    });
  });
});

describe('outdated_comment_badge', () => {
  it('marks a comment the daemon calls outdated, and still shows it', async () => {
    const fake = fakeDiff({
      comments: [comment({ id: 'gone', line: 40, outdated: true, body: 'Old remark' })],
    });
    mount(fake);
    const body = await screen.findByText('Old remark');
    const card = body.closest('[data-diff-comment-annotation]') as HTMLElement;
    expect(within(card).getByText('outdated')).toBeTruthy();
    expect(screen.getByText(/1 outdated/)).toBeTruthy();
    act(() => fake.setComments([comment({ id: 'gone', line: 2, body: 'Old remark' })]));
    await waitFor(() => {
      const current = screen.getByText('Old remark').closest('[data-diff-comment-annotation]');
      expect(within(current as HTMLElement).queryByText('outdated')).toBeNull();
    });
  });
});

describe('send_review_to_thread_calls_rpc', () => {
  it('sends the open comments to the agent', async () => {
    const fake = fakeDiff({
      comments: [
        comment({ id: 'a' }),
        comment({ id: 'b', line: 3, outdated: true }),
        comment({ id: 'done', line: 1, resolved: true }),
      ],
    });
    mount(fake);
    fireEvent.click(await screen.findByRole('button', { name: /Send 2 comments to the agent/ }));
    await waitFor(() => expect(fake.sendReview).toHaveBeenCalledWith(['a', 'b']));
    await screen.findByText('Sent 2 comments to the agent.');
  });
});

describe('post_review_only_with_change_request', () => {
  it('offers "Post to PR" only while a pull request is open, without outdated comments', async () => {
    const fake = fakeDiff({
      comments: [comment({ id: 'a' }), comment({ id: 'b', outdated: true })],
    });
    mount(fake);
    await screen.findByRole('button', { name: /Send 2 comments/ });
    expect(screen.queryByRole('button', { name: /Post to PR/ })).toBeNull();

    act(() =>
      fake.setChangeRequest({
        number: 7,
        title: 'Fix',
        url: 'https://forge.example/pr/7',
        provider: 'forgejo',
      }),
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Post to PR #7' }));
    await waitFor(() => expect(fake.postReview).toHaveBeenCalledWith('co-1', ['a'], 'comment'));

    act(() => fake.setChangeRequest(null));
    await waitFor(() => expect(screen.queryByRole('button', { name: /Post to PR/ })).toBeNull());
  });
});

describe('discard_requires_confirmation_and_checksum', () => {
  it('discards everything only after confirming, with the diff’s checksum', async () => {
    const fake = fakeDiff({ checksum: 'sum-9' });
    mount(fake);
    fireEvent.click(await screen.findByRole('button', { name: 'Discard all changes' }));
    expect(fake.discardAll).not.toHaveBeenCalled();
    const dialog = await screen.findByRole('alertdialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(fake.discardAll).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole('button', { name: 'Discard all changes' }));
    fireEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Discard all' }),
    );
    await waitFor(() => expect(fake.discardAll).toHaveBeenCalledWith('co-1', 'sum-9'));
  });

  it('discards one file with the checksum, and says so when the diff moved', async () => {
    const fake = fakeDiff({ checksum: 'sum-2', stale: true });
    mount(fake);
    fireEvent.click(await screen.findByRole('button', { name: 'Discard changes to README.md' }));
    fireEvent.click(
      within(await screen.findByRole('alertdialog')).getByRole('button', { name: 'Discard file' }),
    );
    await waitFor(() =>
      expect(fake.discardPaths).toHaveBeenCalledWith('co-1', ['README.md'], 'sum-2'),
    );
    await screen.findByText(/The diff changed since you last saw it/);
  });

  it('offers no discard outside the working tree', async () => {
    mount(fakeDiff());
    await screen.findByRole('button', { name: 'Discard all changes' });
    fireEvent.click(screen.getByRole('button', { name: 'Latest turn' }));
    await waitFor(() => expect(viewerFor('src/turn.ts')).toBeTruthy());
    expect(screen.queryByRole('button', { name: 'Discard all changes' })).toBeNull();
    expect(screen.queryByRole('button', { name: /Discard changes to/ })).toBeNull();
  });
});

describe('diff_theme_tokens_only', () => {
  const LITERAL =
    /#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})(?![\w-])|\b(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch)\(\s*[-\d.]/gi;

  it('paints the viewer from @loams/ui tokens and no colour literal', () => {
    for (const scheme of ['light', 'dark'] as const) {
      const css = `${diffSurfaceCss(scheme)}\n${DIFF_VIEW_UNSAFE_CSS}`;
      expect(css.match(LITERAL)).toBeNull();
      expect(css).toContain(`color-scheme: ${scheme}`);
    }
    for (const value of Object.values(DIFF_TOKEN_VARIABLES)) {
      expect(value).toMatch(/^var\(--(op|loams-font)-[a-z-]+\)$/);
    }
  });

  it('follows the console’s dark class', async () => {
    document.documentElement.classList.add('dark');
    try {
      mount(fakeDiff());
      await waitFor(() =>
        expect(viewerFor('src/app.ts').shadowRoot?.innerHTML).toContain('color-scheme: dark'),
      );
    } finally {
      document.documentElement.classList.remove('dark');
    }
  });
});

describe('the diff panel', () => {
  it('lists the files in a tree and opens one in Files', async () => {
    const opened: string[] = [];
    mount(fakeDiff(), (path) => opened.push(path));
    await waitFor(() => expect(viewerFor('src/app.ts')).toBeTruthy());
    fireEvent.click(screen.getByRole('button', { name: 'File tree' }));
    const tree = await screen.findByRole('tree', { name: 'Working tree files' });
    expect(within(tree).getByRole('treeitem', { name: 'src' })).toBeTruthy();
    expect(within(tree).getByRole('treeitem', { name: 'README.md' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Open src/app.ts in Files' }));
    expect(opened).toEqual(['src/app.ts']);
  });

  it('collapses a file marked viewed', async () => {
    mount(fakeDiff());
    await waitFor(() => expect(viewerFor('README.md')).toBeTruthy());
    const section = document.querySelector('[data-diff-file="README.md"]') as HTMLElement;
    fireEvent.click(within(section).getByLabelText('Viewed'));
    await waitFor(() => expect(section.querySelector('diffs-container')).toBeNull());
    expect(within(section).getByRole('button', { name: 'Expand README.md' })).toBeTruthy();
  });

  it('says so when the working tree has no changes', async () => {
    const fake = fakeDiff();
    fake.source.watchWorkingDiff = (listener) => {
      listener({ ...checkoutDiff('', 'empty'), files: [], additions: 0, deletions: 0 });
      return () => {};
    };
    mount(fake);
    await screen.findByText('The working tree matches HEAD.');
  });
});

// Upstream's cases (apps/web/src/components/diffs/diffFileTree.logic.test.ts and
// commentSubmitShortcut.test.ts at 6524a8d4), ported to vitest; the @pierre/trees reading
// order case is checked on the VirtualTree rows instead.
function file(type: FileDiffMetadata['type'], name: string, prevName = name): FileDiffMetadata {
  return { type, name, prevName } as FileDiffMetadata;
}

describe('upstream diffFileTree.logic cases', () => {
  it('maps each change type to its git status under the file’s current path', () => {
    expect(
      diffFileTreeEntries([
        file('new', 'a/src/a.ts'),
        file('deleted', 'src/b.ts'),
        file('rename-pure', 'src/c.ts', 'src/old-c.ts'),
        file('rename-changed', 'src/d.ts', 'src/old-d.ts'),
        file('change', 'README.md'),
      ]),
    ).toEqual([
      { path: 'a/src/a.ts', status: 'added' },
      { path: 'src/b.ts', status: 'deleted' },
      { path: 'src/c.ts', status: 'renamed' },
      { path: 'src/d.ts', status: 'renamed' },
      { path: 'README.md', status: 'modified' },
    ]);
  });

  it('folds a file-to-symlink type change into one modified entry', () => {
    expect(
      diffFileTreeEntries([
        file('change', 'CLAUDE.md'),
        file('deleted', 'AGENTS.md'),
        file('new', 'AGENTS.md'),
        file('new', 'docs/new.md'),
      ]),
    ).toEqual([
      { path: 'CLAUDE.md', status: 'modified' },
      { path: 'AGENTS.md', status: 'modified' },
      { path: 'docs/new.md', status: 'added' },
    ]);
  });

  it('lists every ancestor once, parents first, with a trailing slash', () => {
    expect(collectDirectoryPaths(['apps/web/src/a.ts', 'apps/web/b.ts', 'README.md'])).toEqual([
      'apps/',
      'apps/web/',
      'apps/web/src/',
    ]);
  });

  it('places folders and files where their first diff appears', () => {
    const rows = diffFileTreeRows(
      [
        { path: 'apps/mobile/src/state/shell.ts', status: 'modified' },
        { path: 'apps/mobile/src/features/threads/route.ts', status: 'modified' },
        { path: 'apps/mobile/src/features/threads/screen.tsx', status: 'added' },
      ],
      new Set(),
    ).map((row) => row.path);
    expect(rows).toEqual([
      'apps/',
      'apps/mobile/',
      'apps/mobile/src/',
      'apps/mobile/src/state/',
      'apps/mobile/src/state/shell.ts',
      'apps/mobile/src/features/',
      'apps/mobile/src/features/threads/',
      'apps/mobile/src/features/threads/route.ts',
      'apps/mobile/src/features/threads/screen.tsx',
    ]);
  });

  it('builds tree updates: directories before files, and removals deepest first', () => {
    expect(buildDiffFileTreeUpdates(['README.md'], ['README.md', 'src/lib/a.ts'])).toEqual([
      { type: 'add', path: 'src/' },
      { type: 'add', path: 'src/lib/' },
      { type: 'add', path: 'src/lib/a.ts' },
    ]);
    expect(buildDiffFileTreeUpdates(['src/lib/a.ts', 'src/b.ts'], ['src/b.ts'])).toEqual([
      { type: 'remove', path: 'src/lib/a.ts' },
      { type: 'remove', path: 'src/lib/', recursive: true },
    ]);
    expect(buildDiffFileTreeUpdates(['src/a.ts'], ['src/a.ts'])).toEqual([]);
  });
});

describe('upstream commentSubmitShortcut cases', () => {
  it('accepts Command or Ctrl+Enter only while an eligible comment is idle', () => {
    expect(
      isCommentSubmitShortcut({ key: 'Enter', metaKey: true, ctrlKey: false }, 'Looks good', false),
    ).toBe(true);
    expect(
      isCommentSubmitShortcut({ key: 'Enter', metaKey: false, ctrlKey: true }, 'Looks good', false),
    ).toBe(true);
    expect(
      isCommentSubmitShortcut({ key: 'Enter', metaKey: true, ctrlKey: false }, 'Looks good', true),
    ).toBe(false);
    expect(
      isCommentSubmitShortcut({ key: 'Enter', metaKey: true, ctrlKey: false }, '   ', false),
    ).toBe(false);
    expect(
      isCommentSubmitShortcut({ key: 'K', metaKey: true, ctrlKey: false }, 'Looks good', false),
    ).toBe(false);
  });
});

describe('review_draft_survives_thread_updates', () => {
  /** Opens a draft by editing the stored comment, and types into it. */
  async function openDraft(text: string): Promise<HTMLTextAreaElement> {
    await screen.findByText('Why a <script> here?');
    fireEvent.click(screen.getByRole('button', { name: 'Edit comment' }));
    const box = (await screen.findByRole('textbox', {
      name: /^Comment on line/,
    })) as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: text } });
    return box;
  }

  it('keeps the viewer and the draft when the watch sends the same diff again', async () => {
    const fake = fakeDiff({ comments: [comment()] });
    mount(fake);
    await waitFor(() =>
      expect(viewerFor('src/app.ts').shadowRoot?.querySelector('[data-line]')).not.toBeNull(),
    );
    const box = await openDraft('Half-written');
    const line = viewerFor('src/app.ts').shadowRoot?.querySelector('[data-line]') as Element;
    // The daemon's watch re-sends an unchanged snapshot (a new object, a later timestamp).
    act(() => fake.emitWorking({ ...checkoutDiff(WORKING_PATCH, 'sum-1'), updatedAtMs: 2 }));
    expect(line.isConnected).toBe(true);
    expect(box.isConnected).toBe(true);
    expect(box.value).toBe('Half-written');
    // A changed diff is shown.
    act(() =>
      fake.emitWorking(
        checkoutDiff(WORKING_PATCH.replace('export const d = 4;', 'export const d = 5;'), 'sum-2'),
      ),
    );
    await waitFor(() => expect(line.isConnected).toBe(false));
  });

  it('keeps an open draft when the host hands over a new source for the same thread', async () => {
    const fake = fakeDiff({ comments: [comment()] });
    const view = mount(fake);
    const box = await openDraft('Half-written');
    // A host rebuilds its source as the thread's row changes (its branch renamed); the new
    // object's watch answers later, as a daemon's does.
    const renamed: typeof fake.source = {
      ...fake.source,
      watchWorkingDiff(listener, onError) {
        const timer = setTimeout(() => fake.source.watchWorkingDiff(listener, onError), 10);
        return () => clearTimeout(timer);
      },
    };
    view.rerender(wrap(<DiffPanel threadId="thread-1" source={renamed} />));
    await act(() => new Promise((resolve) => setTimeout(resolve, 30)));
    expect(screen.queryByText('Loading changes')).toBeNull();
    expect(box.isConnected).toBe(true);
    expect(box.value).toBe('Half-written');
  });
});
