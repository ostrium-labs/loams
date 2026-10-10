/**
 * Probing the IdP's JWKS so the HS256 trap is caught BEFORE a login, legibly.
 *
 * THE TRAP
 * An Authentik OAuth2 provider with no Signing Key does not refuse to issue tokens. It
 * signs ID tokens with HS256, using the client secret as the HMAC key, and omits the
 * `kid` header entirely. openid-client v6 cannot verify that at any point: its engine is
 * oauth4webapi, which has no symmetric verification branch and explicitly rejects `HS*`
 * algorithms. The result is an opaque "no applicable keys found" at the moment of the
 * token exchange, with nothing in it that points at the setting that actually needs
 * changing.
 *
 * Rather than translate that message after the fact (which can only guess), this module
 * asks the IdP directly what keys it publishes, once, right after discovery. A JWKS with
 * no asymmetric key is unambiguous evidence of the misconfiguration, and it can be
 * reported before anybody tries to log in.
 *
 * This also accepts ES256/EdDSA rather than insisting on RSA: the requirement is an
 * ASYMMETRIC key, and refusing a legitimate ES256 deployment would be its own bug.
 */

import { AuthError, SIGNING_KEY_MESSAGE } from "./errors.js";

/** Asymmetric JWS algorithms openid-client can verify. HS* is deliberately absent. */
const ASYMMETRIC_ALGS = new Set([
  "RS256",
  "RS384",
  "RS512",
  "PS256",
  "PS384",
  "PS512",
  "ES256",
  "ES384",
  "ES512",
  "EdDSA",
]);

const ASYMMETRIC_KTY = new Set(["RSA", "EC", "OKP"]);

export interface SigningKeyProbe {
  jwksUri?: string;
  /** How many keys the JWKS published. */
  total: number;
  /** `kid`/`alg`/`kty` of every key that could verify an asymmetric ID token. */
  asymmetric: { kid?: string; alg?: string; kty?: string }[];
  /** How many symmetric (`oct`) keys were published. */
  symmetric: number;
  /** True when at least one asymmetric key is usable. */
  usable: boolean;
  /** Why the probe could not run, when it could not. */
  error?: string;
}

/**
 * Fetch and classify the IdP's JWKS.
 *
 * Never throws: a JWKS that cannot be fetched is reported as `error` so the caller can
 * decide whether that is fatal. Failing to reach the key set is not the same as the key
 * set being wrong, and the two need different advice.
 */
export async function probeSigningKeys(
  jwksUri: string | undefined,
  timeoutMs: number,
): Promise<SigningKeyProbe> {
  if (!jwksUri) {
    return { total: 0, asymmetric: [], symmetric: 0, usable: false, error: "no jwks_uri" };
  }
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetch(jwksUri, { signal: controller.signal });
    if (!response.ok) {
      return {
        jwksUri,
        total: 0,
        asymmetric: [],
        symmetric: 0,
        usable: false,
        error: `jwks_uri returned HTTP ${response.status}`,
      };
    }
    const body = (await response.json()) as { keys?: Record<string, unknown>[] };
    const keys = Array.isArray(body?.keys) ? body.keys : [];
    const asymmetric = keys
      .filter((key) => {
        const kty = String(key?.kty ?? "");
        const alg = String(key?.alg ?? "");
        return (
          ASYMMETRIC_KTY.has(kty) ||
          ASYMMETRIC_ALGS.has(alg) ||
          (alg !== "" && !alg.startsWith("HS"))
        );
      })
      .map((key) => ({
        kid: typeof key?.kid === "string" ? key.kid : undefined,
        alg: typeof key?.alg === "string" ? key.alg : undefined,
        kty: typeof key?.kty === "string" ? key.kty : undefined,
      }));
    const symmetric = keys.filter((key) => String(key?.kty ?? "") === "oct").length;
    return {
      jwksUri,
      total: keys.length,
      asymmetric,
      symmetric,
      usable: asymmetric.length > 0,
    };
  } catch (err) {
    return {
      jwksUri,
      total: 0,
      asymmetric: [],
      symmetric: 0,
      usable: false,
      error: err instanceof Error ? err.message : String(err),
    };
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Turn a failed probe into an actionable configuration error, or `undefined` when the
 * probe is inconclusive and there is nothing useful to say.
 *
 * Only fires on positive evidence -- a JWKS that really has no asymmetric key. An
 * unreachable JWKS returns `undefined` rather than guessing, because the honest answer
 * there is "we could not check", not "your key is missing".
 */
export function signingKeyProblem(probe: SigningKeyProbe): AuthError | undefined {
  if (probe.usable) return undefined;
  if (probe.error) return undefined;

  if (probe.total === 0 || probe.symmetric > 0) {
    return new AuthError(
      `${SIGNING_KEY_MESSAGE} (Checked ${probe.jwksUri ?? "the provider's jwks_uri"}: it ` +
        `publishes ${probe.total} key(s), ${probe.symmetric} of them symmetric, and no ` +
        `asymmetric key that could verify an ID token.)`,
      { code: "oidc_no_signing_key", httpStatus: 500 },
    );
  }

  return new AuthError(
    `${SIGNING_KEY_MESSAGE} (Checked ${probe.jwksUri ?? "the provider's jwks_uri"}: none of ` +
      `its ${probe.total} key(s) can verify an ID token.)`,
    { code: "oidc_unusable_signing_key", httpStatus: 500 },
  );
}

/**
 * The same question answered from metadata rather than the key set, for providers that
 * advertise their algorithms without a usable JWKS. Kept separate so the two pieces of
 * evidence are never conflated in a message.
 */
export function algMetadataProblem(algs: unknown): string | undefined {
  if (!Array.isArray(algs) || algs.length === 0) return undefined;
  const usable = algs.filter((alg) => ASYMMETRIC_ALGS.has(String(alg)));
  if (usable.length > 0) return undefined;
  return (
    `The provider advertises id_token_signing_alg_values_supported = ${JSON.stringify(algs)}, ` +
    `which contains no asymmetric algorithm. ${SIGNING_KEY_MESSAGE}`
  );
}
