// Inputs CodeQL's js/polynomial-redos query flagged: each helper keeps its
// output on ordinary text and stays linear on long adversarial strings (the
// previous patterns took seconds on these sizes).
import { describe, expect, it } from 'vite-plus/test';
import { shouldCollapseUserMessage } from '../src/chat/timeline.logic';
import { commandDisplayText } from '../src/chat/tools/commandLabel';
import { liveThoughtLine } from '../src/chat/tools/presentation';
import { toComposerContextId } from '../src/composer/contextReferences';
import {
  collectComposerInlineTokens,
  SCOPED_PACKAGE_REFERENCE_REGEX,
} from '../src/composer/shared/composerInlineTokens';
import { sanitizeBranchFragment } from '../src/git/shared';
import { trimEndWhere, trimStartWhere, trimWhere } from '../src/lib/trimWhere';
import { isPathWithin } from '../src/thread-list/model';
import { formatWorktreePathForDisplay } from '../src/thread-list/worktreeCleanup';

const LONG = 200_000;
const BUDGET_MS = 1_000;

function timed(run: () => unknown): number {
  const start = performance.now();
  run();
  return performance.now() - start;
}

describe('linear-time text helpers', () => {
  it('shouldCollapseUserMessage counts link labels, not targets', () => {
    expect(shouldCollapseUserMessage(`[short](https://example.com/${'a'.repeat(700)})`)).toBe(
      false,
    );
    expect(shouldCollapseUserMessage(`[short](<https://example.com/${'a'.repeat(700)}>)`)).toBe(
      false,
    );
    expect(shouldCollapseUserMessage('a'.repeat(601))).toBe(true);
    for (const unit of ['[\\', '[](<', '[](!', '[](']) {
      expect(timed(() => shouldCollapseUserMessage(unit.repeat(LONG / unit.length)))).toBeLessThan(
        BUDGET_MS,
      );
    }
  });

  it('liveThoughtLine strips links and stays linear', () => {
    expect(liveThoughtLine('Read [the docs](https://example.com/x) first. Then more.')).toBe(
      'Read the docs first.',
    );
    for (const unit of ['[\\', '[](', '[](!']) {
      expect(timed(() => liveThoughtLine(unit.repeat(LONG / unit.length)))).toBeLessThan(BUDGET_MS);
    }
  });

  it('commandDisplayText unwraps shell -c and stays linear on long flags', () => {
    expect(commandDisplayText("bash -lc 'ls -la'")).toBe('ls -la');
    expect(commandDisplayText("sh -c 'echo hi'")).toBe('echo hi');
    const flag = `-${'c'.repeat(LONG)}!`;
    expect(timed(() => commandDisplayText(`bash ${flag} 'ls'`))).toBeLessThan(BUDGET_MS);
  });

  it('formatWorktreePathForDisplay trims trailing slashes and stays linear', () => {
    expect(formatWorktreePathForDisplay('/home/me/wt///')).toBe('wt');
    expect(formatWorktreePathForDisplay('///')).toBe('///');
    expect(timed(() => formatWorktreePathForDisplay(`${'/'.repeat(LONG)}x`))).toBeLessThan(
      BUDGET_MS,
    );
  });
});

