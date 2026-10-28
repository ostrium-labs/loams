// Parallel runs (SD1 Task 25; design §53 §5.5, D904): the group view's pieces in
// @loams/studio-ui. T3 Code has no compare view (inventory (l)), so this is new code: the
// diff matrix, the side-by-side pair diff (Task 20's viewer), the model's rules.

import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test';
import {
  changeLevel,
  DiffMatrix,
  formatElapsed,
  type GroupMemberView,
  judgePrompt,
  matrixCells,
  memberElapsedMs,
  PairDiff,
  sharedApprovals,
} from '../src/groups';
import { installDomStubs, wrap } from './files-fakes';

beforeAll(installDomStubs);
afterEach(cleanup);

function member(over: Partial<GroupMemberView> & { chatId: string }): GroupMemberView {
  return {
    label: 'Claude Code',
    status: 'done',
    branch: `loams/${over.chatId}`,
    cwd: `/wt/${over.chatId}`,
    filesChanged: 0,
    additions: 0,
    deletions: 0,
    elapsedMs: 0,
    check: { state: 'none', logTail: [] },
    detached: false,
    archived: false,
    pendingApprovals: 0,
    ...over,
  };
}

const MEMBERS = [
  member({ chatId: 'c1', label: 'Claude Code sonnet' }),
  member({ chatId: 'c2', label: 'Codex' }),
  member({ chatId: 'c3', label: 'opencode' }),
];

const MATRIX = [
  {
    path: 'src/login.ts',
    perMember: [
      { chatId: 'c1', additions: 40, deletions: 2 },
      { chatId: 'c3', additions: 3, deletions: 1 },
    ],
  },
  { path: 'src/util.ts', perMember: [{ chatId: 'c2', additions: 1, deletions: 0 }] },
];

describe('matrix_marks_touched_files', () => {
  it('folds the daemon’s rows into one cell per member, with a change level', () => {
    const rows = matrixCells(MATRIX, ['c1', 'c2', 'c3']);
    expect(rows.map((r) => r.path)).toEqual(['src/login.ts', 'src/util.ts']);
    expect(rows[0]?.cells.map((c) => c.touched)).toEqual([true, false, true]);
    expect(rows[1]?.cells.map((c) => c.touched)).toEqual([false, true, false]);
    // Levels are relative to the largest change in the matrix.
    expect(rows[0]?.cells[0]?.level).toBe(3);
    expect(rows[0]?.cells[1]?.level).toBe(0);
    expect(rows[1]?.cells[1]?.level).toBe(1);
    expect(changeLevel(0, 42)).toBe(0);
    expect(changeLevel(42, 42)).toBe(3);
  });

  it('marks each touched cell, says which are untouched, and picks a cell', () => {
    const onSelect = vi.fn();
    render(<DiffMatrix rows={MATRIX} members={MEMBERS} onSelect={onSelect} />);
    const table = screen.getByRole('table', { name: 'Files changed by each variant' });
    const headers = within(table).getAllByRole('columnheader');
    expect(headers.map((h) => h.textContent)).toEqual([
      'File',
      'Claude Code sonnet',
      'Codex',
      'opencode',
    ]);
    const login = within(table).getByRole('row', { name: /src\/login\.ts/ });
    const cells = within(login).getAllByRole('cell');
    expect(cells.map((c) => c.getAttribute('data-touched'))).toEqual(['true', 'false', 'true']);
    expect(cells[0]?.textContent).toContain('+40');
    expect(cells[0]?.textContent).toContain('−2');
    expect(
      within(cells[1] as HTMLElement).getByText('Codex did not change src/login.ts'),
    ).toBeTruthy();
    fireEvent.click(
      within(cells[2] as HTMLElement).getByRole('button', {
        name: 'Compare src/login.ts from opencode',
      }),
    );
    expect(onSelect).toHaveBeenCalledWith({ path: 'src/login.ts', chatId: 'c3' });
  });

  it('says so when no variant changed a file', () => {
    render(<DiffMatrix rows={[]} members={MEMBERS} onSelect={() => {}} />);
    expect(screen.getByText('No variant has changed a file yet.')).toBeTruthy();
  });
});

