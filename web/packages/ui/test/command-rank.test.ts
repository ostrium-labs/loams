// palette_ranks_like_upstream (SD1 Task 24): the palette's ranking is T3 Code's
// (CommandPalette.logic.ts, `filterCommandPaletteGroups`), so these cases restate
// upstream's own: every query word must match; an exact field beats a prefix, a
// prefix beats a substring; an earlier search term beats a later one; recency breaks
// a tie between thread titles; a secondary row sorts last; groups keep their order.

import { describe, expect, it } from 'vite-plus/test';
import { normalizeSearchText, type RankableCommand, rankCommands } from '../src/command-rank';

const row = (id: string, text: string, extra: Partial<RankableCommand> = {}): RankableCommand => ({
  id,
  text,
  ...extra,
});

const ids = (rows: readonly RankableCommand[]) => rows.map((r) => r.id);

describe('palette_ranks_like_upstream', () => {
  it('normalises like upstream: accents folded, case and whitespace collapsed', () => {
    expect(normalizeSearchText('  Café   Déjà  VU ')).toBe('cafe deja vu');
  });

  it('keeps every row, in order, for an empty query', () => {
    const rows = [row('b', 'Beta'), row('a', 'Alpha')];
    expect(rankCommands(rows, '   ')).toEqual(rows);
  });

  it('needs every query word somewhere in the row', () => {
    const rows = [row('1', 'Push branch', { keywords: ['git'] }), row('2', 'Pull request')];
    expect(ids(rankCommands(rows, 'git push'))).toEqual(['1']);
    expect(ids(rankCommands(rows, 'p'))).toEqual(['1', '2']);
    expect(rankCommands(rows, 'merge')).toEqual([]);
  });

  it('ranks an exact field above a prefix, and a prefix above a substring', () => {
    const rows = [row('sub', 'Open diff'), row('prefix', 'Diff panel'), row('exact', 'Diff')];
    expect(ids(rankCommands(rows, 'diff'))).toEqual(['exact', 'prefix', 'sub']);
  });

  it('ranks a match in an earlier search term above one in a later term', () => {
    const rows = [
      row('later', 'Fix login', { keywords: ['payments'] }),
      row('first', 'Payments retry'),
    ];
    expect(ids(rankCommands(rows, 'payments'))).toEqual(['first', 'later']);
  });

  it('puts thread titles in one tier, so recency breaks the tie', () => {
    const rows = [
      row('old', 'Refactor parser', { recency: 1 }),
      row('new', 'Parser cleanup', { recency: 9 }),
      row('exact', 'parser', { recency: 0 }),
    ];
    // Upstream: an exact title outranks the tier; the rest sort newest first.
    expect(ids(rankCommands(rows, 'parser'))).toEqual(['exact', 'new', 'old']);
  });

  it('sorts a secondary row after every other match', () => {
    const rows = [row('2nd', 'Theme', { secondary: true }), row('1st', 'Toggle theme')];
    expect(ids(rankCommands(rows, 'theme'))).toEqual(['1st', '2nd']);
  });

  it('ranks within a group and keeps the groups in their first order', () => {
    const rows = [
      row('t1', 'Fix diff view', { group: 'Threads' }),
      row('a1', 'Diff', { group: 'Actions' }),
      row('t2', 'Diff', { group: 'Threads' }),
      row('a2', 'Show diff', { group: 'Actions' }),
    ];
    expect(ids(rankCommands(rows, 'diff'))).toEqual(['t2', 't1', 'a1', 'a2']);
  });

  it('keeps the original order between equal matches', () => {
    const rows = [row('x', 'Go to Jobs'), row('y', 'Go to Logs')];
    expect(ids(rankCommands(rows, 'go'))).toEqual(['x', 'y']);
  });
});
