/**
 * `ctx.auth` -- the authentication service.
 *
 * One service, three jobs: run the OIDC code flow, hold server-side sessions, and answer
 * "who is this request and what may they do". Everything a route handler needs is on
 * `ctx.auth`, so a plugin author writes
 *
 *   await ctx.auth.requireScope(request, "bi:admin")
 *
 * and nothing else: no session plumbing, no cookie parsing, no store access.
 *
 * EVERY authorization method is async, including the ones that read nothing but a cookie.
 * That is deliberate. Session state lives in the store, so resolving a principal means a
 * read; making the API pretend otherwise would force a second, synchronous path that
 * silently answers "anonymous" on the first request of a session and "user" on the second.
 * A race that produces a wrong authorization decision is far worse than an `await`.
 *
 * REFRESH, and the two Authentik behaviours that dictate its shape
 *
 *  1. `refresh_token_threshold` defaults to 0, which means "always renew". Every refresh
 *     returns a DIFFERENT refresh-token string and the previous one is immediately dead.
 *     A refresh must therefore read-and-replace the stored token set ATOMICALLY. Two
 *     concurrent requests refreshing one session would both present the same token, one
 *     would win and the other would get `invalid_grant` -- an intermittent login loop that
 *     only reproduces under load. `rotateAuthSessionTokens` is a compare-and-swap on the
 *     refresh token, so exactly one request wins and the loser re-reads instead of
 *     clobbering the winner's newer token set.
 *
 *  2. Refresh-token reuse does NOT cascade-revoke. Authentik emits a SUSPICIOUS_REQUEST
 *     event and nothing more, and the successor token stays valid. So an `invalid_grant`
 *     here is read as "this session's token chain is broken": the session row is destroyed
 *     outright and the user is forced to log in again. Watching Authentik's event log for
 *     SUSPICIOUS_REQUEST is the intrusion signal for this.
 *
 * ID tokens are validated by openid-client, never here. This service does not decode a
 * token to make an authorization decision and never hands one to the SPA.
 */

import { Context, Service } from "cordis";
import type { IncomingMessage } from "node:http";
import { loadAuthConfig, validateAuthConfig, type AuthConfig, type Env } from "./config.js";
import { AuthError, ForbiddenError, classifyOidcError, offlineAccessMessage } from "./errors.js";
import {
  AuthentikOidcClient,
  newAuthorizationRequest,
  pkceChallenge,
  type AuthorizationRequest,
  type OidcClient,
  type TokenSet,
} from "./oidc.js";
import {
  MemoryAuthStore,
  clearSessionCookie,
  newSessionId,
  readSessionCookie,
  serializeSessionCookie,
  sessionKey,
  sha256,
  type AuthSessionRecord,
  type AuthStore,
  type AuthUser,
  type ServiceTokenRecord,
} from "./session.js";
import {
  ANONYMOUS,
  evaluateScopeGate,
  principalIsAdmin,
  principalScopes,
  principalUser,
  type AuthPrincipal,
} from "./scopes.js";
import {
  createServiceToken,
  readBearerToken,
  serviceTokenUsable,
  type CreateServiceTokenInput,
  type CreatedServiceToken,
} from "./tokens.js";
import { BackChannelLogoutVerifier, type RemoteJwksFactory } from "./backchannel.js";

declare module "cordis" {
  interface Context {
    auth: AuthService;
  }
}

/** Anything carrying an `IncomingMessage`: a `RouteRequest` or the raw request. */
export type RequestLike = IncomingMessage | { req: IncomingMessage };

/** Refresh this far ahead of real expiry, absorbing clock skew and latency. */
const REFRESH_SKEW_MS = 60_000;

export interface AuthServiceConfig extends Partial<AuthConfig> {
  env?: Env;
  /** Swap the OIDC implementation. Tests use this; production does not. */
  clientFactory?: (config: AuthConfig) => OidcClient;
  /** Swap the store. Defaults to `ctx.store`, or an in-memory one when absent. */
  store?: AuthStore;
  createJwks?: RemoteJwksFactory;
}

