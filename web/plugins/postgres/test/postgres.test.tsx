import type { PgTimeline } from '@loams/desktop/contracts';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { Branches, flattenTimelines } from '../src/index.js';

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

HTMLDialogElement.prototype.showModal ??= function (this: HTMLDialogElement) {
  this.setAttribute('open', '');
};
HTMLDialogElement.prototype.close ??= function (this: HTMLDialogElement) {
  this.removeAttribute('open');
};

const tl = (timelineId: string, name: string, ancestorTimelineId?: string): PgTimeline => ({
  timelineId,
  name,
  ancestorTimelineId,
  ancestorLsn: ancestorTimelineId ? '0/16B3748' : undefined,
  lastRecordLsn: '0/1A2B3C8',
});
// Deliberately unordered: children before parents.
const TIMELINES = [
  tl('c3', 'feature-x', 'b2'),
  tl('d4', 'hotfix', 'a1'),
  tl('b2', 'dev', 'a1'),
  tl('a1', 'main'),
];

function fakePg() {
  const created: unknown[] = [];
  const pg = {
    tenants: async () => ({ ok: true as const, value: ['t1'] }),
    timelines: async () => ({ ok: true as const, value: TIMELINES }),
    walStatus: async (_t: string, id: string) => ({
      ok: true as const,
      value: { timelineId: id, flushLsn: '0/FF', commitLsn: `0/WAL-${id}` },
    }),
    createBranch: vi.fn(async (_t: string, b: unknown) => {
      created.push(b);
      return { ok: true as const, value: tl('e5', 'x', 'a1') };
    }),
  };
  return { pg, created };
}

describe('postgres branches', () => {
  it('branch_tree_from_timelines', async () => {
    expect(flattenTimelines(TIMELINES).map((r) => [r.timeline.name, r.depth])).toEqual([
      ['main', 0],
      ['dev', 1],
      ['feature-x', 2],
      ['hotfix', 1],
    ]);
    const { pg } = fakePg();
    render(<Branches pg={pg as never} />);
    const rows = await screen.findAllByRole('row');
    // header + 4 timelines, depth-first by ancestor.
    expect(rows.slice(1).map((r) => r.getAttribute('data-depth'))).toEqual(['0', '1', '2', '1']);
    expect(within(rows[3] as HTMLElement).getByText('feature-x')).toBeTruthy();
    expect(within(rows[3] as HTMLElement).getByText('c3')).toBeTruthy();
    expect(within(rows[1] as HTMLElement).getByText('0/WAL-a1')).toBeTruthy();
  });

  it('creates a branch from a row', async () => {
    const { pg, created } = fakePg();
    render(<Branches pg={pg as never} />);
    const rows = await screen.findAllByRole('row');
    fireEvent.click(
      within(rows[2] as HTMLElement).getByRole('button', { name: 'Create branch from here' }),
    );
    fireEvent.change(await screen.findByLabelText('Branch name'), { target: { value: 'try-it' } });
    fireEvent.click(screen.getByRole('button', { name: 'Create branch' }));
    await waitFor(() => expect(created).toEqual([{ name: 'try-it', ancestorTimelineId: 'b2' }]));
  });
});
