/**
 * Server-side sessions: the record, the cookie, and the hashing.
 *
 * WHY SERVER-SIDE AND NOT A JWT IN THE COOKIE
 * The obvious design -- seal the session into an encrypted cookie -- is wrong here for
 * one specific reason: it cannot be revoked server-side. The plugin console's whole job
 * is turning plugins off, and "disable this plugin now" has to take effect on the next
 * request. An encrypted cookie is valid until it expires no matter what the server
 * thinks, so a session could keep reaching a plugin that was disabled, or an account
 * could keep admin after its scopes were withdrawn. So the cookie carries nothing but a
 * random opaque id and every decision is a database read.
 *
 * THE COOKIE FLAGS, and why each one is the way it is:
 *   HttpOnly     - the SPA never needs to read it; XSS must not be able to exfiltrate it.
 *   Secure       - sent over TLS only.
 *   SameSite=Lax NOT Strict. The code flow's callback is a TOP-LEVEL cross-site GET
 *                 navigation. SameSite=Strict would withhold the cookie on exactly that
 *                 navigation and the login would silently fail to establish a session.
 *                 Lax still blocks the CSRF-vulnerable cross-site POST, which is what the
 *                 flag is actually for.
 *   Path=/       - the whole app.
 */

import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { parse as parseCookie, serialize as serializeCookie } from "cookie";

/** How a session's subject is identified. See `sessionKey()`. */
export type SubMode = "hashed_user_id" | "sub";

/** The identity we keep about a signed-in user. */
export interface AuthUser {
  sub: string;
  email?: string;
  name?: string;
  preferred_username?: string;
  /**
   * Group names from the `groups` claim.
   *
   * The claim is PLAIN and unnamespaced -- `claims.groups`, a `string[]` of names. There
   * is no `goauthentik.io/...` namespace for claims; reading one returns undefined
   * forever. It also contains DIRECT memberships only, so a user in a nested subgroup
   * does not appear in its parent's group list. Use leaf groups.
   */
  groups: string[];
}

/** One row of `auth_sessions`. Tokens live here and are never sent to the SPA. */
export interface AuthSessionRecord {
  /** sha256 of the cookie value. The cookie value itself is never stored. */
  idHash: string;
  /**
   * Stable key for "this subject", used to find every session a user holds.
   * `hashed_user_id` by default: sha256(installId + sub), which is global and
   * app-independent in Authentik, so the same person is one key across applications.
   */
  sessionKey: string;
  /**
   * The IdP's `sub` claim, verbatim.
   *
   * Distinct from `sessionKey`, which is the HASHED form used for lookups. Plugins are
   * handed this one, so a plugin that needs to correlate with another system sees the
   * real subject rather than an opaque digest.
   */
  sub: string;
  issuer?: string;
  /** The OIDC `sid`, used by back-channel logout to target the right session. */
  sid?: string;
  email?: string;
  name?: string;
  preferredUsername?: string;
  groups: string[];
  scopes: string[];
  accessToken?: string;
  refreshToken?: string;
  idToken?: string;
  /**
   * The `nonce` carried by the stored ID token.
   *
   * A REFRESHED id token repeats the ORIGINAL nonce rather than minting a new one, so
   * re-validating a refreshed token against a freshly generated nonce would reject every
   * legitimate refresh. Storing the original is what makes the value checkable at all.
   */
  idTokenNonce?: string;
  accessTokenExpiresAt?: number;
  createdAt: number;
  lastSeenAt?: number;
  expiresAt: number;
}

/** One row of `service_tokens`. */
export interface ServiceTokenRecord {
  id: string;
  /** sha256 of the bearer value. */
  tokenHash: string;
  name: string;
  /** The agent this token may act as, if any. */
  agentId?: string;
  /** Skill ids this token may invoke. Narrower than `scopes` for A2A. */
  skills: string[];
  scopes: string[];
  createdAt: number;
  lastUsedAt?: number;
  expiresAt?: number;
  revokedAt?: number;
}

/**
 * The slice of the store auth needs.
 *
 * Structural on purpose: it is exactly `StoreService`'s auth methods, but naming it
 * structurally lets the auth service run against an in-memory implementation in tests
 * and against no store at all in a bare context.
 */
