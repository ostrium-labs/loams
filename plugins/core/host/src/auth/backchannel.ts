/**
 * Back-channel logout, hand-rolled.
 *
 * openid-client v6 ships no back-channel logout implementation: it is not part of the
 * OIDC discovery surface the client models, and its `buildEndSessionUrl` covers only
 * front-channel (RP-initiated) logout. So this is written against `jose` directly.
 *
 * A logout token is a JWT with several REQUIRED claims beyond the usual signature and
 * expiry checks. Each one exists to stop a specific forgery, and each is enforced here
 * rather than assumed:
 *
 *   `events` must contain the logout event URI
 *       Without this, an ordinary ID token (which is a perfectly valid signed JWT from
 *       the same issuer and audience) would be accepted as a logout token and would
 *       destroy live sessions on presentation. This is the most important check here.
 *   `aud` must equal OUR client_id
 *       Otherwise any client on the same IdP can log our users out.
 *   `sub` or `sid` must be present
 *       Otherwise there is nothing to match and the token cannot identify a session.
 *   signature verified against the IdP's `jwks_uri`
 *       An unsigned or foreign-signed token must not be trusted.
 *   `iat` must be recent, and `jti` must be unseen
 *       Replay protection: the same logout token presented twice must not be able to
 *       cause a second, unrelated effect.
 */

import { createRemoteJWKSet, jwtVerify, type JWTPayload } from "jose";
import type { AuthConfig } from "./config.js";
import { AuthError } from "./errors.js";
import type { AuthStore } from "./session.js";

/** OIDC Back-Channel Logout 1.0 event URI. */
export const LOGOUT_EVENT_URI = "http://schemas.openid.net/event/backchannel-logout";

/** How old a logout token's `iat` may be. */
const MAX_LOGOUT_TOKEN_AGE_MS = 5 * 60 * 1000;

export interface LogoutTokenClaims {
  sub?: string;
  sid?: string;
  events?: string[];
  aud?: string | string[];
  jti?: string;
  /** Seconds since the epoch. Used for the freshness window. */
  iat?: number;
}

export interface ValidatedLogoutToken {
  claims: LogoutTokenClaims;
  /** `sid` when present, otherwise resolved from `sub` via the store. */
  sessions: { idHash: string }[];
}

/** Interface for `createRemoteJWKSet`, so tests can inject a fixed key set. */
export type RemoteJwksFactory = (url: URL) => ReturnType<typeof createRemoteJWKSet>;

export interface BackChannelDeps {
  config: AuthConfig;
  store: AuthStore;
  /** Defaults to a real `createRemoteJWKSet` over the discovered jwks_uri. */
  createJwks?: RemoteJwksFactory;
  /** Resolve the provider's `jwks_uri`. Supplied by the service after discovery. */
  jwksUri: () => string | undefined;
  /** Reject a `jti` already seen. Defaults to an in-process set. */
  seenJti?: Set<string>;
}

export class BackChannelLogoutVerifier {
  private readonly _seenJti: Set<string>;
  private readonly _jwks = new Map<string, ReturnType<typeof createRemoteJWKSet>>();

  constructor(private readonly deps: BackChannelDeps) {
    this._seenJti = deps.seenJti ?? new Set<string>();
  }

