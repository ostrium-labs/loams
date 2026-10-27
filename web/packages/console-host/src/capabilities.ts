// What the engine behind the console can do (design §54 §13.1, D938). The
// hosted console always runs the newest console, so it must work with the
// newest engine (N) and the release before it (N−1): it reads GetInstance
// into capabilities, hides what the engine does not serve, and blocks an
// engine older than N−1 (or one without GetInstance) with an upgrade page.

import type { PluginManifest } from './manifest.js';
import type { FlagsService } from './services.js';

/**
 * The oldest engine this console supports: N−1, the `from` of the newest
 * release in `release/upgrade-path.toml` (a test keeps them equal). A patch
 * release's `from` is its minor line (`0.1`): every release of the line is
 * supported, so N−1 is the line's first release (`0.1.0`).
 */
export const MIN_ENGINE = '0.1.0';

/** The command the upgrade page tells an operator to run. */
export const UPGRADE_COMMAND = 'loams upgrade --to latest';

type Parsed = { core: [number, number, number]; pre: (number | string)[] };

function parse(version: string): Parsed | undefined {
  const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(
    version.trim(),
  );
  if (!m) return undefined;
  const pre = m[4] ? m[4].split('.').map((p) => (/^\d+$/.test(p) ? Number(p) : p)) : [];
  return { core: [Number(m[1]), Number(m[2]), Number(m[3])], pre };
}

/**
 * Semver order of `a` and `b` (pre-releases before their release, numeric
 * identifiers numerically): negative, zero or positive; `undefined` when
 * either does not parse.
 */
export function compareVersions(a: string, b: string): number | undefined {
  const x = parse(a);
  const y = parse(b);
  if (!x || !y) return undefined;
  for (let i = 0; i < 3; i++) {
    const d = (x.core[i] ?? 0) - (y.core[i] ?? 0);
    if (d !== 0) return d;
  }
  if (x.pre.length === 0 || y.pre.length === 0) return y.pre.length - x.pre.length;
  for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
    const p = x.pre[i];
    const q = y.pre[i];
    if (p === undefined) return -1;
    if (q === undefined) return 1;
    if (p === q) continue;
    if (typeof p === 'number' && typeof q === 'number') return p - q;
    if (typeof p === 'number') return -1;
    if (typeof q === 'number') return 1;
    return p < q ? -1 : 1;
  }
  return 0;
}

export interface Capabilities {
  /** The engine's version (`GetInstance.server_version`), or '' when unknown. */
  version: string;
  /** False when the engine is older than N−1 or has no GetInstance. */
  supported: boolean;
  /** Why it is not supported. */
  reason?: 'no-instance' | 'too-old';
  /** The oldest engine the console supports. */
  minEngine: string;
  /** Whether the engine serves `api` (a `loams.<area>.v<N>` package), at `minVersion` or later. */
  has(api: string, minVersion?: string): boolean;
}

/** GetInstance's answer as capabilities. */
export function capabilities(
  flags: FlagsService | undefined,
  minEngine: string = MIN_ENGINE,
): Capabilities {
  const version = flags?.serverVersion ?? '';
  const atLeast = (min: string) => {
    const order = compareVersions(version, min);
    // A version that does not parse (a local build) is not refused.
    return order === undefined || order >= 0;
  };
  let reason: Capabilities['reason'];
  if (!flags || flags.instanceError === 'unimplemented') reason = 'no-instance';
  else if (version && !atLeast(minEngine)) reason = 'too-old';
  return {
    version,
    supported: reason === undefined,
    reason,
    minEngine,
    has: (api, minVersion) =>
      reason === undefined &&
      flags?.has(api) === true &&
      (minVersion === undefined || atLeast(minVersion)),
  };
}

/**
 * What a plugin's `requires` asks of the engine that it does not have, as a
 * reason; `undefined` when every requirement is met.
 */
export function unmet(
  requires: PluginManifest['requires'],
  caps: Capabilities,
): string | undefined {
  if (requires.engine !== undefined) {
    const order = compareVersions(caps.version, requires.engine);
    if (order !== undefined && order < 0) {
      return `needs engine ${requires.engine} or later (the engine is ${caps.version})`;
    }
  }
  const missing = requires.api.filter((api) => !caps.has(api, requires.engine));
  if (missing.length > 0) return `the engine does not serve ${missing.join(', ')}`;
  return undefined;
}