export interface AuthStore {
  createAuthSession(record: AuthSessionRecord): Promise<void>;
  getAuthSession(idHash: string): Promise<AuthSessionRecord | undefined>;
  rotateAuthSessionTokens(
    idHash: string,
    expectedRefreshToken: string | null,
    next: {
      accessToken?: string | null;
      refreshToken?: string | null;
      idToken?: string | null;
      accessTokenExpiresAt?: number | null;
    },
  ): Promise<boolean>;
  touchAuthSession(idHash: string, lastSeenAt: number): Promise<void>;
  deleteAuthSession(idHash: string): Promise<void>;
  listAuthSessionsBySessionKey(sessionKey: string): Promise<AuthSessionRecord[]>;
  /**
   * Sessions for the RAW `sub` claim.
   *
   * Distinct from `listAuthSessionsBySessionKey`, and both are needed. A back-channel
   * logout token may carry only `sub`, never a `sid`, and `sessionKey` is the HASHED form
   * (sha256 of installId + sub), so looking a `sub` up by session key silently matches
   * nothing and the logout destroys zero sessions. That failure is invisible -- the
   * endpoint answers 200 -- which is why the raw claim is stored and indexed too.
   */
  listAuthSessionsBySub(sub: string): Promise<AuthSessionRecord[]>;
  listAuthSessionsBySid(sid: string): Promise<AuthSessionRecord[]>;
  pruneAuthSessions(now: number): Promise<number>;
  createServiceToken(record: ServiceTokenRecord): Promise<void>;
  getServiceTokenByHash(tokenHash: string): Promise<ServiceTokenRecord | undefined>;
  touchServiceToken(id: string, lastUsedAt: number): Promise<void>;
  revokeServiceToken(id: string): Promise<void>;
  listServiceTokens(): Promise<ServiceTokenRecord[]>;
}

/** In-memory `AuthStore`, used when no store plugin is loaded and by tests. */
export class MemoryAuthStore implements AuthStore {
  private readonly _sessions = new Map<string, AuthSessionRecord>();
  private readonly _tokens = new Map<string, ServiceTokenRecord>();
  /**
   * token hash -> id.
   *
   * A SECOND index, because the two lookups use different keys: `revokeServiceToken` and
   * `listServiceTokens` address a token by its id, while `getServiceTokenByHash` addresses
   * it by the hash of the bearer value. Keying the single map by id makes the hash lookup
   * silently return nothing -- which reads as "unknown token" and rejects every valid
   * service-token caller.
   */
  private readonly _tokenIdsByHash = new Map<string, string>();

  async createAuthSession(record: AuthSessionRecord): Promise<void> {
    this._sessions.set(record.idHash, { ...record });
  }
  async getAuthSession(idHash: string) {
    const row = this._sessions.get(idHash);
    return row ? { ...row } : undefined;
  }
  async rotateAuthSessionTokens(
    idHash: string,
    expectedRefreshToken: string | null,
    next: {
      accessToken?: string | null;
      refreshToken?: string | null;
      idToken?: string | null;
      accessTokenExpiresAt?: number | null;
    },
  ) {
    const row = this._sessions.get(idHash);
    if (!row) return false;
    // The compare-and-swap. Two concurrent refreshes must not both write.
    if ((row.refreshToken ?? null) !== (expectedRefreshToken ?? null)) return false;
    // `null` from a caller means "not supplied, keep what is stored", which matches the
    // SQL COALESCE the real store does. Normalised to undefined so the row's optional
    // fields stay genuinely optional.
    if (next.accessToken !== undefined) row.accessToken = next.accessToken ?? undefined;
    if (next.refreshToken !== undefined) row.refreshToken = next.refreshToken ?? undefined;
    if (next.idToken !== undefined) row.idToken = next.idToken ?? undefined;
    if (next.accessTokenExpiresAt !== undefined) {
      row.accessTokenExpiresAt = next.accessTokenExpiresAt ?? undefined;
    }
    row.lastSeenAt = Date.now();
    return true;
  }
  async touchAuthSession(idHash: string, lastSeenAt: number) {
    const row = this._sessions.get(idHash);
    if (row) this._sessions.set(idHash, { ...row, lastSeenAt });
  }
  async deleteAuthSession(idHash: string) {
    this._sessions.delete(idHash);
  }
  async listAuthSessionsBySessionKey(sessionKey: string) {
    return [...this._sessions.values()]
      .filter((row) => row.sessionKey === sessionKey)
      .map((row) => ({ ...row }));
  }
  async listAuthSessionsBySub(sub: string) {
    return [...this._sessions.values()].filter((row) => row.sub === sub).map((row) => ({ ...row }));
  }
  async listAuthSessionsBySid(sid: string) {
    return [...this._sessions.values()].filter((row) => row.sid === sid).map((row) => ({ ...row }));
  }
  async pruneAuthSessions(now: number) {
    let removed = 0;
    for (const [key, row] of [...this._sessions]) {
      if (row.expiresAt <= now) {
        this._sessions.delete(key);
        removed += 1;
      }
    }
    return removed;
  }
  async createServiceToken(record: ServiceTokenRecord) {
    this._tokens.set(record.id, { ...record });
    this._tokenIdsByHash.set(record.tokenHash, record.id);
  }
  async getServiceTokenByHash(tokenHash: string) {
    const id = this._tokenIdsByHash.get(tokenHash);
    const row = id === undefined ? undefined : this._tokens.get(id);
    return row ? { ...row } : undefined;
  }
  async touchServiceToken(id: string, lastUsedAt: number) {
    const row = this._tokens.get(id);
    if (row) this._tokens.set(id, { ...row, lastUsedAt });
  }
  async revokeServiceToken(id: string) {
    const row = this._tokens.get(id);
    if (row) this._tokens.set(id, { ...row, revokedAt: Date.now() });
  }
  async listServiceTokens() {
    return [...this._tokens.values()].map((row) => ({ ...row }));
  }

