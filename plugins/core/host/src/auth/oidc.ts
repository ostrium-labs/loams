/**
 * The OpenID Connect relying party.
 *
 * PROTOCOL, locked
 *   Authorization Code + PKCE (S256) + state + nonce. All three, always.
 *
 * Authentik never *requires* PKCE, which is exactly why it is not optional here: an
 * admin could turn the provider setting off, and PKCE would then be the only thing still
 * binding the returned code to the request that asked for it. `state` alone protects
 * against CSRF, not against code injection. Belt and braces on purpose.
 *
 * THE v6 TRAPS, all of which are silent or misleading:
 *
 *  1. `expectedState: undefined` ASSERTS that no state is present. It is not "skip the
 *     check" -- it is a positive claim that would reject a correct response. Same for
 *     `expectedNonce: undefined`. Both are therefore passed through from the stored
 *     authorization request, never defaulted.
 *  2. v6 defaults to `ClientSecretPost`. RFC 6749 2.3.1 prefers `client_secret_basic`,
 *     and Authentik is configured for it by default, so `ClientSecretBasic(secret)` is
 *     passed EXPLICITLY rather than relied upon.
 *  3. `redirect_uri` is derived for the token request by stripping searchParams and the
 *     hash, and a bare origin may gain a trailing slash. `validateAuthConfig` therefore
 *     requires a path-only redirect URI with no query string, so the two requests agree.
 *  4. The HTTP timeout defaults to 30s for discovery AND every later request, so an IdP
 *     that accepts a connection and then stalls holds a request handler open for half a
 *     minute. An explicit timeout is set on both.
 *  5. `allowInsecureRequests` is applied only when OIDC_INSECURE=true AND
 *     NODE_ENV !== production. An http:// IdP is a legitimate local setup and a
 *     catastrophic production one.
 *
 * Endpoint URLs are NEVER constructed by hand. `authorize/`, `token/` and `userinfo/`
 * are global to the authentik instance while `jwks/` and `end-session/` are
 * per-application, so there is no way to derive them from the issuer by string
 * manipulation. Discovery is the only supported source.
 */

import {
  ClientSecretBasic,
  allowInsecureRequests,
  enableNonRepudiationChecks,
  authorizationCodeGrant,
  buildAuthorizationUrl,
  buildEndSessionUrl,
  calculatePKCECodeChallenge,
  discovery,
  randomNonce,
  randomPKCECodeVerifier,
  randomState,
  refreshTokenGrant,
  type Configuration,
} from "openid-client";
import type { AuthConfig } from "./config.js";
import { AuthError, classifyOidcError, requestIdFrom } from "./errors.js";
import { algMetadataProblem, probeSigningKeys, signingKeyProblem } from "./jwks.js";

/** What a token endpoint gave us, normalised. */
export interface TokenSet {
  accessToken?: string;
  refreshToken?: string;
  idToken?: string;
  accessTokenExpiresAt?: number;
  /** Verified ID token claims. */
  claims?: Record<string, unknown>;
  /** The `sid` claim, when present. */
  sid?: string;
  /** The `nonce` the ID token was minted with. */
  nonce?: string;
  /** Scopes the provider says it actually granted. */
  grantedScopes: string[];
  /** The provider's own `scope` field, when it sent one. */
  rawScope?: string;
}

/** Everything the authorization-request half of the flow needs. */
export interface AuthorizationRequest {
  state: string;
  nonce: string;
  codeVerifier: string;
  /** Absolute redirect target after a successful login. */
  returnTo?: string;
  createdAt: number;
}

export interface OidcClient {
  /** Run discovery and probe the JWKS. Safe to call more than once. */
  discover(): Promise<void>;
  /** Where to send the browser. */
  authorizationUrl(request: AuthorizationRequest, challenge: string): Promise<string>;
  /** Exchange the callback code. `expectedState`/`expectedNonce` MUST be passed. */
  exchange(params: ExchangeParams): Promise<TokenSet>;
  /** Refresh. `scope` is only ever passed when it contains `offline_access`. */
  refresh(refreshToken: string, idTokenNonce?: string): Promise<TokenSet>;
  /** RP-initiated logout URL, when the provider advertises `end_session_endpoint`. */
  endSessionUrl(idTokenHint?: string): Promise<string | undefined>;
  /** True once discovery has succeeded. */
  readonly ready: boolean;
  /** The provider's `jwks_uri`, once discovered. Needed by back-channel logout. */
  jwksUri(): string | undefined;
}