// Issue #1105: the patterns CodeQL flagged before 0.1.1. Each helper is checked
// against its previous regex on short random strings (same output), then timed on a
// long input of the shape CodeQL named.
const REFERENCE = {
  scopedPackage: /^[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._-]*(?:\/[^\s@"]+)*$/,
  sanitizeBranchFragment(raw: string): string {
    const normalized = raw
      .trim()
      .toLowerCase()
      .replace(/['"`]/g, '')
      .replace(/^[./\s_-]+|[./\s_-]+$/g, '');
    const fragment = normalized
      .replace(/[^a-z0-9/_-]+/g, '-')
      .replace(/\/+/g, '/')
      .replace(/-+/g, '-')
      .replace(/^[./_-]+|[./_-]+$/g, '')
      .slice(0, 64)
      .replace(/[./_-]+$/g, '');
    return fragment.length > 0 ? fragment : 'update';
  },
  contextSlug: (id: string) =>
    id
      .replace(/[^a-z0-9_-]+/gi, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 48),
};

/** Deterministic pseudo-random strings over `alphabet` (no flaky seeds). */
function* samples(alphabet: string, count: number, maxLength: number): Generator<string> {
  let state = 0x9e3779b9;
  const next = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return (state >>> 0) / 0x1_0000_0000;
  };
  for (let i = 0; i < count; i += 1) {
    const length = Math.floor(next() * (maxLength + 1));
    let s = '';
    for (let j = 0; j < length; j += 1) s += alphabet[Math.floor(next() * alphabet.length)];
    yield s;
  }
}

describe('linear-time text helpers (issue #1105)', () => {
  it('trimWhere trims only the ends', () => {
    const slash = (c: string) => c === '/';
    expect(trimWhere('//a/b//', slash)).toBe('a/b');
    expect(trimStartWhere('//a/', slash)).toBe('a/');
    expect(trimEndWhere('/a//', slash)).toBe('/a');
    expect(trimWhere('///', slash)).toBe('');
    expect(trimWhere('', slash)).toBe('');
    expect(timed(() => trimWhere(`${'/'.repeat(LONG)}x${'/'.repeat(LONG)}`, slash))).toBeLessThan(
      BUDGET_MS,
    );
  });

  it('the scoped-package test matches what the old pattern did, without backtracking', () => {
    for (const s of samples('a0/!.-_ @"Z', 20_000, 12)) {
      expect(SCOPED_PACKAGE_REFERENCE_REGEX.test(s), s).toBe(REFERENCE.scopedPackage.test(s));
    }
    expect(SCOPED_PACKAGE_REFERENCE_REGEX.test('scope/pkg')).toBe(true);
    expect(SCOPED_PACKAGE_REFERENCE_REGEX.test('scope/pkg/deep//path')).toBe(true);
    expect(SCOPED_PACKAGE_REFERENCE_REGEX.test('Src/pkg')).toBe(false);
    // CodeQL's js/redos witness: `0/0/`, many `!/`, then a character the tail refuses.
    // The old pattern doubles its work every two units here (seconds at ~60 units).
    const witness = `0/0/${'!/'.repeat(LONG / 2)} `;
    expect(timed(() => SCOPED_PACKAGE_REFERENCE_REGEX.test(witness))).toBeLessThan(BUDGET_MS);
    expect(
      timed(() => collectComposerInlineTokens(`@0/0${'/!'.repeat(LONG / 2)} @Src/a.ts `)),
    ).toBeLessThan(BUDGET_MS);
    expect(collectComposerInlineTokens('@scope/pkg/sub @Src/a.ts ').map((t) => t.value)).toEqual([
      'Src/a.ts',
    ]);
  });

  it('sanitizeBranchFragment is unchanged and linear on long separator runs', () => {
    for (const s of samples('aZ0./_- \t"\'`#', 20_000, 16)) {
      expect(sanitizeBranchFragment(s), JSON.stringify(s)).toBe(
        REFERENCE.sanitizeBranchFragment(s),
      );
    }
    expect(sanitizeBranchFragment('  Fix: the Login bug!  ')).toBe('fix-the-login-bug');
    expect(sanitizeBranchFragment('...')).toBe('update');
    for (const unit of ['\t', '.', '-/', '_']) {
      const run = unit.repeat(LONG / unit.length);
      expect(timed(() => sanitizeBranchFragment(`a${run}b`))).toBeLessThan(BUDGET_MS);
      expect(timed(() => sanitizeBranchFragment(`${run}b${run}`))).toBeLessThan(BUDGET_MS);
    }
  });

  it('toComposerContextId keeps its slugs and is linear on long dash runs', () => {
    for (const s of samples('a-Z:_ 0/', 20_000, 16)) {
      const id = toComposerContextId(s);
      if (/^[a-z0-9_-]{1,128}$/i.test(s)) {
        expect(id).toBe(s);
      } else {
        expect(id.startsWith(`${REFERENCE.contextSlug(s) || 'ctx'}-`), s).toBe(true);
      }
    }
    expect(toComposerContextId('pull-request-finding:42')).toMatch(
      /^pull-request-finding-42-[0-9a-f]{16}$/,
    );
    expect(timed(() => toComposerContextId(`a${'-'.repeat(LONG)}:`))).toBeLessThan(BUDGET_MS);
  });

  it('isPathWithin ignores trailing slashes and is linear on long slash runs', () => {
    expect(isPathWithin('/repo/wt/', '/repo//')).toBe(true);
    expect(isPathWithin('C:\\repo\\wt', 'C:/repo/')).toBe(true);
    expect(isPathWithin('/repository', '/repo')).toBe(false);
    expect(isPathWithin('/a', '///')).toBe(false);
    expect(timed(() => isPathWithin(`${'/'.repeat(LONG)}x`, `${'/'.repeat(LONG)}y`))).toBeLessThan(
      BUDGET_MS,
    );
  });
});