export interface LoginResult {
  session: AuthSessionRecord;
  /** The `Set-Cookie` value to write. */
  setCookie: string;
  /** Where to send the browser once the session exists. */
  redirectTo: string;
}

export interface LogoutResult {
  setCookie: string;
  /** Provider logout URL, when one could be built. */
  redirectTo?: string;
  destroyed: number;
}

export interface CreatedSession {
  record: AuthSessionRecord;
  /**
   * The opaque session id, returned ONCE and never stored.
   *
   * `record.idHash` is sha256 of this value. The two are deliberately different types of
   * fact and are never interchangeable: only the plaintext goes in the cookie, only the
   * hash goes in the table.
   */
  cookieValue: string;
}

export class AuthService extends Service {
  static inject = ["router"];

  public config: AuthConfig;

  private readonly _store: AuthStore;
  private readonly _client: OidcClient;
  private readonly _verifier: BackChannelLogoutVerifier;
  private readonly _pending = new Map<string, AuthorizationRequest>();
  /** One principal resolution per request, shared by every helper that asks. */
  private readonly _principalCache = new WeakMap<IncomingMessage, Promise<AuthPrincipal>>();
  private _warnedOfflineAccess = false;
  private _warnedInsecure = false;

  constructor(ctx: Context, options: AuthServiceConfig = {}) {
    super(ctx, "auth");
    const { env, clientFactory, store, createJwks, ...overrides } = options;
    this.config = { ...loadAuthConfig(env ?? process.env), ...stripUndefined(overrides) };
    this._store = store ?? this._storeFromContext() ?? new MemoryAuthStore();
    this._client = (clientFactory ?? ((cfg) => new AuthentikOidcClient(cfg)))(this.config);
    this._verifier = new BackChannelLogoutVerifier({
      config: this.config,
      store: this._store,
      createJwks,
      jwksUri: () => this._client.jwksUri(),
    });
  }

  /** True when the IdP is configured. False means the app runs unauthenticated. */
  get enabled(): boolean {
    return this.config.enabled;
  }

  get client(): OidcClient {
    return this._client;
  }

  get store(): AuthStore {
    return this._store;
  }

  async [Service.init](): Promise<void> {
    if (!this.config.enabled) {
      this.ctx.logger.warn("auth: %s", this.config.disabledReason ?? "not configured.");
      return;
    }

    // Deterministic misconfiguration stops the boot with a legible message, rather than
    // producing a login loop that an operator has to diagnose from the browser.
    validateAuthConfig(this.config);

    if (this.config.insecure && !this._warnedInsecure) {
      this._warnedInsecure = true;
      this.ctx.logger.warn(
        "auth: OIDC_INSECURE is set, so the provider is contacted over plain HTTP. This " +
          "is honoured only because NODE_ENV is not production.",
      );
    }

    try {
      await this._client.discover();
      this.ctx.logger.info("auth: OIDC discovery succeeded for issuer %s", this.config.issuer);
    } catch (err) {
      // Deliberately NOT fatal. An IdP that is unreachable at boot must not stop the
      // server starting; the actionable message is logged here and re-raised on the login
      // route, where it reaches the operator as a legible error rather than a 500.
      this.ctx.logger.error(
        "auth: OIDC discovery failed at startup: %s",
        err instanceof Error ? err.message : String(err),
      );
    }
  }

  /* ---------------------------------------------------------------------- */
  /* The `ctx.auth` authorization surface                                      */
  /* ---------------------------------------------------------------------- */

  /** The signed-in user, or undefined. A service token is not a user. */
  async user(request: RequestLike): Promise<AuthUser | undefined> {
    return principalUser(await this.principal(request));
  }

  /** Scopes granted to this request's principal. Empty for anonymous. */
  async scopes(request: RequestLike): Promise<string[]> {
    return principalScopes(await this.principal(request));
  }

