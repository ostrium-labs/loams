// Consistency tokens (design §44 §7.4, D609; runtime contract R4 as amended
// by SDKP §2.1).
//
// A write answers with a token; a read accepts one, so a caller that just
// wrote can read its own write. The encoding is the server's
// (`crates/loams-collection/src/token.rs`):
//
//   v1:s<stream>/p<partition>@<offset>,s<stream>/p<partition>@<offset>,…
//
// Each item is "the next offset after the write" for one `(stream, partition)`.
// Merging two tokens keeps, per pair, the larger offset: a read held to the
// merge waits for both writes. Every number is canonical decimal (no sign, no
// leading zeros), so a token has exactly one spelling, and a malformed token is
// `invalid_argument` here, before any network call.
//
// The session store is off by default. With `sessionConsistency: true` every
// token a response carries is merged into it, and every read whose request has
// a `consistency` field is sent `at_least = session` when the caller left the
// field unset.
//
// Offsets are `bigint`: an offset is a `u64` on the server, and a `number`
// silently rounds above 2^53, which would make a read wait for the wrong
// position.

import { Code } from '@connectrpc/connect';
import { InvalidArgumentError } from './errors.js';

/** The prefix every token carries (§44 §7.4). */
export const TOKEN_PREFIX = 'v1:';

/** Whether a string looks like a consistency token (the prefix only). */
export function isConsistencyToken(value: string): boolean {
  return value.startsWith(TOKEN_PREFIX);
}

/** One item of a token: the next offset after a write, for one partition. */
export interface TokenPosition {
  readonly stream: bigint;
  readonly partition: number;
  readonly offset: bigint;
}

const U32_MAX = 0xffff_ffffn;
const U64_MAX = 0xffff_ffff_ffff_ffffn;

/** A decimal `u64` with no sign and no leading zeros (except `"0"`). */
function canonical(text: string, max: bigint): bigint | undefined {
  if (text === '' || !/^[0-9]+$/.test(text) || (text.length > 1 && text.startsWith('0'))) {
    return undefined;
  }
  const value = BigInt(text);
  return value <= max ? value : undefined;
}

/** `s<stream>/p<partition>@<offset>`. */
function parseItem(item: string): TokenPosition | undefined {
  const match = /^s([^/]*)\/p([^@]*)@(.*)$/.exec(item);
  if (match === null) {
    return undefined;
  }
  const stream = canonical(match[1] as string, U64_MAX);
  const partition = canonical(match[2] as string, U32_MAX);
  const offset = canonical(match[3] as string, U64_MAX);
  if (stream === undefined || partition === undefined || offset === undefined) {
    return undefined;
  }
  return { stream, partition: Number(partition), offset };
}

function compare(a: TokenPosition, b: TokenPosition): number {
  if (a.stream !== b.stream) {
    return a.stream < b.stream ? -1 : 1;
  }
  if (a.partition !== b.partition) {
    return a.partition - b.partition;
  }
  if (a.offset !== b.offset) {
    return a.offset < b.offset ? -1 : 1;
  }
  return 0;
}

/**
 * A parsed consistency token: `parse`, `toString`, `merge`, `equals`.
 *
 * Immutable. `merge` returns a new token, normalised (sorted by stream and
 * partition, one item per pair, the max offset kept), which is exactly what
 * `ConsistencyToken::merge` on the server produces, so the two print the same.
 */
export class ConsistencyToken {
  readonly positions: readonly TokenPosition[];

  private constructor(positions: readonly TokenPosition[]) {
    this.positions = positions;
  }

  /** The token that names nothing: `v1:`. */
  static empty(): ConsistencyToken {
    return new ConsistencyToken([]);
  }

  /** A token from its items, in the order given. */
  static of(positions: readonly TokenPosition[]): ConsistencyToken {
    return new ConsistencyToken([...positions]);
  }

  /** Parses the text form, or throws `InvalidArgumentError`. The items are
   * kept in the order given, as the server's parser keeps them. */
  static parse(text: string): ConsistencyToken {
    const invalid = () =>
      new InvalidArgumentError(`invalid consistency token ${JSON.stringify(text)}`, {
        code: Code.InvalidArgument,
        reason: 'invalid_consistency_token',
        metadata: { field: 'consistency_token' },
      });
    if (!text.startsWith(TOKEN_PREFIX)) {
      throw invalid();
    }
    const body = text.slice(TOKEN_PREFIX.length);
    if (body === '') {
      return ConsistencyToken.empty();
    }
    const positions: TokenPosition[] = [];
    for (const item of body.split(',')) {
      const position = parseItem(item);
      if (position === undefined) {
        throw invalid();
      }
      positions.push(position);
    }
    return new ConsistencyToken(positions);
  }

