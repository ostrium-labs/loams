// An agent's token from its identity provider's private-key JWT key (design 76 §10, D1607;
// plan IA1, IA4 Task 2).
//
// An agent is a machine user at the identity provider (Zitadel). Its key file is the JSON the
// provider hands out once (`{"type": "serviceaccount", "keyId", "key", "userId"}`). The source
// signs an RFC 7523 assertion (RS256, `kid` = `keyId`, `iss` = `sub` = `userId`, `aud` = the
// issuer, valid for 5 minutes) with WebCrypto, trades it at `<issuer>/oauth/v2/token` for an
// access token whose audience is the Loams project
// (`urn:zitadel:iam:org:project:id:<project>:aud`), caches it, and trades again a minute
// before it expires or when the server answers `token_expired`. Concurrent callers share one
// trade. The private key never leaves the process. A sandboxed agent holds no key: agentd
// vends it a 15-minute token file instead (D1618).

import type { TokenSource } from './token-source.js';

/** Bytes on a plain `ArrayBuffer`, which WebCrypto takes. */
type Bytes = Uint8Array<ArrayBuffer>;

/** How long an assertion is valid, in seconds. */
export const MACHINE_KEY_ASSERTION_TTL_S = 300;
/** How long before expiry a token is traded again, in seconds. */
export const MACHINE_KEY_REFRESH_EARLY_S = 60;
/** The RFC 7523 grant type. */
export const JWT_BEARER_GRANT = 'urn:ietf:params:oauth:grant-type:jwt-bearer';

/** A machine user's key file, as the identity provider hands it out. */
export interface MachineKeyFile {
  type?: string;
  keyId: string;
  key: string;
  userId: string;
}

export interface MachineKeyOptions {
  /** The key file's JSON text, or the parsed file. */
  keyFile: string | MachineKeyFile;
  /** The identity provider's issuer URL (`https://auth.loams.dev`). */
  issuer: string;
  /** The Loams project's id at the identity provider: the audience Loams checks. */
  project: string;
  /** Injected for tests and for hosts with their own `fetch`. */
  fetch?: typeof globalThis.fetch;
  /** The clock, in milliseconds (tests). */
  now?: () => number;
}

const b64url = (bytes: Bytes): string => {
  let text = '';
  for (const b of bytes) {
    text += String.fromCharCode(b);
  }
  return btoa(text).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/u, '');
};

const utf8 = (text: string): Bytes => Uint8Array.from(new TextEncoder().encode(text));

function derLength(n: number): Bytes {
  if (n < 0x80) {
    return Uint8Array.of(n);
  }
  const out: number[] = [];
  for (let v = n; v > 0; v >>= 8) {
    out.unshift(v & 0xff);
  }
  return Uint8Array.of(0x80 | out.length, ...out);
}

function der(tag: number, ...parts: Bytes[]): Bytes {
  const body = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    body.set(p, at);
    at += p.length;
  }
  const length = derLength(body.length);
  const out = new Uint8Array(1 + length.length + body.length);
  out[0] = tag;
  out.set(length, 1);
  out.set(body, 1 + length.length);
  return out;
}

/** PKCS #1 `RSAPrivateKey` wrapped as PKCS #8, which is what WebCrypto imports. */
function pkcs1ToPkcs8(pkcs1: Bytes): Bytes {
  const version = Uint8Array.of(0x02, 0x01, 0x00);
  // rsaEncryption (1.2.840.113549.1.1.1) with NULL parameters.
  const algorithm = Uint8Array.of(
    0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00,
  );
  return der(0x30, version, algorithm, der(0x04, pkcs1));
}

function pemToPkcs8(pem: string): Bytes {
  const match = /-----BEGIN (RSA )?PRIVATE KEY-----([\s\S]+?)-----END (RSA )?PRIVATE KEY-----/u.exec(
    pem,
  );
  if (match === null) {
    throw new Error('machineKey: the key is not a PEM private key');
  }
  const bytes = Uint8Array.from(atob((match[2] ?? '').replace(/\s+/gu, '')), (c) => c.charCodeAt(0));
  return match[1] === undefined ? bytes : pkcs1ToPkcs8(bytes);
}

