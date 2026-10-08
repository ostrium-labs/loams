/**
 * Errors that must never reach a client as a bare 500.
 *
 * Almost everything that goes wrong with an Authentik integration is a
 * CONFIGURATION mistake, and the failure modes are all silent on authentik's
 * side: no signing key means it signs HS256 without complaining, an unassigned
 * `offline_access` scope is intersected away, and a `redirect_uri` mismatch is
 * not even an error until the browser round-trips. openid-client's own message
 * for these is accurate but cryptic ("no applicable keys found"), which is the
 * worst possible thing to hand somebody who is trying to stand up an IdP.
 *
 * So every one of those is translated here into a message that names the Authentik
 * setting to change. `AuthConfigError` carries a stable `code` so tests and logs
 * can assert on the diagnosis rather than on prose.
 */

/** HTTP status for an authorization failure. The router honours this. */
export class AuthError extends Error {
  readonly httpStatus: number;
  readonly code: string;
  /**
   * authentik's `request_id` from the failed response.
   *
   * Kept as structured data rather than only being interpolated into the message: it is
   * the only handle an operator has on a token-endpoint failure, and grepping the event
   * log for it is how an `invalid_grant` is traced to a real request.
   */
  readonly requestId?: string;

  constructor(
    message: string,
    options: { httpStatus?: number; code?: string; requestId?: string } = {},
  ) {
    super(message);
    this.name = "AuthError";
    this.httpStatus = options.httpStatus ?? 401;
    this.code = options.code ?? "unauthorized";
    if (options.requestId !== undefined) this.requestId = options.requestId;
  }
}

/** 403: authenticated, but the principal lacks the scope. */
export class ForbiddenError extends AuthError {
  constructor(message: string, options: { code?: string; missing?: string[] } = {}) {
    super(message, { httpStatus: 403, code: options.code ?? "forbidden" });
    this.name = "ForbiddenError";
    this.missing = options.missing ?? [];
  }

  /** The scopes that were required and not held. Named so the console can show them. */
  readonly missing: string[];
}

/** The single most common Authentik misconfiguration, named explicitly. */
export const RS256_HINT =
  "configure a Certificate-key pair (RS256) on your Authentik OAuth2 provider";

export const SIGNING_KEY_MESSAGE =
  `The identity provider's ID token could not be verified. This almost always means the ` +
  `Authentik OAuth2 provider has no Signing Key configured: without one authentik falls ` +
  `back to signing ID tokens with HS256 using the client secret and omits the "kid" ` +
  `header, and openid-client cannot verify HS256 ID tokens at all (its underlying ` +
  `oauth4webapi engine has no symmetric verification branch and rejects any HS* ` +
  `algorithm outright). To fix it, ${RS256_HINT}, save it, and redeploy the provider. ` +
  `You can confirm the current state with: ` +
  `curl -s <jwks_uri> | jq '.keys[] | {kid, alg, kty}' -- expecting kty "RSA" and alg "RS256". ` +
  `Do not "fix" this by downgrading the client. openid-client's own message for this is ` +
  `"no applicable keys found", which names no setting and no cause.`;

/**
 * The trailing-slash trap.
 *
 * openid-client compares the discovered `issuer` to the configured one BYTE FOR
 * BYTE. Authentik's per-application issuer ends in a slash
 * (`https://idp.example.com/application/o/<slug>/`); configure it without and every
 * login fails at discovery with a message that reads like the IdP is misconfigured.
 */
export function issuerMismatchMessage(expected: string, discovered: string): string {
  return (
    `OIDC issuer mismatch: discovery at "${expected}" returned issuer "${discovered}", ` +
    `which does not match. openid-client compares the issuer byte for byte, so ` +
    `OIDC_ISSUER must be EXACTLY the "issuer" field in ` +
    `${expected}.well-known/openid-configuration, including its trailing slash. For a ` +
    `per-application Authentik provider that means ` +
    `https://idp.example.com/application/o/<slug>/ (with the slash). ` +
    `Note that authorize/, token/ and userinfo/ are GLOBAL endpoints while jwks/ and ` +
    `end-session/ are PER-APPLICATION, so the two paths are expected to differ -- do not ` +
    `"fix" this by hand-editing endpoint URLs; only the issuer value is compared.`
  );
}

