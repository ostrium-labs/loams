// Semantic versions (semver.org 2.0.0) for ordering a listing's versions, and
// the version-range syntax listings and revocations use: Cargo's VersionReq
// (comma-separated comparators such as `^2.0.0`, `>=1.0.0, <1.2.0`, `=1.0.3`
// or `*`), so agentd parses it with the `semver` crate unchanged.

const SEMVER =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

export interface Version {
  major: number;
  minor: number;
  patch: number;
  pre: string[];
}

export function parse(version: string): Version | undefined {
  const m = SEMVER.exec(version);
  if (!m) return undefined;
  return { major: Number(m[1]), minor: Number(m[2]), patch: Number(m[3]), pre: m[4] ? m[4].split('.') : [] };
}

function compareIdentifiers(a: string, b: string): number {
  const an = /^\d+$/.test(a);
  const bn = /^\d+$/.test(b);
  if (an && bn) return Number(a) - Number(b);
  if (an) return -1;
  if (bn) return 1;
  return a < b ? -1 : a > b ? 1 : 0;
}

/** Orders by precedence; build metadata is ignored, so ties fall back to the strings. */
export function compare(a: string, b: string): number {
  const x = parse(a);
  const y = parse(b);
  if (!x || !y) throw new Error(`not a semantic version: ${!x ? a : b}`);
  for (const key of ['major', 'minor', 'patch'] as const) {
    if (x[key] !== y[key]) return x[key] - y[key];
  }
  if (x.pre.length === 0 || y.pre.length === 0) {
    const pre = (y.pre.length === 0 ? 0 : 1) - (x.pre.length === 0 ? 0 : 1);
    if (pre !== 0) return pre;
  } else {
    for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
      const xi = x.pre[i];
      const yi = y.pre[i];
      if (xi === undefined) return -1;
      if (yi === undefined) return 1;
      const c = compareIdentifiers(xi, yi);
      if (c !== 0) return c;
    }
  }
  return a < b ? -1 : a > b ? 1 : 0;
}

export function isPrerelease(version: string): boolean {
  return (parse(version)?.pre.length ?? 0) > 0;
}

const COMPARATOR = /^(?:\*|(?:[=<>~^]|<=|>=)?\d+(?:\.(?:\d+|\*))?(?:\.(?:\d+|\*))?(?:-[0-9A-Za-z.-]+)?)$/;

/** The version D1791 pins for Bun; a Bun listing's range must admit it (D1808). */
export const BUN_PINNED = '1.4.4';

/** Whether `range` is a Cargo VersionReq. */
export function isRange(range: string): boolean {
  const parts = range.split(',').map((part) => part.trim());
  return parts.length > 0 && parts.every((part) => COMPARATOR.test(part.replace(/\s+/g, '')));
}

const triple = (v: Version): [number, number, number] => [v.major, v.minor, v.patch];

function cmp(a: [number, number, number], b: [number, number, number]): number {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i]! - b[i]!;
  return 0;
}

/** One comparator, with Cargo's semantics for partial versions (`>=22.13` is `>=22.13.0`, `^1.4` is `>=1.4.0, <2.0.0`). */
function matchesComparator(part: string, v: [number, number, number]): boolean {
  if (part === '*') return true;
  const m = /^(<=|>=|[=<>~^])?(\d+)(?:\.(\d+|\*))?(?:\.(\d+|\*))?/.exec(part);
  if (!m) return false;
  const op = m[1] ?? '^';
  const major = Number(m[2]);
  const minor = m[3] === undefined || m[3] === '*' ? undefined : Number(m[3]);
  const patch = m[4] === undefined || m[4] === '*' ? undefined : Number(m[4]);
  const low: [number, number, number] = [major, minor ?? 0, patch ?? 0];
  // The first version above the partial version (`22.13` → `22.14.0`).
  const next: [number, number, number] =
    minor === undefined ? [major + 1, 0, 0] : patch === undefined ? [major, minor + 1, 0] : [major, minor, patch + 1];
  switch (op) {
    case '>=':
      return cmp(v, low) >= 0;
    case '>':
      return cmp(v, next) >= 0;
    case '<':
      return cmp(v, low) < 0;
    case '<=':
      return cmp(v, next) < 0;
    case '=':
      return cmp(v, low) >= 0 && cmp(v, next) < 0;
    case '~':
      return cmp(v, low) >= 0 && cmp(v, minor === undefined ? [major + 1, 0, 0] : [major, minor + 1, 0]) < 0;
    default: {
      // Caret: the left-most non-zero component may not change.
      const upper: [number, number, number] =
        major > 0 || minor === undefined
          ? [major + 1, 0, 0]
          : minor > 0 || patch === undefined
            ? [0, minor + 1, 0]
            : [0, 0, patch + 1];
      return cmp(v, low) >= 0 && cmp(v, upper) < 0;
    }
  }
}

/** Whether `version` (a release, not a prerelease) is in the Cargo VersionReq `range`. */
export function satisfies(range: string, version: string): boolean {
  const v = parse(version);
  if (!v || !isRange(range)) return false;
  return range
    .split(',')
    .map((part) => part.replace(/\s+/g, ''))
    .every((part) => matchesComparator(part, triple(v)));
}