function parseKeyFile(keyFile: string | MachineKeyFile): MachineKeyFile {
  let file: MachineKeyFile;
  if (typeof keyFile === 'string') {
    try {
      file = JSON.parse(keyFile) as MachineKeyFile;
    } catch {
      throw new Error('machineKey: not a machine key file');
    }
  } else {
    file = keyFile;
  }
  if (file.type !== undefined && file.type !== '' && file.type !== 'serviceaccount') {
    throw new Error(`machineKey: a ${JSON.stringify(file.type)} key is not a machine user's key`);
  }
  if (!file.keyId || !file.userId || !file.key) {
    throw new Error('machineKey: the key file has no keyId, key or userId');
  }
  return file;
}

/**
 * An agent's token source from its machine key (module header). The key is checked when the
 * source is built; it is imported into WebCrypto on first use.
 */
export function machineKey(options: MachineKeyOptions): TokenSource & { readonly userId: string } {
  const file = parseKeyFile(options.keyFile);
  const issuer = options.issuer.replace(/\/+$/u, '');
  if (!/^https?:\/\/[^/]+/u.test(issuer)) {
    throw new Error(`machineKey: the issuer ${JSON.stringify(issuer)} is not a URL`);
  }
  if (options.project === '') {
    throw new Error('machineKey: the Loams project id (the audience) is empty');
  }
  const pkcs8 = pemToPkcs8(file.key);
  const scope = `openid urn:zitadel:iam:org:project:id:${options.project}:aud`;
  const doFetch = options.fetch ?? globalThis.fetch;
  const now = options.now ?? Date.now;
  let signingKey: Promise<CryptoKey> | undefined;
  let cached: { token: string; refreshAt: number } | undefined;
  let inFlight: Promise<string> | undefined;

  const assertion = async (issuedAtMs: number): Promise<string> => {
    signingKey ??= globalThis.crypto.subtle.importKey(
      'pkcs8',
      pkcs8,
      { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' },
      false,
      ['sign'],
    );
    const iat = Math.floor(issuedAtMs / 1000);
    const header = b64url(utf8(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: file.keyId })));
    const claims = b64url(
      utf8(
        JSON.stringify({
          iss: file.userId,
          sub: file.userId,
          aud: issuer,
          iat,
          exp: iat + MACHINE_KEY_ASSERTION_TTL_S,
        }),
      ),
    );
    const signing = `${header}.${claims}`;
    const signature = await globalThis.crypto.subtle.sign(
      'RSASSA-PKCS1-v1_5',
      await signingKey,
      utf8(signing),
    );
    return `${signing}.${b64url(new Uint8Array(signature))}`;
  };

  const trade = (): Promise<string> => {
    inFlight ??= (async () => {
      const started = now();
      const response = await doFetch(`${issuer}/oauth/v2/token`, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({
          grant_type: JWT_BEARER_GRANT,
          scope,
          assertion: await assertion(started),
        }).toString(),
      });
      if (!response.ok) {
        const text = (await response.text()).slice(0, 512);
        throw new Error(`the token endpoint refused the machine key (${response.status}): ${text}`);
      }
      const body = (await response.json()) as { access_token?: string; expires_in?: number };
      if (!body.access_token) {
        throw new Error('the token endpoint answered no access_token');
      }
      const life = body.expires_in ?? 900;
      const early = Math.min(MACHINE_KEY_REFRESH_EARLY_S, life / 2);
      cached = { token: body.access_token, refreshAt: started + (life - early) * 1000 };
      return body.access_token;
    })()
      .catch((error: unknown) => {
        cached = undefined;
        throw error;
      })
      .finally(() => {
        inFlight = undefined;
      });
    return inFlight;
  };

  return {
    userId: file.userId,
    token: async () => {
      if (cached !== undefined && now() < cached.refreshAt) {
        return cached.token;
      }
      try {
        return await trade();
      } catch {
        // A failed trade sends no credential; `refresh()` reports the reason.
        return undefined;
      }
    },
    refresh: async () => {
      await trade();
    },
  };
}
