// Writes the hosted console's runtime config.json from the deploy's environment
// (src/runtime-config.ts reads it in the browser):
//
//   LOAMS_AUTH_CLIENT_ID=loams-console node deploy/config.mjs dist-hosted/config.json
//
//   LOAMS_ENGINE_URL      optional: fixes the engine (https://loams.<tailnet>.ts.net); unset,
//                         the person connects one on first run. LOAMS_CONSOLE_SERVER is
//                         its older name.
//   LOAMS_AUTH_MODE       the default sign-in mode: tailscale (default) | oidc | apikey.
//                           tailscale  no sign-in; the engine identifies the tailnet user
//                           oidc       sign in with Authorization Code + PKCE at the issuer
//                           apikey     ask for an API key, kept in the tab's memory only
//                         The person can pick another one on the connect screen; oidc is
//                         offered when the deploy has a client. Anything else is an error.
//   LOAMS_AUTH_CLIENT_ID  the console's public client id at the issuer (Ostrium's console:
//                         loams-console); required for LOAMS_AUTH_MODE=oidc and for a
//                         --require-auth deploy. Self-service client registration
//                         (REGISTER_URL) is coming soon.
//   LOAMS_AUTH_ISSUER     the issuer, default https://auth.loams.dev (your own IdP: coming
//                         soon in the UI, already accepted here).
//   LOAMS_AUTH_SCOPE      optional, default "openid profile email offline_access".
//
// stage.mjs calls `hostedConfig` too, so Cloudflare and Vercel deploys get the same file.
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const LOOPBACK = new Set(['localhost', '127.0.0.1', '[::1]']);

/** An https URL (http only on loopback), or a thrown error naming `what`. */
export function secureUrl(value, what) {
  let url;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`${what} must be a valid HTTPS URL, got ${JSON.stringify(value)}`);
  }
  const ok = url.protocol === 'https:' || (url.protocol === 'http:' && LOOPBACK.has(url.hostname));
  if (!ok) throw new Error(`${what} must use HTTPS (plain http only on localhost)`);
  if (url.search || url.hash) throw new Error(`${what} must not carry a query or fragment`);
  return url;
}

const get = (env, name) => {
  const v = env[name];
  return typeof v === 'string' && v.trim() !== '' ? v.trim() : undefined;
};

export const AUTH_MODES = ['tailscale', 'oidc', 'apikey'];
export const DEFAULT_MODE = 'tailscale';
export const DEFAULT_ISSUER = 'https://auth.loams.dev';
/** Self-service OIDC client registration on auth.loams.dev: coming soon (URL to confirm). */
export const REGISTER_URL = 'https://auth.loams.dev/register';

/**
 * LOAMS_AUTH_MODES: the sign-in modes this deploy offers, separated by spaces or
 * commas. Unset, the console offers all of them (oidc only with a client). Set it to
 * narrow the choice, for example `tailscale` for a console where the other ways in
 * are not tested yet. It must be a subset of AUTH_MODES, and must include the default
 * LOAMS_AUTH_MODE, and `oidc` needs LOAMS_AUTH_CLIENT_ID.
 */
export function authModes(env = process.env, mode = DEFAULT_MODE) {
  const raw = get(env, 'LOAMS_AUTH_MODES');
  if (!raw) return undefined;
  const wanted = [];
  for (const value of raw.split(/[\s,]+/).filter(Boolean)) {
    const m = value.toLowerCase();
    if (!AUTH_MODES.includes(m)) {
      throw new Error(
        `LOAMS_AUTH_MODES entry ${JSON.stringify(value)} must be one of ${AUTH_MODES.join(', ')}`,
      );
    }
    if (!wanted.includes(m)) wanted.push(m);
  }
  if (!wanted.includes(mode)) {
    throw new Error(`LOAMS_AUTH_MODES must include the default LOAMS_AUTH_MODE (${mode})`);
  }
  // The console's order, whatever order the value listed them in.
  return AUTH_MODES.filter((m) => wanted.includes(m));
}

/**
 * The config.json object for a hosted deploy, validated. `requireAuth` (Ostrium's
 * console.loams.dev) requires the OIDC client, so the console offers a Loams account.
 */