  /** Test helper: how many sessions currently exist. */
  get sessionCount(): number {
    return this._sessions.size;
  }

  /**
   * Every session row. Back-channel logout tests need to enumerate sessions whose
   * `sessionKey` they do not know, and neither `getAuthSession` nor
   * `listAuthSessionsBySessionKey` can do that.
   */
  listSessions(): AuthSessionRecord[] {
    return [...this._sessions.values()].map((row) => ({ ...row }));
  }
}

/* -------------------------------------------------------------------------- */
/* Identifiers                                                                */
/* -------------------------------------------------------------------------- */

/** 256 bits of CSPRNG output, URL-safe. The value that goes in the cookie. */
export function newSessionId(): string {
  return randomBytes(32).toString("base64url");
}

/**
 * The key a session row is stored under.
 *
 * `hashed_user_id` (the default) mirrors Authentik's own `sub_mode=hashed_user_id`:
 * sha256(userId + installId), which is global and application-independent, so one person
 * is one key no matter which application they signed in through. `installId` is what
 * keeps two Bi installations sharing one database apart.
 */
export function sessionKey(claims: { sub: string }, mode: SubMode, installId: string): string {
  if (mode === "sub") return claims.sub;
  return sha256(`${installId}:${claims.sub}`);
}

export function sha256(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

/** Constant-time comparison for secrets, so a token check cannot be timed. */
export function safeEqual(a: string, b: string): boolean {
  const left = Buffer.from(a);
  const right = Buffer.from(b);
  if (left.length !== right.length) return false;
  return timingSafeEqual(left, right);
}

/* -------------------------------------------------------------------------- */
/* Cookies                                                                    */
/* -------------------------------------------------------------------------- */

export interface SessionCookieOptions {
  name: string;
  /** Default true. Overridable only for plain-http localhost development. */
  secure: boolean;
  maxAgeMs: number;
}

export function serializeSessionCookie(value: string, options: SessionCookieOptions): string {
  return serializeCookie(options.name, value, {
    httpOnly: true,
    secure: options.secure,
    // Lax, NOT Strict: the callback is a top-level cross-site GET and Strict would
    // withhold the cookie on precisely that navigation, so login would appear to
    // succeed and then leave the browser signed out.
    sameSite: "lax",
    path: "/",
    maxAge: Math.floor(options.maxAgeMs / 1000),
  });
}

export function readSessionCookie(header: string | undefined, name: string): string | undefined {
  if (!header) return undefined;
  const parsed = parseCookie(header);
  return parsed[name];
}

/** Clear the cookie. Attributes must match the ones it was set with, or it survives. */
export function clearSessionCookie(name: string, secure: boolean): string {
  return serializeCookie(name, "", {
    httpOnly: true,
    secure,
    sameSite: "lax",
    path: "/",
    maxAge: 0,
    expires: new Date(0),
  });
}
