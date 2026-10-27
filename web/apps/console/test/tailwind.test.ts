import { describe, expect, it } from 'vite-plus/test';
import cordisMain from '../src/cordis/main.tsx?raw';

// The console tsconfig carries no Node types, and vitest returns '' for CSS
// `?raw` imports, so read the two stylesheets through node:fs by a variable
// specifier.
const fsId = 'node:fs';
const { readFileSync } = (await import(/* @vite-ignore */ fsId)) as {
  readFileSync(url: URL, enc: 'utf8'): string;
};
const css = (rel: string) => readFileSync(new URL(rel, import.meta.url), 'utf8');
const tokens = css('../../../packages/ui/src/tokens.css');
const tailwind = css('../src/tailwind.css');

const code = tailwind.replace(/\/\*[\s\S]*?\*\//g, '');

/**
 * The `--op-*` colour tokens: every name declared in the light block. The terminal
 * palette (`--op-term-*`, SD1 Task 22) is left out: xterm reads it from the computed
 * style for its own theme (studio-ui `terminalThemeFromTokens`), and no utility should
 * paint UI with ANSI colours.
 */
const lightBlock = tokens.slice(tokens.indexOf(':root'), tokens.indexOf('.dark'));
const colorTokens = [...lightBlock.matchAll(/^\s*(--op-[a-z-]+):\s*(?:#|rgb\(|oklch\()/gm)]
  .map((m) => m[1])
  .filter((name) => !name?.startsWith('--op-term-') && !name?.startsWith('--op-chart-'));

describe('console tailwind theme (utilities over @loams/ui tokens)', () => {
  it('finds the colour tokens', () => {
    expect(colorTokens.length).toBeGreaterThan(15);
  });

  it.each(colorTokens)('maps %s to a --color-* theme variable', (token) => {
    expect(tailwind).toMatch(new RegExp(`--color-[a-z-]+:\\s*var\\(${token}\\);`));
  });

  it('maps fonts, easing, radii and the type scale to the tokens', () => {
    expect(tailwind).toContain('--font-sans: var(--loams-font-sans);');
    expect(tailwind).toContain('--font-mono: var(--loams-font-mono);');
    expect(tailwind).toContain('--ease-op: var(--op-ease);');
    expect(tailwind).toContain('--ease-panel: var(--op-ease-panel);');
    expect(tailwind).toContain('--radius-none: 0px;');
    for (const step of ['xs', 'sm', 'md', 'lg', 'xl', 'pill']) {
      expect(tailwind).toContain(`--radius-${step}: var(--op-radius-${step});`);
    }
    expect(tailwind).toContain('--radius-panel: var(--op-radius);');
    for (const size of ['micro', 'small', 'body', 'emphasis', 'section', 'title', 'hero']) {
      expect(tailwind).toContain(`--text-${size}: var(--op-text-${size});`);
    }
  });

  it('uses inline theme so dark mode follows the token switch', () => {
    expect(tailwind).toMatch(/@theme inline\s*\{/);
  });

  it('has no preflight and no raw colours', () => {
    expect(code).not.toMatch(/@import\s+["']tailwindcss["']/);
    expect(code).not.toContain('preflight');
    expect(code).not.toMatch(/#[0-9a-f]{3,8}\b/i);
  });

  it('is imported by the console entry', () => {
    expect(cordisMain).toContain("import '../tailwind.css'");
  });
});