const PAIR_PATCH = `diff --git a/src/login.ts b/src/login.ts
index 1111111..2222222 100644
--- a/src/login.ts
+++ b/src/login.ts
@@ -1,3 +1,3 @@
 export const a = 1;
-export const b = "<img src=x onerror=alert(1)>";
+export const b = "<script>window.__pwned = 1</script>";
 export const c = 3;
`;

describe('pair_diff_side_by_side', () => {
  it('renders the pair’s diff in two columns, as text', async () => {
    render(wrap(<PairDiff patch={PAIR_PATCH} from="Claude Code sonnet" to="Codex" />));
    expect(screen.getByText('Claude Code sonnet')).toBeTruthy();
    expect(screen.getByText('Codex')).toBeTruthy();
    const host = await waitFor(() => {
      const found = document.querySelector<HTMLElement>(
        '[data-pair-file="src/login.ts"] diffs-container',
      );
      if (!found) throw new Error('no viewer yet');
      return found;
    });
    await waitFor(() =>
      expect(
        host.shadowRoot?.querySelector('[data-diff-type]')?.getAttribute('data-diff-type'),
      ).toBe('split'),
    );
    expect(host.shadowRoot?.textContent).toContain('<script>window.__pwned = 1</script>');
    expect(host.shadowRoot?.querySelector('script, img')).toBeNull();
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined();
  });

  it('says when the two results are the same', () => {
    render(wrap(<PairDiff patch="" from="Codex" to="opencode" />));
    expect(screen.getByText('No difference between these two results.')).toBeTruthy();
  });
});

describe('group model', () => {
  it('approvals pending with the same tool and arguments in several members are shared', () => {
    const shared = sharedApprovals(
      [
        { id: 'a1', chatId: 'c1', tool: 'Bash', args: { command: 'pnpm test', cwd: '.' } },
        { id: 'a2', chatId: 'c2', tool: 'Bash', args: { cwd: '.', command: 'pnpm test' } },
        { id: 'a3', chatId: 'c3', tool: 'Bash', args: { command: 'rm -rf dist' } },
        { id: 'a4', chatId: 'other', tool: 'Bash', args: { command: 'pnpm test', cwd: '.' } },
      ],
      ['c1', 'c2', 'c3'],
    );
    expect(shared).toHaveLength(1);
    expect(shared[0]?.tool).toBe('Bash');
    expect(shared[0]?.approvals).toEqual([
      { id: 'a1', chatId: 'c1' },
      { id: 'a2', chatId: 'c2' },
    ]);
  });

  it('a running member’s time runs on from its start', () => {
    const running = member({ chatId: 'c1', status: 'running', elapsedMs: 1000, startedAtMs: 0 });
    expect(memberElapsedMs(running, 65_000)).toBe(65_000);
    expect(memberElapsedMs(member({ chatId: 'c2', elapsedMs: 5000 }), 99_000)).toBe(5000);
    expect(formatElapsed(65_000)).toBe('1m 05s');
    expect(formatElapsed(3_725_000)).toBe('1h 02m');
    expect(formatElapsed(9_000)).toBe('9s');
  });

  it('the judge’s prompt quotes the comparison as data', () => {
    const prompt = judgePrompt({
      prompt: 'Fix the login redirect',
      base: 'dev',
      members: MEMBERS,
      matrix: MATRIX,
      diffs: { c1: '+ok', c2: '```\n+fence', c3: '' },
    });
    expect(prompt).toContain('Fix the login redirect');
    expect(prompt).toContain('Claude Code sonnet');
    expect(prompt).toContain('src/login.ts');
    expect(prompt).toMatch(/do not (change|edit) any file/i);
    // A diff holding a fence cannot close the block it is quoted in.
    expect(prompt).toContain('````');
  });
});
