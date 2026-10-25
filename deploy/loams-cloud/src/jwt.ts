// AuthKit access tokens (design 78 §25.5, §25.8): RS256 JWTs verified against
// WorkOS's JWKS at https://api.workos.com/sso/jwks/{client_id}, with `sub`,
// `sid`, `org_id`, `role`, `roles` and `permissions` claims.

const dec = new TextDecoder();

function unb64url(s: string): Uint8Array {
  const pad = s.length % 4 === 2 ? "==" : s.length % 4 === 3 ? "=" : "";
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/") + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export interface Jwk {
  kty: string;
  kid?: string;
  alg?: string;
  use?: string;
  n?: string;
  e?: string;
}

export interface AccessClaims {
  sub: string;
  sid?: string;
  org_id?: string;
  role?: string;
  roles?: string[];
  permissions?: string[];
  iss?: string;
  aud?: string | string[];
  exp?: number;
  nbf?: number;
  iat?: number;
  act?: unknown;
  [k: string]: unknown;
}

export class JwtError extends Error {}

/** Fetches the JWKS. The Worker's is a fetch of the WorkOS URL; tests serve their own keys. */
export type JwksSource = () => Promise<{ keys: Jwk[] }>;

export function httpJwks(url: string, fetchFn: typeof fetch = fetch): JwksSource {
  return async () => {
    const r = await fetchFn(url, { signal: AbortSignal.timeout(5000) });
    if (!r.ok) throw new JwtError(`JWKS ${r.status}`);
    return (await r.json()) as { keys: Jwk[] };
  };
}

export interface VerifierOptions {
  issuer: string;
  audience?: string;
  /** Clock skew allowed on exp and nbf, in seconds. */
  leewayS?: number;
  now?: () => number;
  /** A JWKS refetch for an unknown kid happens at most this often. */
  minRefetchMs?: number;
}

export class JwtVerifier {
  private readonly source: JwksSource;
  private readonly opts: Required<Omit<VerifierOptions, "audience">> & { audience?: string };
  private keys = new Map<string, CryptoKey>();
  private fetchedAt = -Infinity;

  constructor(source: JwksSource, opts: VerifierOptions) {
    this.source = source;
    this.opts = { leewayS: 60, now: () => Date.now(), minRefetchMs: 60_000, ...opts };
  }

  private async refresh(): Promise<void> {
    const now = this.opts.now();
    if (now - this.fetchedAt < this.opts.minRefetchMs && this.keys.size > 0) return;
    this.fetchedAt = now;
    const { keys } = await this.source();
    const next = new Map<string, CryptoKey>();
    for (const k of keys) {
      if (k.kty !== "RSA" || (k.alg && k.alg !== "RS256") || (k.use && k.use !== "sig") || !k.n || !k.e) continue;
      const key = await crypto.subtle.importKey("jwk", { kty: "RSA", n: k.n, e: k.e, alg: "RS256", ext: true }, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
      next.set(k.kid ?? "", key);
    }
    this.keys = next;
  }

  private async key(kid: string): Promise<CryptoKey> {
    let k = this.keys.get(kid);
    if (!k) {
      await this.refresh();
      k = this.keys.get(kid);
    }
    if (!k) throw new JwtError("unknown signing key");
    return k;
  }

  async verify(token: string): Promise<AccessClaims> {
    const parts = token.split(".");
    if (parts.length !== 3) throw new JwtError("malformed token");
    let header: { alg?: string; kid?: string; typ?: string };
    let claims: AccessClaims;
    try {
      header = JSON.parse(dec.decode(unb64url(parts[0])));
      claims = JSON.parse(dec.decode(unb64url(parts[1])));
    } catch {
      throw new JwtError("malformed token");
    }
    if (header.alg !== "RS256") throw new JwtError("unsupported algorithm");
    const key = await this.key(header.kid ?? "");
    const ok = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, unb64url(parts[2]) as BufferSource, new TextEncoder().encode(`${parts[0]}.${parts[1]}`));
    if (!ok) throw new JwtError("bad signature");
    const nowS = this.opts.now() / 1000;
    const leeway = this.opts.leewayS;
    if (typeof claims.exp !== "number" || claims.exp + leeway < nowS) throw new JwtError("token expired");
    if (typeof claims.nbf === "number" && claims.nbf - leeway > nowS) throw new JwtError("token not yet valid");
    if (claims.iss !== this.opts.issuer) throw new JwtError("wrong issuer");
    if (this.opts.audience !== undefined) {
      const aud = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
      if (!aud.includes(this.opts.audience)) throw new JwtError("wrong audience");
    }
    if (typeof claims.sub !== "string" || !claims.sub) throw new JwtError("no subject");
    return claims;
  }
}