  async hasScope(request: RequestLike, ...scopes: string[]): Promise<boolean> {
    if (scopes.length === 0) return true;
    const held = new Set(await this.scopes(request));
    return scopes.every((scope) => held.has(scope));
  }

  /** Admin is a SCOPE, not a flag, and is false for anonymous by construction. */
  async isAdmin(request: RequestLike): Promise<boolean> {
    const adminScope = this.config.enabled ? this.config.adminScope : undefined;
    return principalIsAdmin(await this.principal(request), adminScope);
  }

  /** The user, or a 401. */
  async requireUser(request: RequestLike): Promise<AuthUser> {
    const user = await this.user(request);
    if (!user) throw unauthenticated("a user session");
    return user;
  }

  /** The principal, or a 401. Accepts a service token as well as a session. */
  async requirePrincipal(request: RequestLike): Promise<AuthPrincipal> {
    const principal = await this.principal(request);
    if (principal.kind === "anonymous") throw unauthenticated("a session or service token");
    return principal;
  }

  /** 403 unless every named scope is held. Names exactly what was missing. */
  async requireScope(request: RequestLike, ...scopes: string[]): Promise<AuthPrincipal> {
    const principal = await this.requirePrincipal(request);
    const verdict = evaluateScopeGate(scopes, principal);
    if (!verdict.ok) {
      throw new ForbiddenError(
        `Missing required scope${scopes.length === 1 ? "" : "s"}: ${verdict.missing.join(", ")}.`,
        { code: "insufficient_scope", missing: verdict.missing },
      );
    }
    return principal;
  }

  /** The admin scope, or undefined while auth is disabled. */
  get adminScope(): string | undefined {
    return this.config.enabled ? this.config.adminScope : undefined;
  }

  /**
   * Resolve the principal for this request, once.
   *
   * A `Authorization: Bearer` header is tried first, then the session cookie. The result
   * is memoised on the request object so `requireUser` followed by a route guard costs one
   * store read rather than two.
   */
  async principal(request: RequestLike): Promise<AuthPrincipal> {
    const req = unwrapRequest(request);
    const cached = this._principalCache.get(req);
    if (cached) return cached;
    const resolved = this._resolvePrincipal(req);
    this._principalCache.set(req, resolved);
    return resolved;
  }

  private async _resolvePrincipal(req: IncomingMessage): Promise<AuthPrincipal> {
    // Disabled auth yields an anonymous principal with NO scopes. That is what stops a
    // dev-mode server from handing out admin powers just because no IdP is configured.
    if (!this.config.enabled) {
      return { ...ANONYMOUS, reason: this.config.disabledReason };
    }

    const bearer = readBearerToken(req.headers.authorization);
    if (bearer) {
      return (await this.principalFromBearer(bearer)) ?? ANONYMOUS;
    }

    const raw = readSessionCookie(req.headers.cookie, this.config.sessionCookieName);
    if (!raw) return ANONYMOUS;

    const idHash = sha256(raw);
    let record: AuthSessionRecord | undefined;
    try {
      record = await this._store.getAuthSession(idHash);
    } catch (err) {
      this.ctx.logger.warn(
        "auth: could not read session: %s",
        err instanceof Error ? err.message : String(err),
      );
      return ANONYMOUS;
    }
    if (!record) return ANONYMOUS;

    if (record.expiresAt <= Date.now()) {
      await this._store.deleteAuthSession(idHash);
      return ANONYMOUS;
    }

    void this._store.touchAuthSession(idHash, Date.now()).catch(() => {});
    return {
      kind: "user",
      sessionIdHash: idHash,
      user: userFromRecord(record),
      scopes: record.scopes,
    };
  }

