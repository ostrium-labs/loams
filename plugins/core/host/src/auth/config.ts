/**
 * Reading the auth configuration out of the environment.
 *
 * DESIGN RULE: auth is DISABLE-able, and disabling it is loud.
 * With no `OIDC_ISSUER`/`OIDC_CLIENT_ID` the service reports `enabled: false` and the
 * app boots unauthenticated, because a developer running `npm run dev` against no IdP
 * should get a working dashboard rather than a login loop. The inverse is the
 * dangerous direction and is guarded explicitly: admin powers are NEVER granted just
 * because auth is off. `effectiveAdminScope()` returns nothing while disabled, so a
 * dev-mode console cannot reconfigure the system's capabilities.
 */

import { AuthError } from "./errors.js";

export interface AuthConfig {
  /** False when the IdP is not configured. The app then runs unauthenticated. */
  enabled: boolean;
  /** Why auth is off, for the boot log. */
  disabledReason?: string;

  issuer: string;
  clientId: string;
  clientSecret: string;
  redirectUri: string;
  postLogoutRedirectUri?: string;

  /** Space-separated scope string, exactly as sent in the authorization request. */
  scope: string;
  /** The same set parsed, for membership checks. */
  scopes: string[];
  /** True when `offline_access` was requested. Governs refresh behaviour. */
  wantsOfflineAccess: boolean;

  insecure: boolean;
  adminScope: string;
  sessionCookieName: string;
  sessionCookieSecure: boolean;
  /** Absolute session lifetime, regardless of activity. */
  sessionTtlMs: number;
  /** How long an authorization request (state/nonce/PKCE) stays valid. */
  authRequestTtlMs: number;
  /** HTTP timeout for discovery and every token request, in ms. */
  requestTimeoutMs: number;

  /** `hashed_user_id` (default) or `sub`. */
  subMode: "hashed_user_id" | "sub";
  installId: string;

  /** Explicit CORS allowlist. Empty means "reflect nothing but same-origin". */
  allowedOrigins: string[];

  /** Set for a session whose user has the admin scope. Derived, not configured. */
  issuerMetadataUrl?: string;
}

export type Env = Record<string, string | undefined>;

export const DEFAULTS = {
  scope: "openid profile email",
  adminScope: "bi:admin",
  sessionCookieName: "bi_session",
  sessionTtlMs: 12 * 60 * 60 * 1000,
  authRequestTtlMs: 10 * 60 * 1000,
  requestTimeoutMs: 10_000,
  redirectUri: "http://localhost:3001/api/auth/callback",
  postLogoutRedirectUri: "http://localhost:3001/",
  installId: "bi",
  subMode: "hashed_user_id" as const,
};

function bool(value: string | undefined, fallback = false): boolean {
  if (value === undefined || value === "") return fallback;
  return ["1", "true", "yes", "on"].includes(value.toLowerCase());
}