export function hostedConfig(env = process.env, { requireAuth = false } = {}) {
  const config = { hosted: true };
  const engine = get(env, 'LOAMS_ENGINE_URL') ?? get(env, 'LOAMS_CONSOLE_SERVER');
  if (engine) config.engineUrl = secureUrl(engine, 'LOAMS_ENGINE_URL').origin;
  const mode = (get(env, 'LOAMS_AUTH_MODE') ?? DEFAULT_MODE).toLowerCase();
  if (!AUTH_MODES.includes(mode)) {
    throw new Error(`LOAMS_AUTH_MODE must be one of ${AUTH_MODES.join(', ')}, got ${mode}`);
  }
  config.authMode = mode;
  const clientId = get(env, 'LOAMS_AUTH_CLIENT_ID');
  const modes = authModes(env, mode);
  if (modes) {
    if (modes.includes('oidc') && !clientId) {
      throw new Error('LOAMS_AUTH_MODES offers oidc, which needs LOAMS_AUTH_CLIENT_ID');
    }
    config.authModes = modes;
  }
  if (!clientId) {
    if (mode === 'oidc' || requireAuth) {
      throw new Error(
        `${requireAuth ? 'this deploy' : 'LOAMS_AUTH_MODE=oidc'} needs LOAMS_AUTH_CLIENT_ID ` +
          "(the console's client at the issuer; self-service registration at auth.loams.dev " +
          'is coming soon), or set LOAMS_AUTH_MODE=tailscale or apikey',
      );
    }
    if (get(env, 'LOAMS_AUTH_ISSUER')) {
      throw new Error('LOAMS_AUTH_ISSUER is set without LOAMS_AUTH_CLIENT_ID');
    }
    return config;
  }
  const url = secureUrl(get(env, 'LOAMS_AUTH_ISSUER') ?? DEFAULT_ISSUER, 'LOAMS_AUTH_ISSUER');
  config.authIssuer = `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
  config.authClientId = clientId;
  const scope = get(env, 'LOAMS_AUTH_SCOPE');
  if (scope) config.authScope = scope;
  return config;
}

/**
 * LOAMS_AGENTD_ORIGINS: the origins of the remote agentd hosts (`loams-agentd serve
 * --remote`, §53 §6.3) this console may reach, separated by spaces or commas. Each is
 * an exact `https://host[:port]` (http only on loopback): no wildcard, path, query or
 * credentials. Studio calls them over Connect and frames their previews, so they join
 * the CSP's `connect-src` and `frame-src` (SD1 Task 30, ruling T26-9; as §39's console
 * lists the factory apps' origins). Each host must list this console in its own
 * `--allow-origin`. Unset: Studio on the web can reach no agentd host.
 */
export function agentdOrigins(env = process.env) {
  const raw = get(env, 'LOAMS_AGENTD_ORIGINS');
  if (!raw) return [];
  const origins = [];
  for (const value of raw.split(/[\s,]+/).filter(Boolean)) {
    if (value.includes('*')) {
      throw new Error(`LOAMS_AGENTD_ORIGINS: name each agentd host, not ${value}`);
    }
    const url = secureUrl(value, `LOAMS_AGENTD_ORIGINS entry ${JSON.stringify(value)}`);
    if (url.username || url.password || (url.pathname !== '/' && url.pathname !== '')) {
      throw new Error(
        `LOAMS_AGENTD_ORIGINS entry ${JSON.stringify(value)} must be an origin ` +
          `(scheme://host[:port]), such as ${url.origin}`,
      );
    }
    if (!origins.includes(url.origin)) origins.push(url.origin);
  }
  return origins;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const out = process.argv[2];
  if (!out) {
    console.error('usage: node deploy/config.mjs <path/to/config.json>');
    process.exit(2);
  }
  const config = hostedConfig(process.env, {
    requireAuth: process.argv.includes('--require-auth'),
  });
  writeFileSync(out, `${JSON.stringify(config, null, 2)}\n`);
  console.log(`wrote ${out}`);
}