  /**
   * The session row for a request, refreshed when the access token is near expiry.
   *
   * `undefined` for a bearer caller and for anonymous: a machine token has no session.
   */
  async sessionFor(request: RequestLike): Promise<AuthSessionRecord | undefined> {
    const req = unwrapRequest(request);
    if (readBearerToken(req.headers.authorization)) return undefined;
    const raw = readSessionCookie(req.headers.cookie, this.config.sessionCookieName);
    if (!raw) return undefined;
    const record = await this._store.getAuthSession(sha256(raw));
    if (!record) return undefined;
    // Refresh BEFORE the caller acts on the record, so a caller that then reads
    // `accessToken` gets a live one rather than the expired token that triggered the
    // refresh in the first place.
    return this.ensureFreshSession(record);
  }

  /* ---------------------------------------------------------------------- */
  /* Refresh                                                                 */
  /* ---------------------------------------------------------------------- */

  /**
   * Refresh the access token when it is close to expiry.
   *
   * The compare-and-swap is the interesting part: on losing the race the session is simply
   * re-read, because the winner has already written a newer token set and this request
   * must not overwrite it with a rotation that is now stale.
   */
  async ensureFreshSession(record: AuthSessionRecord): Promise<AuthSessionRecord> {
    const now = Date.now();
    const expiry = record.accessTokenExpiresAt;
    if (expiry !== undefined && expiry - REFRESH_SKEW_MS > now) return record;
    if (!record.refreshToken) {
      this._warnAboutOfflineAccess();
      return record;
    }

    let set: TokenSet;
    try {
      set = await this._client.refresh(record.refreshToken, record.idTokenNonce);
    } catch (err) {
      if (err instanceof AuthError && err.code === "oidc_invalid_grant") {
        await this._destroySessionAfterInvalidGrant(record, err.requestId);
        throw new AuthError(
          "Your session could not be renewed and has been ended. Please sign in again.",
          { code: "session_refresh_failed", httpStatus: 401 },
        );
      }
      throw err instanceof AuthError ? err : classifyOidcError(err);
    }

    const won = await this._store.rotateAuthSessionTokens(record.idHash, record.refreshToken, {
      accessToken: set.accessToken ?? null,
      // A provider that returns no new refresh token must not break the session, so the
      // store coalesces and keeps the current one. Authentik always rotates.
      refreshToken: set.refreshToken ?? null,
      idToken: set.idToken ?? null,
      accessTokenExpiresAt: set.accessTokenExpiresAt ?? null,
    });

    if (!won) {
      const latest = await this._store.getAuthSession(record.idHash);
      return latest ?? record;
    }

    const updated: AuthSessionRecord = {
      ...record,
      accessToken: set.accessToken ?? record.accessToken,
      refreshToken: set.refreshToken ?? record.refreshToken,
      idToken: set.idToken ?? record.idToken,
      accessTokenExpiresAt: set.accessTokenExpiresAt ?? record.accessTokenExpiresAt,
      scopes: set.grantedScopes.length > 0 ? set.grantedScopes : record.scopes,
    };
    return updated;
  }

  /**
   * An `invalid_grant` on refresh means this session's token chain is broken.
   *
   * The row is deleted rather than having its tokens cleared, so no half-valid state is
   * left for a later request to pick up. Other sessions belonging to the same user are
   * deliberately untouched: Authentik does not cascade-revoke on reuse, so they remain
   * valid, and signing a user out of every device would be a larger effect than the
   * evidence supports.
   */
  private async _destroySessionAfterInvalidGrant(
    record: AuthSessionRecord,
    requestId?: string,
  ): Promise<void> {
    this.ctx.logger.warn(
      "auth: refresh returned invalid_grant for session %s (subject %s)%s; destroying the " +
        "session and forcing re-login. Authentik records this as SUSPICIOUS_REQUEST and " +
        "does NOT revoke the successor token, so treat it as an intrusion signal and " +
        "grep its event log for that request_id.",
      record.idHash.slice(0, 12),
      record.sessionKey.slice(0, 12),
      requestId ? ` [authentik request_id: ${requestId}]` : "",
    );
    await this._store.deleteAuthSession(record.idHash);
  }