  /** Whether `text` parses. */
  static isValid(text: string): boolean {
    try {
      ConsistencyToken.parse(text);
      return true;
    } catch {
      return false;
    }
  }

  /** Sorted by `(stream, partition)`, one item per pair, the max offset kept. */
  normalized(): ConsistencyToken {
    const sorted = [...this.positions].sort(compare);
    const items: TokenPosition[] = [];
    for (const item of sorted) {
      const last = items[items.length - 1];
      if (last !== undefined && last.stream === item.stream && last.partition === item.partition) {
        items[items.length - 1] = item;
      } else {
        items.push(item);
      }
    }
    return new ConsistencyToken(items);
  }

  /** Both tokens' offsets; a pair both name keeps the larger. Normalised. */
  merge(other: ConsistencyToken | string): ConsistencyToken {
    const right = typeof other === 'string' ? ConsistencyToken.parse(other) : other;
    return new ConsistencyToken([...this.positions, ...right.positions]).normalized();
  }

  /** The offset for `(stream, partition)`, the max if listed twice. */
  offset(stream: bigint | number, partition: number): bigint | undefined {
    const s = BigInt(stream);
    let found: bigint | undefined;
    for (const item of this.positions) {
      if (item.stream === s && item.partition === partition) {
        found = found === undefined || item.offset > found ? item.offset : found;
      }
    }
    return found;
  }

  /** Whether the two tokens name the same positions (after normalising). */
  equals(other: ConsistencyToken | string): boolean {
    const right = typeof other === 'string' ? ConsistencyToken.parse(other) : other;
    return this.normalized().toString() === right.normalized().toString();
  }

  /** Whether the token names nothing. */
  get isEmpty(): boolean {
    return this.positions.length === 0;
  }

  /** The text form, `v1:` plus the items joined by `,`, in this token's order. */
  toString(): string {
    return (
      TOKEN_PREFIX +
      this.positions
        .map((item) => `s${item.stream}/p${item.partition}@${item.offset}`)
        .join(',')
    );
  }

  toJSON(): string {
    return this.toString();
  }
}

/**
 * A session's consistency tokens: every token a response carried, merged, kept
 * **per proto package** (R4, SDKP-17).
 *
 * A Graph read refuses a token naming another package's stream, so a
 * collection write's token must never become a graph read's `at_least`: the
 * session holds one merged token per scope, and the call path passes the
 * call's package as the scope. A caller using the store directly may leave the
 * scope out, which is one shared scope.
 *
 * `record` folds a token in; `current` is the scope's merge, as text. A token
 * that does not parse is counted in `rejected` and otherwise ignored: the write
 * that answered with it has already happened, and turning a committed write
 * into a thrown error invites a caller to retry it.
 */
export class ConsistencySession {
  readonly #tokens = new Map<string, ConsistencyToken>();
  #rejected = 0;

  /** The token to attach to the next read in `scope`, or undefined. */
  current(scope = ''): string | undefined {
    return this.#tokens.get(scope)?.toString();
  }

  /** The merged token of `scope`, parsed. */
  token(scope = ''): ConsistencyToken | undefined {
    return this.#tokens.get(scope);
  }

  /** The scopes the session holds a token for. */
  scopes(): string[] {
    return [...this.#tokens.keys()];
  }

  /** Folds a token the server returned, in `scope`, in. Throws
   * `InvalidArgumentError` on a malformed token, after counting it. */
  record(token: string | ConsistencyToken | undefined, scope = ''): void {
    if (token === undefined || token === '') {
      return;
    }
    let parsed: ConsistencyToken;
    try {
      parsed = typeof token === 'string' ? ConsistencyToken.parse(token) : token;
    } catch (error) {
      this.#rejected += 1;
      throw error;
    }
    const held = this.#tokens.get(scope);
    this.#tokens.set(scope, held === undefined ? parsed.normalized() : held.merge(parsed));
  }

  /** How many tokens the session could not parse. */
  get rejected(): number {
    return this.#rejected;
  }

  /** @deprecated The old name of `rejected`, from before tokens merged. */
  get conflicts(): number {
    return this.#rejected;
  }

  /** Forgets every token, so the next read is not held to one. */
  clear(): void {
    this.#tokens.clear();
    this.#rejected = 0;
  }
}
