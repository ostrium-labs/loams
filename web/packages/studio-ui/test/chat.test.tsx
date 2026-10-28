// Studio's chat timeline (SD1 Task 18): every transcript part renders, deltas re-render
// only their row, and scrolling follows the end and keeps its place on a prepend.
import {
  EMPTY_TRANSCRIPT,
  type MessagePart,
  reduceTranscript,
  type SessionMessageEntry,
  type ToolCall,
  type TranscriptUpdate,
} from '@loams/agentd-client';
import { act, cleanup, fireEvent, render, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import * as fixtures from '../../agentd-client/src/fixtures.gen';
import {
  computeStableTimelineRows,
  deriveTimelineRows,
  findTimelineMatches,
  resolveToolPresentation,
  summarizeToolGroup,
  Timeline,
  type ToolPart,
  TranscriptStore,
} from '../src/chat';
import { EMPTY_STABLE_ROWS } from '../src/chat/timeline.logic';

vi.mock('../src/markdown/highlight', () => ({
  highlightToTokens: vi.fn(async () => {
    throw new Error('no highlighter in tests');
  }),
}));

afterEach(cleanup);

const transcriptFixtures = Object.entries(fixtures).filter(([name]) =>
  name.startsWith('fixture_TranscriptUpdate_'),
) as [string, TranscriptUpdate][];
const partFixtures = Object.entries(fixtures).filter(([name]) =>
  name.startsWith('fixture_MessagePart_'),
) as [string, MessagePart][];

function fold(...frames: TranscriptUpdate[]): SessionMessageEntry[] {
  let state = EMPTY_TRANSCRIPT;
  for (const frame of frames) {
    state = reduceTranscript(state, frame);
    expect(state.desync).toBeNull();
  }
  return state.entries;
}

const TOOL_CALLS: ToolCall[] = [
  { kind: 'exec', command: "bash -lc 'cargo build --release'" },
  { kind: 'readFile', path: 'src/lib.rs' },
  { kind: 'writeFile', path: 'src/new.rs' },
  { kind: 'editFile', path: 'src/main.rs' },
  { kind: 'applyPatch', path: 'Cargo.toml' },
  { kind: 'search', pattern: 'fn main', path: 'src' },
  { kind: 'glob', pattern: '**/*.rs' },
  { kind: 'webFetch', url: 'https://docs.rs' },
  { kind: 'webSearch', query: 'rust linker flag' },
  { kind: 'mcp', server: 'loams', tool: 'search' },
  { kind: 'unknown', name: 'Mystery' },
];

function tool(id: string, call: ToolCall, extra: Partial<ToolPart> = {}): ToolPart {
  return { kind: 'tool', id, call, isError: false, resolved: true, output: `${id} ok`, ...extra };
}

function entry(
  id: string,
  role: SessionMessageEntry['role'],
  parts: MessagePart[],
  extra: Partial<SessionMessageEntry> = {},
): SessionMessageEntry {
  return { id, role, parts, createdAt: 1_791_797_400_000, deviceId: 'device-1', ...extra };
}

describe('timeline_renders_every_part_fixture', () => {
  it('folds every TranscriptUpdate fixture and renders each part kind', () => {
    expect(transcriptFixtures.map(([n]) => n).sort()).toEqual([
      'fixture_TranscriptUpdate_delta',
      'fixture_TranscriptUpdate_reset',
    ]);
    const entries = fold(
      ...transcriptFixtures.map(([, f]) => f).sort((a) => ('reset' in a ? -1 : 1)),
    );
    const { container, getByText } = render(<Timeline entries={entries} />);
    const kinds = [...container.querySelectorAll('[data-row-kind]')].map((n) =>
      n.getAttribute('data-row-kind'),
    );
    expect(kinds).toEqual([
      'user',
      'text',
      'image',
      'reasoning',
      'subagent',
      'input',
      'error',
      'fork',
      'assistant-meta',
      'turn',
    ]);
    getByText('Fix the build', { selector: 'p' });
    getByText('Building now. Done.');
    getByText('shot.png');
    getByText('Thought');
    getByText('Subagent working');
    getByText('Which branch?');
    getByText('rate limited');
    getByText('Forked from Fix the build');
    getByText('Changed 1 file');
  });

  it('covers every MessagePart fixture kind', () => {
    const kinds = new Set(partFixtures.map(([, part]) => part.kind));
    expect([...kinds].sort()).toEqual(
      ['error', 'fork', 'image', 'input', 'reasoning', 'text', 'tool'].sort(),
    );
    const entries = [
      entry('u1', 'user', [{ kind: 'text', id: 't', text: 'Go' }]),
      entry(
        'a1',
        'assistant',
        partFixtures.map(([, part]) => part),
        { status: 'complete' },
      ),
    ];
    const { container } = render(<Timeline entries={entries} />);
    for (const [name, part] of partFixtures) {
      expect(container.querySelector(`[data-row-id^="a1/${part.id}"]`), name).not.toBeNull();
    }
  });

  it('renders every tool call kind, a plan, a failure and a running call', () => {
    const parts: MessagePart[] = [
      ...TOOL_CALLS.map((call, i) => tool(`c${i}`, call)),
      { kind: 'text', id: 'mid', text: 'Now the plan.' },
      tool('plan', {
        kind: 'todo',
        items: [
          { text: 'Find the flag', done: true },
          { text: 'Fix the build', done: false },
          { text: 'Run the tests', done: false },
        ],
      }),
      tool('bad', { kind: 'exec', command: 'false' }, { isError: true }),
      tool('live', { kind: 'exec', command: 'sleep 9' }, { resolved: false, output: null }),
    ];
    const entries = [
      entry('u1', 'user', [{ kind: 'text', id: 't', text: 'Go' }]),
      entry('a1', 'assistant', parts, { status: 'streaming' }),
    ];
    const { container, getByText } = render(<Timeline entries={entries} />);
    const work = container.querySelector('[data-row-kind="work"]');
    expect(work).not.toBeNull();
    // Eleven calls collapse into one summary line: two actions, then the rest.
    expect(work?.textContent).toBe('Ran 1 command, changed 3 files, and performed 7 other actions');
    fireEvent.click(within(work as HTMLElement).getByRole('button', { expanded: false }));
    for (const call of TOOL_CALLS) {
      const part = tool('x', call);
      expect(work?.textContent).toContain(resolveToolPresentation(part).label);
    }
    expect(work?.textContent).toContain('cargo build --release');
    // The plan card, the failed call, the running call and the working row.
    getByText('Fix the build');
    expect(container.querySelector('[data-tasks-progress]')?.textContent).toBe('1/3');
    const last = [...container.querySelectorAll('[data-row-kind="work"]')].at(-1) as HTMLElement;
    expect(last.textContent).toBe('Ran 2 commands');
    expect(within(last).getByRole('status', { name: 'Running' })).toBeTruthy();
    fireEvent.click(within(last).getByRole('button', { expanded: false }));
    expect(last.textContent).toContain('Failed to run false');
    expect(last.textContent).toContain('Running sleep 9');
    expect(container.querySelector('[data-row-kind="working"]')).not.toBeNull();
  });

  it('summarises tool groups like upstream', () => {
    const parts = [
      tool('a', { kind: 'exec', command: 'ls' }),
      tool('b', { kind: 'exec', command: 'pwd' }),
      tool('c', { kind: 'readFile', path: 'a' }),
    ];
    expect(summarizeToolGroup(parts)).toEqual({
      summary: 'Ran 2 commands and read 1 file',
      hasFailure: false,
    });
    expect(
      summarizeToolGroup([tool('d', { kind: 'exec', command: 'x' }, { isError: true })]),
    ).toEqual({ summary: 'Ran 1 command', hasFailure: true });
  });
});

describe('delta_rerenders_only_its_row', () => {
  const reset: TranscriptUpdate = {
    contextUsage: null,
    reset: [
      entry('u1', 'user', [{ kind: 'text', id: 't0', text: 'Hello' }]),
      entry(
        'a1',
        'assistant',
        [
          { kind: 'reasoning', id: 'r', text: 'Thinking it over.' },
          tool('c1', { kind: 'readFile', path: 'README.md' }),
          { kind: 'text', id: 't1', text: 'Hi' },
        ],
        { status: 'streaming' },
      ),
    ],
  };
  const append: TranscriptUpdate = {
    contextUsage: null,
    upsert: [],
    append: [{ entry: 'a1', part: 't1', text: ' there', len: 8 }],
    remove: [],
    count: 2,
  };

  it('keeps every other row object across an append', () => {
    const before = computeStableTimelineRows(
      deriveTimelineRows({ entries: fold(reset) }),
      EMPTY_STABLE_ROWS,
    );
    const after = computeStableTimelineRows(
      deriveTimelineRows({ entries: fold(reset, append) }),
      before,
    );
    const changed = after.result.filter((row, i) => row !== before.result[i]).map((r) => r.id);
    expect(changed).toEqual(['a1/t1']);
  });

  it('re-renders only the appended row', () => {
    const probe = vi.fn();
    const first = fold(reset);
    const { rerender, getByText } = render(<Timeline entries={first} renderProbe={probe} />);
    expect(probe.mock.calls.map(([id]) => id)).toEqual([
      'u1',
      'a1/r',
      'a1/c1:work',
      'a1/t1',
      'working',
    ]);
    probe.mockClear();
    rerender(<Timeline entries={fold(reset, append)} renderProbe={probe} />);
    expect(probe.mock.calls.map(([id]) => id)).toEqual(['a1/t1']);
    getByText('Hi there');
  });
});

describe('stick_to_bottom_and_anchor_on_prepend', () => {
  const ROW = 100;
  const VIEW = 300;
  let restore: (() => void)[] = [];

  beforeEach(() => {
    restore = [];
    const define = (name: 'scrollHeight' | 'clientHeight', get: (el: HTMLElement) => number) => {
      const original = Object.getOwnPropertyDescriptor(HTMLElement.prototype, name);
      Object.defineProperty(HTMLElement.prototype, name, {
        configurable: true,
        get(this: HTMLElement) {
          return this.getAttribute('role') === 'log' ? get(this) : 0;
        },
      });
      restore.push(() => {
        if (original) Object.defineProperty(HTMLElement.prototype, name, original);
      });
    };
    define('scrollHeight', (el) => el.querySelectorAll('[data-row-id]').length * ROW);
    define('clientHeight', () => VIEW);
    const rect = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function (this: Element) {
      const log = this.closest('[role="log"]') as HTMLElement | null;
      if (!log) return rect.call(this);
      if (this === log) return { top: 0, height: VIEW, bottom: VIEW } as DOMRect;
      const rows = [...log.querySelectorAll('[data-row-id]')];
      const index = rows.indexOf(this);
      const top = index * ROW - log.scrollTop;
      return { top, height: ROW, bottom: top + ROW } as DOMRect;
    };
    restore.push(() => {
      Element.prototype.getBoundingClientRect = rect;
    });
  });
  afterEach(() => {
    for (const undo of restore.reverse()) undo();
  });

  const messages = (ids: string[]) =>
    ids.map((id) => entry(id, 'user', [{ kind: 'text', id: 't', text: `message ${id}` }]));

  it('follows the end while at the bottom, and stays put once scrolled up', () => {
    const { rerender, getByRole } = render(
      <Timeline entries={messages(['m1', 'm2', 'm3', 'm4', 'm5'])} />,
    );
    const log = getByRole('log');
    expect(log.scrollTop).toBe(500);
    rerender(<Timeline entries={messages(['m1', 'm2', 'm3', 'm4', 'm5', 'm6'])} />);
    expect(log.scrollTop).toBe(600);
    // The person scrolls up: new rows no longer move the view.
    act(() => {
      log.scrollTop = 120;
      fireEvent.scroll(log);
    });
    rerender(<Timeline entries={messages(['m1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm7'])} />);
    expect(log.scrollTop).toBe(120);
    // Back within the re-arm band of the bottom (40 px): following again.
    act(() => {
      log.scrollTop = 7 * ROW - VIEW - 30;
      fireEvent.scroll(log);
    });
    rerender(<Timeline entries={messages(['m1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm7', 'm8'])} />);
    expect(log.scrollTop).toBe(800);
  });

  it('keeps the first visible row in place when older rows are prepended', () => {
    const { rerender, getByRole } = render(
      <Timeline entries={messages(['m3', 'm4', 'm5', 'm6', 'm7'])} />,
    );
    const log = getByRole('log');
    act(() => {
      log.scrollTop = 150; // m4 (top 100) is the first visible row, 50 px above the top
      fireEvent.scroll(log);
    });
    rerender(<Timeline entries={messages(['m1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm7'])} />);
    // m4 now starts at 300; it stays 50 px above the viewport's top.
    expect(log.scrollTop).toBe(350);
  });
});

describe('find in thread', () => {
  it('matches rows by their text and marks the active one', () => {
    const entries = [
      entry('u1', 'user', [{ kind: 'text', id: 't', text: 'Fix the linker' }]),
      entry('a1', 'assistant', [{ kind: 'text', id: 't', text: 'The LINKER flag was missing.' }], {
        status: 'complete',
      }),
    ];
    const rows = deriveTimelineRows({ entries });
    const matches = findTimelineMatches(rows, 'linker');
    expect(matches).toEqual(['u1', 'a1/t']);
    const { container } = render(
      <Timeline entries={entries} findMatches={matches} activeMatchId="a1/t" />,
    );
    expect(container.querySelector('[data-find-match="active"]')?.getAttribute('data-row-id')).toBe(
      'a1/t',
    );
    expect(container.querySelectorAll('[data-find-match]')).toHaveLength(2);
  });
});

describe('restore_turn_requires_confirmation', () => {
  it('asks before restoring, and restores only on confirm', async () => {
    const onRestoreTurn = vi.fn(async () => undefined);
    const entries = [
      entry('u1', 'user', [{ kind: 'text', id: 't', text: 'Edit it' }]),
      entry(
        'a1',
        'assistant',
        [
          tool(
            'c1',
            { kind: 'editFile', path: 'src/a.ts' },
            { diffStats: [{ path: 'src/a.ts', additions: 3, deletions: 1 }] },
          ),
        ],
        { status: 'complete' },
      ),
    ];
    const { getByRole, findByRole, queryByRole } = render(
      <Timeline entries={entries} checkpoints={new Set(['u1'])} onRestoreTurn={onRestoreTurn} />,
    );
    fireEvent.click(getByRole('button', { name: /Restore to before this turn/ }));
    const dialog = await findByRole('alertdialog');
    expect(onRestoreTurn).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(onRestoreTurn).not.toHaveBeenCalled();
    await act(async () => {});
    expect(queryByRole('alertdialog')).toBeNull();
    fireEvent.click(getByRole('button', { name: /Restore to before this turn/ }));
    fireEvent.click(
      within(await findByRole('alertdialog')).getByRole('button', { name: 'Restore' }),
    );
    expect(onRestoreTurn).toHaveBeenCalledWith('u1');
  });

  it('offers no restore without a checkpoint, nor in the compact variant', () => {
    const entries = [
      entry('u1', 'user', [{ kind: 'text', id: 't', text: 'Edit it' }]),
      entry('a1', 'assistant', [tool('c1', { kind: 'editFile', path: 'src/a.ts' })], {
        status: 'complete',
      }),
    ];
    const without = render(<Timeline entries={entries} onRestoreTurn={vi.fn()} />);
    expect(without.queryByRole('button', { name: /Restore/ })).toBeNull();
    without.unmount();
    const compact = render(
      <Timeline
        entries={entries}
        variant="compact"
        checkpoints={new Set(['u1'])}
        onRestoreTurn={vi.fn()}
      />,
    );
    expect(compact.queryByRole('button', { name: /Restore/ })).toBeNull();
  });
});

describe('TranscriptStore', () => {
  it('folds a watch, shares it between views and reopens on a desync', async () => {
    const opened: string[] = [];
    const ctl: { push?: (u: TranscriptUpdate) => void } = {};
    const watch = (chatId: string, signal: AbortSignal) => {
      opened.push(chatId);
      const queue: TranscriptUpdate[] = [
        {
          contextUsage: null,
          reset: [entry('u1', 'user', [{ kind: 'text', id: 't', text: 'hi' }])],
        },
      ];
      let wake: (() => void) | null = null;
      ctl.push = (u) => {
        queue.push(u);
        wake?.();
      };
      return {
        async *[Symbol.asyncIterator]() {
          while (!signal.aborted) {
            const next = queue.shift();
            if (next) yield next;
            else
              await new Promise<void>((r) => {
                wake = r;
                signal.addEventListener('abort', () => r(), { once: true });
              });
          }
        },
      };
    };
    const store = new TranscriptStore(watch);
    const a = vi.fn();
    const b = vi.fn();
    const offA = store.subscribe('chat-1', a);
    const offB = store.subscribe('chat-1', b);
    await vi.waitFor(() => expect(store.get('chat-1').status).toBe('live'));
    expect(opened).toEqual(['chat-1']);
    expect(store.get('chat-1').entries.map((e) => e.id)).toEqual(['u1']);
    // A delta whose anchor is missing: the store resubscribes for a reset.
    ctl.push?.({
      contextUsage: null,
      upsert: [{ after: 'nope', entry: entry('x', 'assistant', []) }],
      append: [],
      remove: [],
      count: 2,
    });
    await vi.waitFor(() => expect(opened).toEqual(['chat-1', 'chat-1']));
    offA();
    expect(store.isWatching('chat-1')).toBe(true);
    offB();
    expect(store.isWatching('chat-1')).toBe(false);
  });
});
