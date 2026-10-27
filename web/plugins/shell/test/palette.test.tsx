// The shell's command palette (SD1 Task 24; §53 §4.5, §5.10; T3-6): Mod+K in both
// modes, built on @loams/ui's CommandPalette. Plugins contribute through the
// `shell.palette.source` slot and its alias `palette.command` (CF1 Task 11, W9); the current mode's commands come first and the other
// mode's follow under their mode's heading.

import { type Mode, ModeController, type SessionService } from '@loams/console-host';
import { type PaletteCommand, SlotProvider, SlotRegistry, type SlotSpec } from '@loams/slots';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vite-plus/test';
import { Layout } from '../src/layout.js';
import { HashRouter, type HashSource } from '../src/router.js';

function memory(start = '/'): HashSource {
  let path = start;
  const listeners = new Set<() => void>();
  return {
    get: () => path,
    set: (p) => {
      path = p;
      for (const l of [...listeners]) l();
    },
    listen: (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
  };
}

const session = {
  principal: () => undefined,
  subscribe: () => () => {},
} as unknown as SessionService;

function setup(current: Mode = 'console') {
  const slots = new SlotRegistry();
  const source = memory(current === 'studio' ? '/studio' : '/');
  const router = new HashRouter(slots, source);
  const mode = new ModeController({
    current,
    available: ['console', 'studio'],
    modes: new Map<string, readonly Mode[]>([
      ['pages', ['console']],
      ['studio', ['studio']],
    ]),
    navigate: (to) => router.navigate(to),
    enter: () => {},
  });
  render(
    <SlotProvider registry={slots}>
      <Layout router={router} session={session} mode={mode} />
    </SlotProvider>,
  );
  return { slots, router, mode, source };
}

/** A `palette.command` entry that publishes a fixed list. */
function contribute(
  slots: SlotRegistry,
  spec: Omit<SlotSpec<'palette.command'>, 'name'>,
  commands: PaletteCommand[],
) {
  return slots.register({ name: 'palette.command', ...spec }, ({ publish }) => {
    useEffect(() => publish(commands), [publish]);
    return null;
  });
}

function nav(slots: SlotRegistry, plugin: string, label: string, href: string, group: string) {
  slots.register(
    { name: 'shell.nav.section', plugin, order: 10, meta: { id: label, label, href, group } },
    () => null,
  );
}

/** The palette's rows and headings, in order: headings as `# Label`. */
function listed(): string[] {
  const list = document.querySelector('[data-slot="command-list"]');
  if (!list) return [];
  return [...list.querySelectorAll('[data-slot="command-item"], [role="presentation"]')].map(
    (el) =>
      el.getAttribute('data-slot') === 'command-item'
        ? (el.querySelector('span')?.textContent ?? '')
        : `# ${el.textContent}`,
  );
}

const openPalette = async () => {
  act(() => {
    fireEvent.keyDown(window, { key: 'k', code: 'KeyK', ctrlKey: true });
  });
  return screen.findByRole('dialog', { name: 'Command palette' });
};

beforeEach(() => {
  localStorage.clear();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

describe('the shell palette', () => {
  it('palette_scopes_by_mode', async () => {
    const { slots, mode } = setup('console');
    act(() => {
      nav(slots, 'pages', 'Jobs', '/jobs', 'Compute');
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'studio',
          order: 0,
          meta: {
            id: 'tasks',
            label: 'Tasks',
            href: '/studio/tasks',
            group: 'Studio',
            mode: 'studio',
          },
        },
        () => null,
      );
      contribute(slots, { plugin: 'pages' }, [
        { id: 'run', title: 'Run pipeline', group: 'Pipelines', run: () => {} },
      ]);
      contribute(slots, { plugin: 'studio' }, [
        { id: 't1', title: 'Fix login', group: 'Threads', run: () => {} },
      ]);
      // A console-only plugin cannot put a command in Studio (§53 §4.1).
      contribute(slots, { plugin: 'pages', meta: { mode: 'studio' } }, [
        { id: 'x', title: 'Sneaky', run: () => {} },
      ]);
    });

    await openPalette();
    await waitFor(() => expect(listed()).toContain('Fix login'));
    expect(listed()).toEqual([
      '# Pipelines',
      'Run pipeline',
      // Every page is the current mode's (D1080), a Studio page too.
      '# Pages',
      'Go to Tasks',
      'Go to Jobs',
      '# General',
      'Switch to Studio',
      '# Appearance',
      'Theme: System',
      'Theme: Dark',
      'Theme: Light',
      '# Studio',
      'Fix login',
    ]);

    // In Studio the order flips, and Console's commands sit under "Console".
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Command palette' }), {
      key: 'Escape',
    });
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull(),
    );
    act(() => mode.set('studio'));
    await openPalette();
    await waitFor(() => expect(listed()).toContain('Run pipeline'));
    expect(listed()).toEqual([
      '# Threads',
      'Fix login',
      '# Pages',
      'Go to Tasks',
      'Go to Jobs',
      '# General',
      'Switch to Console',
      '# Appearance',
      'Theme: System',
      'Theme: Dark',
      'Theme: Light',
      '# Console',
      'Run pipeline',
    ]);
    expect(listed()).not.toContain('Sneaky');
  });

  it('ranks and filters the query, and runs a command, closing the palette', async () => {
    const { slots, router } = setup('console');
    const run = vi.fn();
    act(() => {
      nav(slots, 'pages', 'Jobs', '/jobs', 'Compute');
      contribute(slots, { plugin: 'pages' }, [
        { id: 'restart', title: 'Restart job', group: 'Jobs', keywords: ['rerun'], run },
      ]);
    });
    await openPalette();
    const input = await screen.findByRole('combobox', { name: 'Command palette' });
    fireEvent.change(input, { target: { value: 'rerun' } });
    await waitFor(() => expect(listed()).toEqual(['# Jobs', 'Restart job']));
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull(),
    );

    await openPalette();
    const again = await screen.findByRole('combobox', { name: 'Command palette' });
    // The query starts empty each time the palette opens.
    expect((again as HTMLInputElement).value).toBe('');
    fireEvent.change(again, { target: { value: 'go jobs' } });
    await waitFor(() => expect(listed()).toEqual(['# Pages', 'Go to Jobs']));
    fireEvent.keyDown(again, { key: 'Enter' });
    await waitFor(() => expect(router.current().path).toBe('/jobs'));
  });

  it('reads shell.palette.source as well as palette.command (W9)', async () => {
    const { slots } = setup('console');
    act(() => {
      slots.register({ name: 'shell.palette.source', plugin: 'pages' }, ({ publish }) => {
        useEffect(
          () =>
            publish([{ id: 'p', title: 'Go to project acme', group: 'Projects', run: () => {} }]),
          [publish],
        );
        return null;
      });
    });
    contribute(slots, { plugin: 'pages' }, [
      { id: 'old', title: 'An alias row', group: 'Projects', run: () => {} },
    ]);
    await openPalette();
    expect(await screen.findByRole('option', { name: /Go to project acme/ })).toBeTruthy();
    expect(screen.getByRole('option', { name: /An alias row/ })).toBeTruthy();
  });

  it('Mod+K toggles the palette', async () => {
    setup('console');
    await openPalette();
    act(() => {
      fireEvent.keyDown(window, { key: 'k', code: 'KeyK', ctrlKey: true });
    });
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: 'Command palette' })).toBeNull(),
    );
  });

  it('keeps the palette working when a contributed command throws', async () => {
    const { slots } = setup('console');
    act(() => {
      contribute(slots, { plugin: 'pages' }, [
        {
          id: 'boom',
          title: 'Explode',
          run: () => {
            throw new Error('no daemon');
          },
        },
      ]);
    });
    await openPalette();
    const input = await screen.findByRole('combobox', { name: 'Command palette' });
    fireEvent.change(input, { target: { value: 'explode' } });
    await waitFor(() => expect(listed()).toContain('Explode'));
    fireEvent.keyDown(input, { key: 'Enter' });
    expect(await screen.findByText('no daemon')).toBeTruthy();
  });
});