  /**
   * Validate a `logout_token` and return the sessions it revokes.
   *
   * Throws {@link AuthError} with a specific message for each failure mode. A caller
   * that cannot tell "this token was forged" from "this token was malformed" cannot
   * debug an IdP, so the messages name the claim that failed.
   */
  async validate(logoutToken: string): Promise<ValidatedLogoutToken> {
    const { config } = this.deps;
    const jwksUri = this.deps.jwksUri();
    if (!jwksUri) {
      throw new AuthError(
        "Cannot validate a back-channel logout token: the provider's jwks_uri is not " +
          "known yet. OIDC discovery has not completed.",
        { code: "logout_jwks_unavailable", httpStatus: 500 },
      );
    }

    let payload: JWTPayload;
    try {
      const getKey = this._keySet(jwksUri);
      const verified = await jwtVerify(logoutToken, getKey, {
        issuer: config.issuer,
        audience: config.clientId,
        // `alg` is left to jose, which rejects `none` and keys the algorithm off the
        // JWK set rather than the header -- so an attacker cannot pick the algorithm.
      });
      payload = verified.payload;
    } catch (err) {
      throw new AuthError(
        `Back-channel logout token failed verification: ${
          err instanceof Error ? err.message : String(err)
        }`,
        { code: "logout_token_invalid", httpStatus: 401 },
      );
    }

    const claims = payload as LogoutTokenClaims;

    // --- events: the check that stops a plain ID token being used as a logout token.
    const events = Array.isArray(claims.events) ? claims.events : [];
    if (!events.includes(LOGOUT_EVENT_URI)) {
      throw new AuthError(
        `Back-channel logout token is missing the required "events" claim value ` +
          `"${LOGOUT_EVENT_URI}". Got ${JSON.stringify(claims.events ?? null)}. A token ` +
          `without it is an ordinary ID token, and accepting one here would destroy live ` +
          `sessions on presentation of any valid token from this issuer.`,
        { code: "logout_missing_event", httpStatus: 400 },
      );
    }

    // --- aud: jwtVerify already checked this, but restated because it is load-bearing.
    const audiences = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    if (!audiences.includes(config.clientId)) {
      throw new AuthError(
        `Back-channel logout token audience does not include this client ` +
          `(aud=${JSON.stringify(claims.aud ?? null)}, client_id="${config.clientId}").`,
        { code: "logout_wrong_audience", httpStatus: 400 },
      );
    }

    if (!claims.sub && !claims.sid) {
      throw new AuthError(
        `Back-channel logout token carries neither "sub" nor "sid", so it identifies no ` +
          `session to revoke.`,
        { code: "logout_no_subject", httpStatus: 400 },
      );
    }

    // --- replay: an `iat` in the future, or older than the window, is not fresh.
    const iat = typeof claims.iat === "number" ? claims.iat * 1000 : undefined;
    const now = Date.now();
    if (iat !== undefined) {
      if (iat > now + 60_000) {
        throw new AuthError(
          `Back-channel logout token has an "iat" in the future ` +
            `(${new Date(iat).toISOString()}); the clock or the IdP is wrong.`,
          { code: "logout_token_not_yet_valid", httpStatus: 400 },
        );
      }
      if (now - iat > MAX_LOGOUT_TOKEN_AGE_MS) {
        throw new AuthError(
          `Back-channel logout token is older than the ${MAX_LOGOUT_TOKEN_AGE_MS / 1000}s ` +
            `acceptance window and was rejected as a possible replay.`,
          { code: "logout_token_too_old", httpStatus: 400 },
        );
      }
    }
    if (claims.jti) {
      if (this._seenJti.has(claims.jti)) {
        throw new AuthError(
          `Back-channel logout token "jti" ${claims.jti} has already been used.`,
          { code: "logout_token_replayed", httpStatus: 400 },
        );
      }
      this._seenJti.add(claims.jti);
    }

    // --- resolve to concrete sessions.
    let records: { idHash: string }[];
    if (claims.sid) {
      records = await this.deps.store.listAuthSessionsBySid(claims.sid);
    } else {
      // By the RAW `sub`, never by `sessionKey`: sessionKey is a hash, so comparing a
      // `sub` against it would silently match nothing and the logout would destroy zero
      // sessions while still answering 200.
      records = await this.deps.store.listAuthSessionsBySub(claims.sub as string);
    }

    return { claims, sessions: records };
  }

  /** Destroy every session the token matched. Returns how many went. */
  async revoke(validated: ValidatedLogoutToken): Promise<number> {
    let destroyed = 0;
    for (const session of validated.sessions) {
      await this.deps.store.deleteAuthSession(session.idHash);
      destroyed += 1;
    }
    return destroyed;
  }

  private _keySet(jwksUri: string): ReturnType<typeof createRemoteJWKSet> {
    let set = this._jwks.get(jwksUri);
    if (!set) {
      set = (this.deps.createJwks ?? createRemoteJWKSet)(new URL(jwksUri));
      this._jwks.set(jwksUri, set);
    }
    return set;
  }
}