  private _warnAboutOfflineAccess(): void {
    if (this._warnedOfflineAccess || !this.config.wantsOfflineAccess) return;
    this._warnedOfflineAccess = true;
    this.ctx.logger.warn("auth: %s", offlineAccessMessage());
  }

  /* ---------------------------------------------------------------------- */
  /* Login / logout                                                         */
  /* ---------------------------------------------------------------------- */

  /** Build the authorization URL and stash the state/nonce/PKCE triple. */
  async beginLogin(returnTo?: string): Promise<string> {
    await this._client.discover();
    const request = newAuthorizationRequest(returnTo);
    this._prunePending();
    this._pending.set(request.state, request);
    const challenge = await pkceChallenge(request.codeVerifier);
    return this._client.authorizationUrl(request, challenge);
  }

  /**
   * Finish the code flow: validate the response, exchange the code, create a session.
   *
   * The stored `state` and `nonce` are looked up by the `state` the browser returned and
   * BOTH are then passed to the grant as the expected values. Neither is ever defaulted:
   * in v6, `expectedState: undefined` ASSERTS that no state is present rather than
   * skipping the check, so a "helpful" default of undefined would turn a working login
   * into a rejection.
   */
  async completeLogin(currentUrl: URL): Promise<LoginResult> {
    await this._client.discover();

    const code = currentUrl.searchParams.get("code");
    const state = currentUrl.searchParams.get("state");
    if (!code) {
      throw new AuthError(
        `Authorization response contained no "code" parameter (path ${currentUrl.pathname}).`,
        { code: "oauth_no_code", httpStatus: 400 },
      );
    }
    if (!state) {
      throw new AuthError(
        'Authorization response contained no "state" parameter, so it cannot be matched ' +
          "to a login request.",
        { code: "oauth_no_state", httpStatus: 400 },
      );
    }

    const pending = this._pending.get(state);
    if (!pending) {
      throw new AuthError(
        `No pending login matches state "${state}". It may have expired or already been ` +
          "used. Start again at /api/auth/login.",
        { code: "oauth_unknown_state", httpStatus: 400 },
      );
    }
    // Single use, consumed BEFORE the exchange so a replayed callback cannot reuse it.
    this._pending.delete(state);

    if (Date.now() - pending.createdAt > this.config.authRequestTtlMs) {
      throw new AuthError(
        "This login request expired before it completed. Start again at /api/auth/login.",
        { code: "oauth_state_expired", httpStatus: 400 },
      );
    }

    const set = await this._client.exchange({
      currentUrl,
      code,
      codeVerifier: pending.codeVerifier,
      expectedState: state,
      expectedNonce: pending.nonce,
    });

    if (!set.refreshToken && this.config.wantsOfflineAccess) {
      this.ctx.logger.warn("auth: %s", offlineAccessMessage());
    }

    const { record, cookieValue } = await this.createSession(set);
    return {
      session: record,
      setCookie: this.serializeCookie(cookieValue),
      redirectTo: pending.returnTo ?? this.config.postLogoutRedirectUri ?? "/",
    };
  }

  /** Persist a new session row for a verified token set. */
  async createSession(set: TokenSet): Promise<CreatedSession> {
    const claims = (set.claims ?? {}) as Record<string, unknown>;
    const sub = typeof claims.sub === "string" ? claims.sub : undefined;
    if (!sub) {
      throw new AuthError(
        "The ID token carried no `sub` claim, so there is no subject to key the session on.",
        { code: "oidc_no_sub", httpStatus: 502 },
      );
    }

    const now = Date.now();
    const cookieValue = newSessionId();
    const record: AuthSessionRecord = {
      // sha256 of the cookie value. The value itself is never stored, so a dump of
      // auth_sessions yields nothing a browser would accept.
      idHash: sha256(cookieValue),
      sessionKey: sessionKey({ sub }, this.config.subMode, this.config.installId),
      sub,
      issuer: this.config.issuer,
      sid: set.sid,
      email: typeof claims.email === "string" ? claims.email : undefined,
      name: typeof claims.name === "string" ? claims.name : undefined,
      preferredUsername:
        typeof claims.preferred_username === "string" ? claims.preferred_username : undefined,
      groups: readGroupsClaim(claims.groups),
      // The GRANTED scopes, never the requested ones. Authentik silently intersects an
      // unassigned scope away, so trusting the request would grant capabilities the IdP
      // never actually gave.
      scopes: set.grantedScopes,
      accessToken: set.accessToken,
      refreshToken: set.refreshToken,
      idToken: set.idToken,
      idTokenNonce: set.nonce,
      accessTokenExpiresAt: set.accessTokenExpiresAt,
      createdAt: now,
      lastSeenAt: now,
      expiresAt: now + this.config.sessionTtlMs,
    };
    await this._store.createAuthSession(record);
    return { record, cookieValue };
  }