function int(value: string | undefined, fallback: number): number {
  if (!value) return fallback;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function list(value: string | undefined): string[] {
  if (!value) return [];
  return value
    .split(",")
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

/**
 * Build the config. Throws {@link AuthConfigError}-style {@link AuthError} for a
 * configuration that is present but wrong, because those must be fixed before boot
 * rather than discovered on the first login.
 */
export function loadAuthConfig(env: Env = process.env): AuthConfig {
  const issuer = (env.OIDC_ISSUER ?? "").trim();
  const clientId = (env.OIDC_CLIENT_ID ?? "").trim();
  const clientSecret = (env.OIDC_CLIENT_SECRET ?? "").trim();

  if (!issuer || !clientId) {
    const missing = [!issuer && "OIDC_ISSUER", !clientId && "OIDC_CLIENT_ID"].filter(
      (entry): entry is string => typeof entry === "string",
    );
    return {
      ...base(env),
      enabled: false,
      disabledReason:
        `${missing.join(" and ")} not set. Running UNAUTHENTICATED: every request is ` +
        `treated as an anonymous principal with no scopes, admin operations are refused, ` +
        `and no session can be created. Set OIDC_ISSUER (with a trailing slash) and ` +
        `OIDC_CLIENT_ID to enable login.`,
      issuer,
      clientId,
      clientSecret,
      scope: (env.OIDC_SCOPE ?? DEFAULTS.scope).trim() || DEFAULTS.scope,
      scopes: [],
      wantsOfflineAccess: false,
      allowedOrigins: list(env.CORS_ALLOWED_ORIGINS ?? env.ORIGIN_ALLOWLIST),
    };
  }

  const scope = (env.OIDC_SCOPE ?? DEFAULTS.scope).trim() || DEFAULTS.scope;
  const scopes = scope.split(/\s+/).filter(Boolean);

  return {
    ...base(env),
    enabled: true,
    issuer,
    clientId,
    clientSecret,
    scope,
    scopes,
    wantsOfflineAccess: scopes.includes("offline_access"),
    allowedOrigins: list(env.CORS_ALLOWED_ORIGINS ?? env.ORIGIN_ALLOWLIST),
  };
}

function base(
  env: Env,
): Omit<
  AuthConfig,
  | "enabled"
  | "issuer"
  | "clientId"
  | "clientSecret"
  | "scope"
  | "scopes"
  | "wantsOfflineAccess"
  | "allowedOrigins"
> {
  const insecure = bool(env.OIDC_INSECURE) && env.NODE_ENV !== "production";
  return {
    redirectUri: (env.OIDC_REDIRECT_URI ?? DEFAULTS.redirectUri).trim(),
    postLogoutRedirectUri: (
      env.OIDC_POST_LOGOUT_REDIRECT_URI ?? DEFAULTS.postLogoutRedirectUri
    ).trim(),
    insecure,
    adminScope: (env.ADMIN_SCOPE ?? DEFAULTS.adminScope).trim() || DEFAULTS.adminScope,
    sessionCookieName: (env.SESSION_COOKIE_NAME ?? DEFAULTS.sessionCookieName).trim(),
    // Secure by default. Overridable ONLY because a developer running the SPA over
    // plain http://localhost has no way to store a Secure cookie, which would make the
    // login flow untestable locally. Never set this in production.
    sessionCookieSecure: bool(env.SESSION_COOKIE_SECURE, true),
    sessionTtlMs: int(env.SESSION_TTL_MS, DEFAULTS.sessionTtlMs),
    authRequestTtlMs: int(env.AUTH_REQUEST_TTL_MS, DEFAULTS.authRequestTtlMs),
    requestTimeoutMs: int(env.OIDC_TIMEOUT_MS, DEFAULTS.requestTimeoutMs),
    subMode: (env.SUB_MODE === "sub" ? "sub" : DEFAULTS.subMode) as AuthConfig["subMode"],
    installId: (env.INSTALL_ID ?? DEFAULTS.installId).trim() || DEFAULTS.installId,
  };
}

/* -------------------------------------------------------------------------- */
/* Validation                                                                 */
/* -------------------------------------------------------------------------- */

/**
 * Validate the parts that fail at LOGIN time if left alone.
 *
 * Called during `[Service.init]` so a bad deployment is refused at boot with a legible
 * message, rather than producing a login loop or a 500 on the callback.
 */
export function validateAuthConfig(config: AuthConfig): void {
  if (!config.enabled) return;

  if (!config.issuer.endsWith("/")) {
    throw new AuthError(
      `OIDC_ISSUER must end with a trailing slash. Got "${config.issuer}". Authentik's ` +
        `per-application issuer is "https://idp.example.com/application/o/<slug>/" and ` +
        `openid-client compares the issuer byte for byte against the discovered value, so a ` +
        `missing slash makes every login fail with "discovered metadata issuer does not ` +
        `match the expected issuer". Add the slash; do not let anything normalise it away.`,
      { code: "oidc_issuer_not_slash_terminated", httpStatus: 500 },
    );
  }

  let redirect: URL;
  try {
    redirect = new URL(config.redirectUri);
  } catch {
    throw new AuthError(`OIDC_REDIRECT_URI must be an absolute URL. Got "${config.redirectUri}".`, {
      code: "oidc_bad_redirect_uri",
      httpStatus: 500,
    });
  }
  if (redirect.search || redirect.hash) {
    throw new AuthError(
      `OIDC_REDIRECT_URI must be path-only: no query string and no fragment. Got ` +
        `"${config.redirectUri}". openid-client derives the redirect_uri it sends to the ` +
        `token endpoint by stripping searchParams and the hash, so anything present here ` +
        `is dropped for that second request and the two no longer match.`,
      { code: "oidc_bad_redirect_uri", httpStatus: 500 },
    );
  }

  if (!config.scopes.includes("openid")) {
    throw new AuthError(
      `OIDC_SCOPE must include "openid" -- without it this is a plain OAuth2 client and ` +
        `there is no ID token, hence no user identity. Got "${config.scope}".`,
      { code: "oidc_scope_missing_openid", httpStatus: 500 },
    );
  }

  if (!config.clientSecret) {
    // Not fatal: a public client with PKCE is legitimate. Warn-worthy, not boot-stopping.
    return;
  }
}

/**
 * The origins a browser is allowed to make credentialed requests from.
 *
 * Replaces `Access-Control-Allow-Origin: *`, which is not merely loose: the CORS spec
 * makes `*` incompatible with credentialed requests, so while it stood the session
 * cookie could never have worked from a browser at all. A configured allowlist is the
 * prerequisite for the whole login flow, not a hardening step.
 */
export function resolveAllowedOrigins(config: AuthConfig, extra: string[] = []): string[] {
  const origins = new Set<string>();
  for (const origin of [...config.allowedOrigins, ...extra]) {
    const trimmed = origin.trim().replace(/\/+$/, "");
    if (trimmed.length > 0) origins.add(trimmed);
  }
  if (origins.has("*")) {
    throw new AuthError(
      `CORS_ALLOWED_ORIGINS may not contain "*". A wildcard origin is incompatible with ` +
        `credentialed requests per the CORS specification, which means the session cookie ` +
        `cannot be sent at all, and it would re-open cross-origin access to every /api route. ` +
        `List the origins explicitly instead.`,
      { code: "cors_wildcard_rejected", httpStatus: 500 },
    );
  }
  // The redirect URI's own origin is always allowed: it is where the callback lands.
  try {
    origins.add(new URL(config.redirectUri).origin);
  } catch {
    /* already validated */
  }
  return [...origins];
}

/**
 * Whether a principal may act as an administrator.
 *
 * Returns `false` while auth is disabled. That is the "never silently enable admin
 * powers without auth configured" rule, enforced in one place.
 */
export function effectiveAdminScope(config: AuthConfig): string | undefined {
  return config.enabled ? config.adminScope : undefined;
}
