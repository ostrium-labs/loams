/**
 * Runtime configuration, read from `<base>config.json` when the console
 * boots (issue #269). Nothing here is baked in at build time: the engine
 * serves its own `config.json` (an empty object, so the console talks to
 * the origin it came from), and a hosted or BYOC deploy (Cloudflare, Vercel,
 * the Compose console) writes one from its environment at deploy time
 * (deploy/config.mjs: LOAMS_ENGINE_URL, LOAMS_AUTH_MODE, LOAMS_AUTH_ISSUER,
 * LOAMS_AUTH_CLIENT_ID).
 */

/**
 * How a hosted console signs in. `authMode` in `config.json` (LOAMS_AUTH_MODE) is the
 * default, and the person may pick another offered one on the connect screen:
 * - `tailscale` (the default): no sign-in screen; the engine identifies the caller from
 *   the tailnet and the console shows whoever the engine says it is.
 * - `oidc`: Authorization Code + PKCE against `auth.issuer` (auth.loams.dev with the
 *   `loams-console` client); the access token goes to the engine as a bearer. Offered
 *   only when the deploy has a client.
 * - `apikey`: the connect screen asks for an API key, kept in memory only.
 */
export type AuthMode = 'oidc' | 'tailscale' | 'apikey';
export const AUTH_MODES: readonly AuthMode[] = ['tailscale', 'oidc', 'apikey'];
export const DEFAULT_AUTH_MODE: AuthMode = 'tailscale';

/**
 * Self-service OIDC client registration on auth.loams.dev, for a console deployed on
 * your own account. Coming soon (the URL is to be confirmed): not linked from the UI.
 */
export const CLIENT_REGISTRATION_URL = 'https://auth.loams.dev/register';

/** The modes this console offers: the deploy's own list when it set one, oidc only with a client. */
export function offeredModes(config: RuntimeConfig): AuthMode[] {
  const modes = config.authModes ?? AUTH_MODES;
  return AUTH_MODES.filter((m) => modes.includes(m) && (m !== 'oidc' || Boolean(config.auth)));
}

export type RuntimeConfig = {
  /**
   * The Loams engine's origin, for example `https://loams.tailnet.ts.net`. Unset means
   * same-origin, or, on a hosted console, the engine the person connects on first run.
   * `config.json` spells it `engineUrl` (or `server`, its older name).
   */
  server?: string;
  /**
   * Set on a console that is not served by the engine (the Compose console, a Cloudflare or
   * Vercel deploy; the hosted console.loams.dev is retired):
   * it asks which engine to talk to and how to sign in, and sends a bearer token (or,
   * on a tailnet, nothing) instead of the engine's session cookie.
   */
  hosted?: boolean;
  /** The OIDC provider (`authIssuer`, `authClientId`, optional `authScope`). */
  auth?: { issuer: string; clientId: string; scope: string };
  /**
   * The default sign-in mode; unset in a hosted `config.json`, `tailscale`. Never guessed
   * from what else is set: an unknown mode, or `oidc` without a usable issuer and client
   * id, is a `configError`.
   */
  authMode?: AuthMode;
  /**
   * The sign-in modes this deploy offers (`authModes`, LOAMS_AUTH_MODES), in the console's
   * order; unset means all of them, `oidc` only with a client. A deploy that narrows the
   * list to what it tests (for example `['tailscale']`) leaves the connect screen with no
   * mode to pick.
   */
  authModes?: AuthMode[];
  /** Why the hosted config cannot be used; the console shows it instead of starting. */
  configError?: string;
};

/** The scopes asked for when `authScope` is unset. */
export const DEFAULT_SCOPE = 'openid profile email offline_access';

const LOOPBACK = new Set(['localhost', '127.0.0.1', '[::1]']);

/**
 * An `http(s)` URL's origin, or undefined. Plain `http` is accepted only when
 * `allowHttp` says so, or for a loopback host (a console and engine on one machine).
 */
export function originOf(value: unknown, allowHttp = false): string | undefined {
  if (typeof value !== 'string' || value.trim() === '') return undefined;
  let url: URL;
  try {
    url = new URL(value.trim());
  } catch {
    return undefined;
  }
  if (url.protocol === 'https:') return url.origin;
  if (url.protocol === 'http:' && (allowHttp || LOOPBACK.has(url.hostname))) return url.origin;
  return undefined;
}