  /** The `Set-Cookie` value carrying an opaque session id. */
  serializeCookie(value: string): string {
    return serializeSessionCookie(value, {
      name: this.config.sessionCookieName,
      secure: this.config.sessionCookieSecure,
      maxAgeMs: this.config.sessionTtlMs,
    });
  }

  /** The cookie-clearing `Set-Cookie` value. */
  clearCookie(): string {
    return clearSessionCookie(this.config.sessionCookieName, this.config.sessionCookieSecure);
  }

  /** End this browser's session, or every session for the subject. */
  async logout(
    request: RequestLike,
    options: { everywhere?: boolean } = {},
  ): Promise<LogoutResult> {
    const req = unwrapRequest(request);
    const raw = readSessionCookie(req.headers.cookie, this.config.sessionCookieName);
    let destroyed = 0;
    let idTokenHint: string | undefined;

    if (raw) {
      const idHash = sha256(raw);
      const record = await this._store.getAuthSession(idHash);
      if (record) {
        idTokenHint = record.idToken;
        if (options.everywhere && record.sessionKey) {
          const all = await this._store.listAuthSessionsBySessionKey(record.sessionKey);
          for (const session of all) {
            await this._store.deleteAuthSession(session.idHash);
            destroyed += 1;
          }
        } else {
          await this._store.deleteAuthSession(idHash);
          destroyed += 1;
        }
      }
    }

    let redirectTo: string | undefined;
    if (this.config.enabled) {
      try {
        await this._client.discover();
        redirectTo = await this._client.endSessionUrl(idTokenHint);
      } catch (err) {
        this.ctx.logger.warn(
          "auth: could not build a provider logout URL: %s",
          err instanceof Error ? err.message : String(err),
        );
      }
    }

    return { setCookie: this.clearCookie(), redirectTo, destroyed };
  }

  /* ---------------------------------------------------------------------- */
  /* Back-channel logout                                                     */
  /* ---------------------------------------------------------------------- */

  /** Validate a `logout_token` (compact JWT) and destroy every session it names. */
  async handleBackChannelLogout(
    logoutToken: string,
  ): Promise<{ destroyed: number; sid?: string; sub?: string }> {
    const validated = await this._verifier.validate(logoutToken);
    const destroyed = await this._verifier.revoke(validated);
    this.ctx.logger.info(
      "auth: back-channel logout revoked %d session(s) (sid=%s, sub=%s)",
      destroyed,
      validated.claims.sid ?? "-",
      validated.claims.sub ?? "-",
    );
    return { destroyed, sid: validated.claims.sid, sub: validated.claims.sub };
  }

  /* ---------------------------------------------------------------------- */
  /* Service tokens                                                          */
  /* ---------------------------------------------------------------------- */

  /** Mint a service token. The plaintext is returned once and never stored. */
  async issueServiceToken(input: CreateServiceTokenInput): Promise<CreatedServiceToken> {
    const created = createServiceToken(input);
    await this._store.createServiceToken(created.record);
    this.ctx.logger.info(
      "auth: issued service token %s (agent=%s, skills=%s)",
      created.record.name,
      created.record.agentId ?? "*",
      created.record.skills.join(",") || "*",
    );
    return created;
  }

