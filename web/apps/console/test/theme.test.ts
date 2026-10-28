import { afterEach, describe, expect, it } from 'vite-plus/test';
import { applySavedTheme, initialChoice } from '../src/cordis/theme.js';

afterEach(() => {
  localStorage.clear();
  delete (globalThis as { loamsDesktop?: unknown }).loamsDesktop;
});

describe('the theme before first paint', () => {
  it('is dark with no saved choice and no OS preference (jsdom has no matchMedia)', () => {
    const root = document.createElement('html');
    applySavedTheme(root);
    expect(root.classList.contains('dark')).toBe(true);
  });

  it('keeps a saved Light', () => {
    localStorage.setItem('loams-theme', 'light');
    const root = document.createElement('html');
    applySavedTheme(root);
    expect(root.classList.contains('dark')).toBe(false);
  });

  it('prefers the desktop setting the preload passed', () => {
    localStorage.setItem('loams-theme', 'dark');
    (globalThis as { loamsDesktop?: unknown }).loamsDesktop = { theme: { initial: 'light' } };
    expect(initialChoice()).toBe('light');
  });

  it('turns panel motion on only when it was chosen', () => {
    const root = document.createElement('html');
    applySavedTheme(root);
    expect(root.dataset.motion).toBeUndefined();
    localStorage.setItem('loams.shell.motion', 'on');
    applySavedTheme(root);
    expect(root.dataset.motion).toBe('on');
  });
});