/** An issuer URL: https (http on loopback), path kept, no trailing slash, no query. */
export function issuerOf(value: unknown): string | undefined {
  const origin = originOf(value);
  if (!origin || typeof value !== 'string') return undefined;
  const url = new URL(value.trim());
  if (url.search || url.hash) return undefined;
  return `${origin}${url.pathname.replace(/\/+$/, '')}`;
}

/** Validates a parsed `config.json`. Unknown keys are ignored; bad values are dropped. */
export function parseRuntimeConfig(raw: unknown): RuntimeConfig {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {};
  const r = raw as Record<string, unknown>;
  const config: RuntimeConfig = {};
  const issuer = issuerOf(r.authIssuer);
  const clientId = typeof r.authClientId === 'string' ? r.authClientId.trim() : '';
  if (issuer && clientId) {
    const scope = typeof r.authScope === 'string' && r.authScope.trim() ? r.authScope.trim() : '';
    config.auth = { issuer, clientId, scope: scope || DEFAULT_SCOPE };
  }
  const rawMode = r.authMode;
  // Any of these keys is a hosted deploy's signature; the engine serves an empty object.
  if (r.hosted === true || config.auth || rawMode !== undefined || Array.isArray(r.authModes)) {
    config.hosted = true;
  }
  // Only the origin: API paths are absolute (`/api/v1/...`, `/v1/...`). The engine-served
  // console has always allowed plain http here (a LAN engine); keep that. A hosted console
  // sends a bearer, so its engine is https, or http on loopback (as deploy/config.mjs).
  const server = originOf(r.engineUrl ?? r.server, !config.hosted);
  if (server) config.server = server;
  if (!config.hosted) return config;
  const mode =
    rawMode === undefined
      ? DEFAULT_AUTH_MODE
      : typeof rawMode === 'string'
        ? rawMode.trim().toLowerCase()
        : '';
  if (!(AUTH_MODES as readonly string[]).includes(mode)) {
    // Not a silent fallback to another way of signing in: the deploy is wrong.
    config.configError =
      `config.json has authMode ${JSON.stringify(rawMode)}; ` +
      `it must be one of ${AUTH_MODES.join(', ')}.`;
    delete config.auth;
    return config;
  }
  // The client stays in every mode: it is what lets the person pick oidc.
  if (mode === 'oidc' && !config.auth) {
    config.configError =
      'This console signs in with OIDC (authMode oidc) but config.json has no usable ' +
      'authIssuer and authClientId (the issuer must be https, or http on loopback).';
    return config;
  }
  const rawModes = r.authModes;
  if (Array.isArray(rawModes)) {
    const listed = AUTH_MODES.filter((m) =>
      rawModes.some((v) => typeof v === 'string' && v.trim().toLowerCase() === m),
    );
    if (listed.length > 0) config.authModes = listed;
  }
  const offered = offeredModes(config);
  if (offered.length === 0) {
    config.configError =
      'config.json offers no sign-in mode this console can use (authModes needs tailscale ' +
      'or apikey, or oidc with an authClientId).';
    return config;
  }
  // The default is one of the offered modes: a deploy that lists only others still lands
  // on a mode it meant to offer, never silently on one it left out.
  config.authMode = offered.includes(mode as AuthMode) ? (mode as AuthMode) : offered[0];
  return config;
}

/** Fetches and parses `<base>config.json`; any failure means the defaults. */
export async function loadRuntimeConfig(
  base: string,
  fetcher: typeof fetch = fetch,
  deadlineMs = 3_000,
): Promise<RuntimeConfig> {
  const controller = new AbortController();
  const deadline = setTimeout(() => controller.abort(), deadlineMs);
  try {
    const res = await fetcher(`${base}config.json`, {
      cache: 'no-store',
      signal: controller.signal,
    });
    if (!res.ok) return {};
    const type = res.headers.get('content-type') ?? '';
    // An SPA fallback answers a missing file with index.html.
    if (!type.includes('json')) return {};
    return parseRuntimeConfig(await res.json());
  } catch {
    return {};
  } finally {
    clearTimeout(deadline);
  }
}