  /** Authenticate a bearer token. Returns undefined for unknown, revoked or expired. */
  async principalFromBearer(plaintext: string): Promise<AuthPrincipal | undefined> {
    let record: ServiceTokenRecord | undefined;
    try {
      record = await this._store.getServiceTokenByHash(sha256(plaintext));
    } catch (err) {
      this.ctx.logger.warn(
        "auth: could not read service token: %s",
        err instanceof Error ? err.message : String(err),
      );
      return undefined;
    }

    const usability = serviceTokenUsable(record, Date.now());
    if (!usability.usable) {
      if (record) {
        this.ctx.logger.warn("auth: service token %s rejected: %s", record.name, usability.reason);
      }
      return undefined;
    }

    const token = record as ServiceTokenRecord;
    await this._store.touchServiceToken(token.id, Date.now());
    return {
      kind: "service-token",
      tokenId: token.id,
      name: token.name,
      agentId: token.agentId,
      skills: token.skills,
      scopes: token.scopes,
    };
  }

  /* ---------------------------------------------------------------------- */
  /* Internals                                                               */
  /* ---------------------------------------------------------------------- */

  private _storeFromContext(): AuthStore | undefined {
    // Read defensively: `ctx.store` is a legal lookup that throws when the key was never
    // provided, and auth must degrade to in-memory rather than refuse to boot.
    try {
      const store = (this.ctx as unknown as { store?: AuthStore }).store;
      if (store && typeof store.createAuthSession === "function") return store;
    } catch {
      /* not provided */
    }
    return undefined;
  }

  private _prunePending(): void {
    const cutoff = Date.now() - this.config.authRequestTtlMs;
    for (const [state, request] of this._pending) {
      if (request.createdAt < cutoff) this._pending.delete(state);
    }
  }
}

/* -------------------------------------------------------------------------- */
/* Helpers                                                                    */
/* -------------------------------------------------------------------------- */

function unauthenticated(what: string): AuthError {
  return new AuthError(`Authentication required: send ${what}. Sign in at /api/auth/login.`, {
    code: "unauthenticated",
    httpStatus: 401,
  });
}

/**
 * Accept either a `RouteRequest` or a bare `IncomingMessage`.
 *
 * Falls back to an empty request rather than throwing: this sits on the path of every
 * authorization check, so a caller that got the shape wrong should get a 401 it can read
 * rather than a 500 from a property access on `undefined`.
 */
function unwrapRequest(request: RequestLike): IncomingMessage {
  const wrapped = (request as { req?: IncomingMessage } | undefined)?.req;
  if (wrapped) return wrapped;
  const bare = request as IncomingMessage | undefined;
  if (bare && typeof bare === "object" && "headers" in bare) return bare;
  return { headers: {} } as unknown as IncomingMessage;
}

/** Build the identity we keep about a user from a session row. */
export function userFromRecord(record: AuthSessionRecord): AuthUser {
  const user: AuthUser = { sub: record.sub ?? record.sessionKey, groups: record.groups ?? [] };
  if (record.email) user.email = record.email;
  if (record.name) user.name = record.name;
  if (record.preferredUsername) user.preferred_username = record.preferredUsername;
  return user;
}

/**
 * Read the `groups` claim.
 *
 * The claim is PLAIN and unnamespaced: `claims.groups`, a `string[]` of group names. It is
 * NOT under a `goauthentik.io/...` namespace -- that namespace does not exist for claims,
 * and reading it returns undefined forever. It carries DIRECT memberships only, so a user
 * who belongs to a nested subgroup does not appear in the parent's list. Use leaf groups,
 * or configure a custom scope mapping and read the claim from there.
 */
export function readGroupsClaim(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string");
}

function stripUndefined<T extends Record<string, unknown>>(value: T): Partial<T> {
  const out: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (entry !== undefined) out[key] = entry;
  }
  return out as Partial<T>;
}