export interface ExchangeParams {
  currentUrl: URL;
  code: string;
  codeVerifier: string;
  /**
   * MUST be the state that was generated for THIS request.
   *
   * Passing `undefined` here is not a relaxed check: v6 reads it as an assertion that the
   * response carries no state at all, so a legitimate response would be rejected. This
   * parameter is typed as `string` to make omitting it impossible.
   */
  expectedState: string;
  /**
   * MUST be the nonce generated for THIS request, for the same reason as above.
   */
  expectedNonce: string;
}

export function newAuthorizationRequest(returnTo?: string): AuthorizationRequest {
  return {
    state: randomState(),
    nonce: randomNonce(),
    codeVerifier: randomPKCECodeVerifier(),
    returnTo,
    createdAt: Date.now(),
  };
}

export class AuthentikOidcClient implements OidcClient {
  private _configuration?: Configuration;
  private _discovering?: Promise<void>;

  constructor(private readonly config: AuthConfig) {}

  get ready(): boolean {
    return this._configuration !== undefined;
  }

  jwksUri(): string | undefined {
    return this._configuration?.serverMetadata().jwks_uri;
  }

  async discover(): Promise<void> {
    // Concurrent callers share one discovery: a burst of logins must not each open a
    // discovery request.
    if (!this._discovering) {
      this._discovering = this._discoverOnce().catch((err) => {
        // Clear the memo on failure so the NEXT call retries. Caching a rejected promise
        // would turn a momentary IdP outage at boot into a permanently broken login, and
        // the only recovery would be a process restart.
        this._discovering = undefined;
        throw err;
      });
    }
    return this._discovering;
  }

  private async _discoverOnce(): Promise<void> {
    let configuration: Configuration;
    try {
      configuration = await discovery(
        new URL(this.config.issuer),
        this.config.clientId,
        // No `id_token_signed_response_alg` pin: the JWKS probe below catches a missing or
        // symmetric key without refusing a legitimate ES256 deployment.
        undefined,
        this.config.clientSecret
          ? ClientSecretBasic(this.config.clientSecret)
          : // Public client (PKCE only, no secret).
            undefined,
        {
          // openid-client takes SECONDS here, and applies it to every later request too.
          timeout: Math.max(1, Math.ceil(this.config.requestTimeoutMs / 1000)),
          // ORDER MATTERS. `enableNonRepudiationChecks` is what makes the authorization
          // code flow VERIFY the ID token's signature.
          //
          // Without it, oauth4webapi's `validateIdTokenClaims` decodes the ID token and
          // checks its claims (iss, aud, exp, nonce) but never checks its signature -- and
          // the only place the code flow would do so is this opt-in. Signature checking
          // does happen in the JARM/hybrid/implicit flows, which is what makes the
          // omission easy to miss: the tests that exercise those flows pass, and the
          // plain `response_type=code` login silently trusts whatever the token endpoint
          // returned. An ID token signed with a completely unrelated key is accepted.
          //
          // This is verified by tests/auth.spec.ts, which forges a token with a wrong key
          // and asserts it is refused.
          execute: [
            enableNonRepudiationChecks,
            ...(this.config.insecure ? [allowInsecureRequests] : []),
          ],
        },
      );
    } catch (err) {
      throw classifyOidcError(err, { requestId: requestIdFrom(err) });
    }

    this._configuration = configuration;

    const metadata = configuration.serverMetadata();
    const algProblem = algMetadataProblem(
      (metadata as Record<string, unknown>).id_token_signing_alg_values_supported,
    );
    if (algProblem) {
      throw new AuthError(algProblem, { code: "oidc_no_signing_key", httpStatus: 500 });
    }

    // Fail here, with an actionable message, rather than on the first login with
    // "no applicable keys found".
    const probe = await probeSigningKeys(metadata.jwks_uri, this.config.requestTimeoutMs);
    const problem = signingKeyProblem(probe);
    if (problem) throw problem;
  }

  private _require(): Configuration {
    if (!this._configuration) {
      throw new AuthError("OIDC client is not initialised: discovery has not completed.", {
        code: "oidc_not_ready",
        httpStatus: 500,
      });
    }
    return this._configuration;
  }

  async authorizationUrl(request: AuthorizationRequest, challenge: string): Promise<string> {
    const configuration = this._require();
    return buildAuthorizationUrl(configuration, {
      redirect_uri: this.config.redirectUri,
      scope: this.config.scope,
      // S256 only. `plain` is not offered anywhere in this file on purpose.
      code_challenge: challenge,
      code_challenge_method: "S256",
      state: request.state,
      nonce: request.nonce,
    }).href;
  }

