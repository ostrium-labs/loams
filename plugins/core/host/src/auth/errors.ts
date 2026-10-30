/**
 * Errors that must never reach a client as a bare 500.
 *
 * Almost everything that goes wrong with an identity-provider integration is a
 * CONFIGURATION mistake, and many of the failure modes are silent on the
 * provider's side: some IdPs with no signing key configured sign HS256 without
 * complaining, an unassigned
 * `offline_access` scope is intersected away, and a `redirect_uri` mismatch is
 * not even an error until the browser round-trips. openid-client's own message
 * for these is accurate but cryptic ("no applicable keys found"), which is the
 * worst possible thing to hand somebody who is trying to stand up an IdP.
 *
 * So every one of those is translated here into a message that names the provider
 * setting to change. `AuthConfigError` carries a stable `code` so tests and logs
 * can assert on the diagnosis rather than on prose.
 */

/** HTTP status for an authorization failure. The router honours this. */
export class AuthError extends Error {
  readonly httpStatus: number;
  readonly code: string;
  /**
   * The `request_id` from the failed response, when the identity provider sends one.
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

/** The single most common identity-provider misconfiguration, named explicitly. */
export const RS256_HINT =
  "configure an asymmetric signing key (RS256) for this application on your identity provider";

export const SIGNING_KEY_MESSAGE =
  `The identity provider's ID token could not be verified. This almost always means the ` +
  `application on the identity provider has no signing key configured: without one ` +
  `some providers fall back to signing ID tokens with HS256 using the client secret and ` +
  `omit the "kid" header, and openid-client cannot verify HS256 ID tokens at all (its underlying ` +
  `oauth4webapi engine has no symmetric verification branch and rejects any HS* ` +
  `algorithm outright). To fix it, ${RS256_HINT}, save it, and re-apply the application's configuration. ` +
  `You can confirm the current state with: ` +
  `curl -s <jwks_uri> | jq '.keys[] | {kid, alg, kty}' -- expecting kty "RSA" and alg "RS256". ` +
  `Do not "fix" this by downgrading the client. openid-client's own message for this is ` +
  `"no applicable keys found", which names no setting and no cause.`;

/**
 * The issuer must match byte for byte (OIDC Discovery 4.3).
 *
 * The `issuer` in the discovery document must be IDENTICAL to the configured one, and
 * the ID token's `iss` is then checked against it. Providers differ on whether their
 * issuer ends in a slash (a path-based issuer often does, a bare origin often does not),
 * and the two spellings look identical to an operator, so the message names both values.
 */
export function issuerMismatchMessage(expected: string, discovered: string): string {
  return (
    `OIDC issuer mismatch: OIDC_ISSUER is "${expected}" but the discovery document ` +
    `${discoveryUrl(expected)} returned issuer "${discovered}". The two must be equal ` +
    `byte for byte (OIDC Discovery 4.3), and a trailing slash difference counts: ` +
    `"https://idp.example.com/x/" and "https://idp.example.com/x" are different issuers. ` +
    `Set OIDC_ISSUER to EXACTLY the "issuer" field of the discovery document. Do not ` +
    `"fix" this by hand-editing endpoint URLs; only the issuer value is compared, and ` +
    `the endpoints are always taken from discovery.`
  );
}

/**
 * The discovery URL for an issuer: `<issuer>/.well-known/openid-configuration`, with no
 * double slash whether or not the issuer ends in one. Used for messages; the request
 * itself is built the same way by openid-client.
 */
export function discoveryUrl(issuer: string): string {
  let end = issuer.length;
  // A linear scan: a `/\/+$/` regex is quadratic on many inner slashes.
  while (end > 0 && issuer.charCodeAt(end - 1) === 47) end -= 1;
  return `${issuer.slice(0, end)}/.well-known/openid-configuration`;
}

/** `offline_access` was silently intersected away, so there is nothing to refresh. */
export function offlineAccessMessage(): string {
  return (
    `The identity provider returned no refresh token. Some identity providers do NOT ` +
    `error when a requested scope is not allowed for the application: they intersect the ` +
    `requested scope set with the application's allowed scopes and return the remainder ` +
    `silently, so "offline_access" disappears with no message anywhere. To fix it, allow ` +
    `the "offline_access" scope (or enable refresh tokens) for this application on your ` +
    `identity provider; some providers need an explicit scope mapping for it, and do not ` +
    `grant it implicitly. Without it this session cannot be renewed ` +
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
export function classifyOidcError(
  err: unknown,
  hints: { requestId?: string; issuer?: string } = {},
): AuthError {
  const raw = err instanceof Error ? err.message : String(err);
  const text = raw.toLowerCase();
  // Prefer a request_id already discovered on a nested cause over the caller's hint.
  const requestId = requestIdFrom(err) ?? hints.requestId;
  const suffix = requestId ? ` (request_id: ${requestId})` : "";

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
    // openid-client's ClientError carries `{ expected, body: { issuer } }` as its cause.
    const detail = (err as { cause?: { expected?: unknown; body?: { issuer?: unknown } } })
      ?.cause;
    const expected =
      hints.issuer ?? (typeof detail?.expected === "string" ? detail.expected : undefined);
    const discovered =
      typeof detail?.body?.issuer === "string" ? detail.body.issuer : undefined;
    const message =
      expected !== undefined && discovered !== undefined
        ? issuerMismatchMessage(expected, discovered)
        : `OIDC issuer mismatch: the "issuer" in the discovery document does not equal ` +
          `OIDC_ISSUER${expected !== undefined ? ` ("${expected}")` : ""}. The two must be ` +
          `equal byte for byte (OIDC Discovery 4.3), and a trailing slash difference ` +
          `counts. Set OIDC_ISSUER to EXACTLY the "issuer" field of the discovery ` +
          `document. (${raw})`;
    return new AuthError(
      `${message}${suffix}`,
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

/** Pull the identity provider's `request_id` out of an OAuth error body, when present. */
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