/** `offline_access` was silently intersected away, so there is nothing to refresh. */
export function offlineAccessMessage(): string {
  return (
    `The identity provider returned no refresh token. Authentik does NOT error when a ` +
    `requested scope is unassigned: it intersects the requested scope set with the ` +
    `provider's assigned scopes and returns the remainder silently, so "offline_access" ` +
    `disappears with no message anywhere. To fix it, add a Scope Mapping on your Authentik ` +
    `OAuth2/provider provider that maps the scope name "offline_access" to the authentik ` +
    `scope "offline_access" (Authentik 2025.x and later require the explicit mapping; the ` +
    `scope is no longer granted implicitly). Without it this session cannot be renewed ` +
    `and the user will be silently signed out when the access token expires.`
  );
}

/**
 * Translate an error from openid-client / oauth4webapi into something actionable.
 *
 * The original message is always appended: the diagnosis is the point, but
 * discarding what the library actually said would hide a genuine protocol bug
 * behind a guess about the common cause.
 */
export function classifyOidcError(err: unknown, hints: { requestId?: string } = {}): AuthError {
  const raw = err instanceof Error ? err.message : String(err);
  const text = raw.toLowerCase();
  // Prefer a request_id already discovered on a nested cause over the caller's hint.
  const requestId = requestIdFrom(err) ?? hints.requestId;
  const suffix = requestId ? ` (authentik request_id: ${requestId})` : "";

  // THE AUTHORITATIVE SIGNAL. A failed token response arrives as oauth4webapi's
  // `ResponseBodyError`, whose `message` is the content-free "server responded with an
  // error in the response body". The actual OAuth error code lives on `.error`, and
  // `invalid_grant` in particular is a message string we must not have to pattern-match.
  const oauthError = (err as { error?: unknown } | null)?.error;
  if (typeof oauthError === "string" && oauthError.length > 0) {
    const description = (err as { error_description?: unknown }).error_description;
    const detail = typeof description === "string" ? ` (${description})` : "";
    if (oauthError === "invalid_grant") {
      return new AuthError(
        `The identity provider rejected our credentials with invalid_grant${detail}.${suffix}`,
        { code: "oidc_invalid_grant", requestId },
      );
    }
    if (oauthError === "invalid_client") {
      return new AuthError(
        `The identity provider rejected this client's credentials (invalid_client)${detail}. ` +
          `Check OIDC_CLIENT_ID and OIDC_CLIENT_SECRET, and that the provider accepts ` +
          `client_secret_basic, which is what this client is configured to send.${suffix}`,
        { code: "oidc_invalid_client" },
      );
    }
    return new AuthError(`OIDC request failed: ${oauthError}${detail}.${suffix}`, {
      code: "oidc_request_failed",
      httpStatus: 502,
    });
  }

  // openid-client sometimes wraps the real failure in an OperationProcessingError whose
  // cause is the parsed OAuth error body.
  const cause = (err as { cause?: unknown } | undefined)?.cause;
  if (cause instanceof Error && cause !== err && cause.message !== raw) {
    return classifyOidcError(cause, hints);
  }

  if (
    text.includes("does not match the expected issuer") ||
    text.includes("issuer") ||
    text.includes("op_error")
  ) {
    return new AuthError(
      `${issuerMismatchMessage("the configured OIDC_ISSUER", "the discovered issuer")}${suffix}`,
      {
        code: "oidc_issuer_mismatch",
        httpStatus: 500,
      },
    );
  }

  // Key selection / signature verification. This is trap #1 and the reason this
  // module exists: the underlying message is "no applicable keys found".
  if (
    text.includes("no applicable keys") ||
    text.includes("failed to validate") ||
    text.includes("could not validate") ||
    text.includes("jwks") ||
    text.includes("signature") ||
    text.includes("alg") ||
    text.includes("unsupported") ||
    text.includes("hs256") ||
    text.includes("unexpected jws")
  ) {
    return new AuthError(`${SIGNING_KEY_MESSAGE}${suffix}`, {
      code: "oidc_unverifiable_token",
      httpStatus: 500,
    });
  }

  if (text.includes("invalid_grant")) {
    return new AuthError(
      `The identity provider rejected our credentials with invalid_grant.${suffix}`,
      { code: "oidc_invalid_grant" },
    );
  }

  return new AuthError(`OIDC request failed: ${raw}${suffix}`, {
    code: "oidc_request_failed",
    httpStatus: 502,
  });
}

/** Pull authentik's `request_id` out of an OAuth error body, when present. */
export function requestIdFrom(err: unknown): string | undefined {
  let current: unknown = err;
  for (let depth = 0; depth < 5 && current; depth += 1) {
    const value = current as Record<string, unknown>;
    for (const key of ["request_id", "requestId"]) {
      const found = value?.[key];
      if (typeof found === "string" && found.length > 0) return found;
    }
    current = value?.cause;
  }
  return undefined;
}