  async exchange(params: ExchangeParams): Promise<TokenSet> {
    const configuration = this._require();
    let response;
    try {
      response = await authorizationCodeGrant(
        configuration,
        params.currentUrl,
        {
          pkceCodeVerifier: params.codeVerifier,
          expectedState: params.expectedState,
          expectedNonce: params.expectedNonce,
          idTokenExpected: true,
        },
        { redirect_uri: this.config.redirectUri },
      );
    } catch (err) {
      throw classifyOidcError(err, { requestId: requestIdFrom(err) });
    }
    return this._toTokenSet(response, params.expectedNonce);
  }

  /**
   * Refresh. Never asserts a nonce.
   *
   * Two facts combine here. First, a REFRESHED id token carries the ORIGINAL nonce, so
   * asserting a freshly generated one would reject every legitimate refresh. Second, the
   * rotated id token has already been signature-verified against the IdP's keys by
   * oauth4webapi on the way in. So `expectedNonce` is deliberately absent from the
   * options object -- and that is safe here precisely BECAUSE the stored nonce is
   * replayed into the check position below rather than asserted against a new value.
   */
  async refresh(refreshToken: string, idTokenNonce?: string): Promise<TokenSet> {
    const configuration = this._require();
    let response;
    try {
      response = await refreshTokenGrant(
        configuration,
        refreshToken,
        // `scope` is only ever sent when it contains `offline_access`. Re-sending a scope
        // set without it asks the provider to narrow the grant, and some providers answer
        // by dropping the refresh token from the response entirely.
        this.config.wantsOfflineAccess ? { scope: this.config.scope } : undefined,
      );
    } catch (err) {
      throw classifyOidcError(err, { requestId: requestIdFrom(err) });
    }
    return this._toTokenSet(response, idTokenNonce);
  }

  async endSessionUrl(idTokenHint?: string): Promise<string | undefined> {
    const configuration = this._require();
    const metadata = configuration.serverMetadata();
    if (!metadata.end_session_endpoint) return undefined;
    const params: Record<string, string> = {};
    if (idTokenHint) params.id_token_hint = idTokenHint;
    if (this.config.postLogoutRedirectUri) {
      params.post_logout_redirect_uri = this.config.postLogoutRedirectUri;
      // The provider only honours this when it was registered as a logout-type redirect
      // URI. Sending it anyway is harmless; it is noted in the README because a silently
      // ignored value here is what strands a user on the IdP's own page.
      params.client_id = this.config.clientId;
    }
    return buildEndSessionUrl(configuration, params).href;
  }

  private _toTokenSet(
    response: {
      access_token?: string;
      refresh_token?: string;
      id_token?: string;
      expires_in?: number;
      scope?: string;
      claims(): Record<string, unknown> | undefined;
    },
    fallbackNonce?: string,
  ): TokenSet {
    const claims = response.claims();
    const set: TokenSet = {
      accessToken: response.access_token,
      refreshToken: response.refresh_token,
      idToken: response.id_token,
      grantedScopes: parseGrantedScopes(response.scope, claims),
      rawScope: response.scope,
    };
    if (response.expires_in !== undefined) {
      set.accessTokenExpiresAt = Date.now() + response.expires_in * 1000;
    }
    if (claims) {
      set.claims = claims;
      const sid = claims.sid;
      if (typeof sid === "string") set.sid = sid;
    }
    const nonce = claims?.nonce;
    set.nonce = typeof nonce === "string" ? nonce : fallbackNonce;
    return set;
  }
}

/**
 * Which scopes the provider actually granted.
 *
 * Prefers the token response's `scope`, then the ID token's `scope` claim, then `scp`
 * (the array form Azure-style providers use). `offline_access` is never counted as a
 * usable scope even when present: it authorises refresh, it is not a capability, and
 * leaving it in would let a plugin declare `requiredScopes: ["offline_access"]` and
 * appear satisfied.
 */
function parseGrantedScopes(
  rawScope: string | undefined,
  claims: Record<string, unknown> | undefined,
): string[] {
  const candidates: string[] = [];
  if (typeof rawScope === "string") candidates.push(...rawScope.split(/\s+/));
  const claimScope = claims?.scope;
  if (typeof claimScope === "string") candidates.push(...claimScope.split(/\s+/));
  const scp = claims?.scp;
  if (Array.isArray(scp)) candidates.push(...scp.map((entry) => String(entry)));
  return [...new Set(candidates.filter(Boolean))].filter((scope) => scope !== "offline_access");
}

/** Compute the S256 challenge for a stored authorization request. */
export async function pkceChallenge(codeVerifier: string): Promise<string> {
  return calculatePKCECodeChallenge(codeVerifier);
}
