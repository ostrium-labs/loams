// The tab and split store (uiStore.ts), with upstream's terminalUiStateStore cases
// ported to daemon-chosen ids, and the stored state checked before it is trusted.

import { beforeEach, describe, expect, it } from 'vite-plus/test';
import {
  closeThreadTerminal,
  DEFAULT_THREAD_TERMINAL_UI_STATE,
  sanitizePersistedTerminals,
  selectThreadTerminalUiState,
  type TerminalTab,
  upsertTerminalIntoGroups,
  useTerminalUiStateStore,
} from '../src/terminal/uiStore';

const tab = (id: string): TerminalTab => ({ id, label: id, cwd: '/w', shell: 'zsh' });
const state = (threadId = 'c1') =>
  selectThreadTerminalUiState(
    useTerminalUiStateStore.getState().terminalUiStateByThreadId,
    threadId,
  );

beforeEach(() => useTerminalUiStateStore.setState({ terminalUiStateByThreadId: {} }));

describe('terminal tab store (upstream cases)', () => {
  it('returns an empty default for unknown threads', () => {
    expect(state('nope')).toBe(DEFAULT_THREAD_TERMINAL_UI_STATE);
  });

  it('opens and splits terminals into the active group', () => {
    const s = useTerminalUiStateStore.getState();
    s.addTerminal('c1', tab('a'));
    s.addTerminal('c1', tab('b'), 'split');
    expect(state().terminalIds).toEqual(['a', 'b']);
    expect(state().terminalGroups).toEqual([{ id: 'group-a', terminalIds: ['a', 'b'] }]);
    expect(state().activeTerminalId).toBe('b');
  });

  it('stacks vertically split terminals in the active group', () => {
    const s = useTerminalUiStateStore.getState();
    s.addTerminal('c1', tab('a'));
    s.addTerminal('c1', tab('b'), 'splitVertical');
    expect(state().terminalGroups[0]?.splitDirection).toBe('vertical');
  });

  it('caps splits at four terminals per group', () => {
    const s = useTerminalUiStateStore.getState();
    s.addTerminal('c1', tab('a'));
    for (const id of ['b', 'c', 'd', 'e']) s.addTerminal('c1', tab(id), 'split');
    expect(state().terminalGroups[0]?.terminalIds).toEqual(['a', 'b', 'c', 'd']);
    expect(state().terminalIds).not.toContain('e');
  });

  it('creates new terminals in a separate group', () => {
    const s = useTerminalUiStateStore.getState();
    s.addTerminal('c1', tab('a'));
    s.addTerminal('c1', tab('b'), 'new');
    expect(state().terminalGroups.map((g) => g.terminalIds)).toEqual([['a'], ['b']]);
    expect(state().activeTerminalGroupId).toBe('group-b');
  });

  it('keeps a valid active terminal after closing an active split terminal', () => {
    let s = upsertTerminalIntoGroups(DEFAULT_THREAD_TERMINAL_UI_STATE, tab('a'), 'new');
    s = upsertTerminalIntoGroups(s, tab('b'), 'split');
    s = upsertTerminalIntoGroups(s, tab('c'), 'split');
    s = closeThreadTerminal(s, 'c');
    expect(s.activeTerminalId).toBe('b');
    expect(s.terminalGroups).toEqual([{ id: 'group-a', terminalIds: ['a', 'b'] }]);
    expect(Object.keys(s.tabs)).toEqual(['a', 'b']);
  });

  it('resets to the default when the last terminal closes', () => {
    const s = useTerminalUiStateStore.getState();
    s.addTerminal('c1', tab('a'));
    s.closeTerminal('c1', 'a');
    expect(useTerminalUiStateStore.getState().terminalUiStateByThreadId).toEqual({});
  });

  it('keeps threads apart', () => {
    const s = useTerminalUiStateStore.getState();
    s.addTerminal('c1', tab('a'));
    s.addTerminal('c2', tab('b'));
    expect(state('c1').terminalIds).toEqual(['a']);
    expect(state('c2').terminalIds).toEqual(['b']);
  });
});

describe('stored terminals', () => {
  it('drops malformed rows and tabs', () => {
    const out = sanitizePersistedTerminals({
      terminalUiStateByThreadId: {
        ok: {
          terminalIds: ['a', 'ghost', 7],
          activeTerminalId: 'a',
          terminalGroups: [{ id: 'g', terminalIds: ['a', 'ghost'] }],
          activeTerminalGroupId: 'g',
          tabs: { a: tab('a'), bad: { id: 'bad' } },
        },
        junk: 'nope',
        empty: { terminalIds: [] },
      },
    });
    expect(Object.keys(out.terminalUiStateByThreadId)).toEqual(['ok']);
    expect(out.terminalUiStateByThreadId.ok).toEqual({
      terminalIds: ['a'],
      activeTerminalId: 'a',
      terminalGroups: [{ id: 'g', terminalIds: ['a'] }],
      activeTerminalGroupId: 'g',
      tabs: { a: tab('a') },
    });
    expect(sanitizePersistedTerminals(null)).toEqual({ terminalUiStateByThreadId: {} });
  });
});
