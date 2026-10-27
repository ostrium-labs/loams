import { describe, expect, it, vi } from 'vite-plus/test';
import {
  localThemeStore,
  parseThemeChoice,
  resolveDark,
  ThemeController,
  type ThemeStore,
} from '../src/theme.js';

/** A matchMedia whose light/dark answer can be flipped, with change listeners. */
function os(prefers: 'light' | 'dark' | 'none') {
  const listeners = new Set<() => void>();
  let state = prefers;
  const list = {
    get matches() {
      return state === 'light';
    },
    addEventListener: (_: string, l: () => void) => listeners.add(l),
    removeEventListener: (_: string, l: () => void) => listeners.delete(l),
  };
  return {
    matchMedia: () => list as unknown as MediaQueryList,
    match: (q: string) => (q.includes('light') ? state === 'light' : state === 'dark'),
    flip(next: 'light' | 'dark') {
      state = next;
      for (const l of listeners) l();
    },
  };
}

function memoryStore(initial = 'system'): ThemeStore & { writes: [string, boolean][] } {
  let value = initial;
  const writes: [string, boolean][] = [];
  return {
    writes,
    read: () => parseThemeChoice(value),
    write: (choice, dark) => {
      value = choice;
      writes.push([choice, dark]);
    },
  };
}

describe('theme selection (D1074)', () => {
  it('follows the OS, and is dark when the OS states no preference', () => {
    expect(resolveDark('system', os('light').match)).toBe(false);
    expect(resolveDark('system', os('dark').match)).toBe(true);
    expect(resolveDark('system', os('none').match)).toBe(true);
    expect(resolveDark('light', os('dark').match)).toBe(false);
    expect(resolveDark('dark', os('light').match)).toBe(true);
  });

  it('reads anything unknown as System', () => {
    expect(parseThemeChoice('dark')).toBe('dark');
    expect(parseThemeChoice('sepia')).toBe('system');
    expect(parseThemeChoice(null)).toBe('system');
  });

  it('toggles .dark on <html>, follows an OS flip while on System, and saves each change', () => {
    const env = os('light');
    const root = document.createElement('html');
    const store = memoryStore();
    const theme = new ThemeController(store, { matchMedia: env.matchMedia, root });
    expect(root.classList.contains('dark')).toBe(false);
    const seen = vi.fn();
    theme.subscribe(seen);
    env.flip('dark');
    expect(theme.dark).toBe(true);
    expect(root.classList.contains('dark')).toBe(true);
    expect(store.writes.at(-1)).toEqual(['system', true]);
    theme.set('light');
    expect(root.classList.contains('dark')).toBe(false);
    expect(store.writes.at(-1)).toEqual(['light', false]);
    env.flip('light');
    env.flip('dark');
    expect(theme.dark).toBe(false); // an explicit choice ignores the OS
    expect(seen).toHaveBeenCalled();
    theme.dispose();
  });

  it('keeps working when storage throws (a private window)', () => {
    const broken = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    };
    const store = localThemeStore(broken);
    expect(store.read()).toBe('system');
    expect(() => store.write('dark', true)).not.toThrow();
  });
});
