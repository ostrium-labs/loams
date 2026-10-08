/**
 * A fake Authentik, served over real HTTP.
 *
 * The point of this file is to NOT mock openid-client. Every trap this suite exists to
 * prove is caught somewhere between `discovery()` and `jwtVerify`, and a mock would sit
 * exactly on top of the code that catches them. So this speaks enough of the protocol for
 * the genuine client library to run against it: real discovery, a real JWKS, a real
 * authorization redirect, a real token exchange, and real RSA-signed ID tokens.
 *
 * Everything the tests need to assert about the requests we make is RECORDED on
 * `requests`, which is how "we sent PKCE S256 and not plain", "we sent
 * client_secret_basic", and "we did not send scope on refresh" become assertions rather
 * than assumptions.
 */

import http from "node:http";
import { SignJWT, exportJWK, generateKeyPair, type JWK } from "jose";

export const CLIENT_ID = "bi-test-client";
export const CLIENT_SECRET = "bi-test-client-secret-value";

const KID = "bi-test-key";
const ISSUER_PATH = "/application/o/test/";

export interface RecordedAuthorize {
  clientId?: string;
  redirectUri?: string;
  responseType?: string;
  scope?: string;
  state?: string;
  nonce?: string;
  codeChallenge?: string;
  codeChallengeMethod?: string;
}

export interface RecordedToken {
  grantType?: string;
  authorizationCode?: string;
  redirectUri?: string;
  codeVerifier?: string;
  refreshToken?: string;
  /** `undefined` means the parameter was absent, which is itself the assertion. */
  scope?: string;
  /** How the client authenticated. */
  authMethod?: string;
}

export interface FakeIdPOptions {
  /**
   * Serve `issuer` in the metadata instead of the configured one. Reproduces the
   * trailing-slash mistake: the operator points at the right host, the metadata does not
   * match byte for byte.
   */
  issuerOverride?: string;
  /** What the JWKS publishes. `empty` is Authentik with no Signing Key configured. */
  jwks?: "rsa" | "empty" | "oct";
  /**
   * How the ID token is signed. `HS256` with the client secret as the HMAC key and no
   * `kid` is EXACTLY what Authentik does when no Signing Key is configured.
   */
  idTokenAlg?: "RS256" | "HS256";
  /**
   * What `id_token_signing_alg_values_supported` advertises. Defaults to `[idTokenAlg]`.
   *
   * Overriding it to claim RS256 while actually signing HS256 reproduces the case the
   * metadata check CANNOT catch: a provider whose advertised algorithms look fine but
   * whose tokens still arrive unverifiable. The failure then has to be caught at
   * signature-verification time instead.
   */
  advertisedAlgs?: string[];
  /** Omit `id_token_signing_alg_values_supported`. */
  omitAlgMetadata?: boolean;
  /** Omit `end_session_endpoint`. */
  omitEndSession?: boolean;
  /** Fail the token endpoint with this OAuth error instead of issuing tokens. */
  tokenError?: { error: string; description?: string; request_id?: string };
  /** Omit the refresh token, reproducing the `offline_access` intersection trap. */
  omitRefreshToken?: boolean;
  /** Scopes the provider claims to grant, in the token response's `scope` field. */
  grantedScopes?: string[];
  /** Extra claims for the ID token. */
  claims?: Record<string, unknown>;
  /** Advertise code challenge methods here. */
  supportedCodeChallengeMethods?: string[];
  /**
   * The HMAC key used for HS256 signing. Defaults to the client secret; override with a
   * DIFFERENT value to forge a token the verifier must reject.
   */
  hs256Secret?: string;
}

export interface FakeIdP {
  /** The issuer URL, WITH the trailing slash. */
  issuer: string;
  url: string;
  requests: { authorize: RecordedAuthorize[]; token: RecordedToken[]; jwks: number };
  setOptions(options: FakeIdPOptions): void;
  /**
   * Replace the published JWKS. Back-channel logout tokens are minted with a test key,
   * and the verifier checks them against the DISCOVERED jwks_uri, so the key has to be
   * verifiable there -- which is exactly right, because in production that is where the
   * IdP holds it.
   */
  setJwks(keys: JWK[]): void;
  close(): Promise<void>;
}

export async function startFakeIdP(options: FakeIdPOptions = {}): Promise<FakeIdP> {
  const { publicKey, privateKey } = await generateKeyPair("RS256", { extractable: true });
  const rsaJwk: JWK = { ...(await exportJWK(publicKey)), kid: KID, alg: "RS256", use: "sig" };

  let current: FakeIdPOptions = { ...options };
  /** Overrides the RSA key when a test publishes its own. */
  let publishedKeys: JWK[] | undefined;
  const requests: { authorize: RecordedAuthorize[]; token: RecordedToken[]; jwks: number } = {
    authorize: [],
    token: [],
    jwks: 0,
  };

  /** authorization code -> the nonce and challenge it was issued against. */
  const codeToState = new Map<string, { nonce?: string; challenge?: string }>();
  /** refresh token -> the ID token claims it was minted with, so a refresh echoes them. */
  const pendingByRefresh = new Map<string, Record<string, unknown>>();
  let counter = 0;

  let base = "";

  const issuerUrl = (): string => `${base}${ISSUER_PATH}`;
  const metadataIssuer = (): string => current.issuerOverride ?? issuerUrl();

  async function signIdToken(claims: Record<string, unknown>): Promise<string> {
    if ((current.idTokenAlg ?? "RS256") === "HS256") {
      // Authentik's no-Signing-Key fallback: HMAC keyed on the client secret, no `kid`.
      return new SignJWT(claims)
        .setProtectedHeader({ alg: "HS256", typ: "JWT" })
        .sign(new TextEncoder().encode(current.hs256Secret ?? CLIENT_SECRET));
    }
    return new SignJWT(claims)
      .setProtectedHeader({ alg: "RS256", kid: KID, typ: "JWT" })
      .sign(privateKey);
  }

  function defaultIdTokenClaims(): Record<string, unknown> {
    return {
      iss: metadataIssuer(),
      aud: CLIENT_ID,
      sub: "user-1",
      azp: CLIENT_ID,
      sid: "sid-1",
      iat: Math.floor(Date.now() / 1000),
      exp: Math.floor(Date.now() / 1000) + 300,
      email: "ada@example.com",
      // Hard-coded false by Authentik since 2025.10. Present here precisely so a test can
      // prove nothing in this codebase gates on it.
      email_verified: false,
      name: "Ada Lovelace",
      preferred_username: "ada",
      // PLAIN and unnamespaced. There is no goauthentik.io/... namespace for claims.
      groups: ["bi-users", "bi-admins"],
    };
  }

  const server = http.createServer((req, res) => {
    void handle(req, res).catch(() => {
      if (!res.headersSent) json(res, 500, { error: "server_error" });
    });
  });

  async function handle(req: http.IncomingMessage, res: http.ServerResponse): Promise<void> {
    const url = new URL(req.url ?? "/", base);
    const path = url.pathname;

    if (path === `${ISSUER_PATH}.well-known/openid-configuration`) {
      const body: Record<string, unknown> = {
        issuer: metadataIssuer(),
        authorization_endpoint: `${base}/authorize`,
        token_endpoint: `${base}/token`,
        jwks_uri: `${base}/jwks${ISSUER_PATH}`,
        response_types_supported: ["code"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        subject_types_supported: ["public"],
        scopes_supported: ["openid", "profile", "email", "offline_access", "bi:admin"],
        code_challenge_methods_supported: current.supportedCodeChallengeMethods ?? ["S256"],
        token_endpoint_auth_methods_supported: ["client_secret_basic", "client_secret_post"],
        claims_supported: ["sub", "email", "name", "preferred_username", "groups", "sid"],
      };
      if (!current.omitAlgMetadata) {
        body.id_token_signing_alg_values_supported = current.advertisedAlgs ?? [
          current.idTokenAlg ?? "RS256",
        ];
      }
      if (!current.omitEndSession) body.end_session_endpoint = `${base}/end-session`;
      return json(res, 200, body);
    }

    if (path.startsWith("/jwks")) {
      requests.jwks += 1;
      if (current.jwks === "empty") return json(res, 200, { keys: [] });
      if (current.jwks === "oct") {
        return json(res, 200, { keys: [{ kty: "oct", alg: "HS256", kid: "sym" }] });
      }
      return json(res, 200, { keys: publishedKeys ?? [rsaJwk] });
    }

    if (path === "/authorize") {
      const state = url.searchParams.get("state") ?? "";
      const nonce = url.searchParams.get("nonce") ?? undefined;
      const challenge = url.searchParams.get("code_challenge") ?? undefined;
      requests.authorize.push({
        clientId: url.searchParams.get("client_id") ?? undefined,
        redirectUri: url.searchParams.get("redirect_uri") ?? undefined,
        responseType: url.searchParams.get("response_type") ?? undefined,
        scope: url.searchParams.get("scope") ?? undefined,
        state: url.searchParams.get("state") ?? undefined,
        nonce: url.searchParams.get("nonce") ?? undefined,
        codeChallenge: challenge,
        codeChallengeMethod: url.searchParams.get("code_challenge_method") ?? undefined,
      });
      const redirect = url.searchParams.get("redirect_uri");
      if (!redirect) return json(res, 400, { error: "invalid_request" });
      counter += 1;
      const code = `code-${counter}`;
      codeToState.set(code, { nonce, challenge });
      const target = new URL(redirect);
      target.searchParams.set("code", code);
      if (state) target.searchParams.set("state", state);
      res.writeHead(302, { Location: target.href });
      res.end();
      return;
    }

    if (path === "/token") {
      const form = new URLSearchParams(await readBody(req));
      const header = req.headers.authorization;
      const authMethod =
        typeof header === "string" && header.startsWith("Basic ")
          ? "client_secret_basic"
          : form.get("client_secret")
            ? "client_secret_post"
            : undefined;

      const grantType = form.get("grant_type");
      const code = form.get("code");
      const refreshToken = form.get("refresh_token") ?? undefined;
      requests.token.push({
        grantType: grantType ?? undefined,
        authorizationCode: code ?? undefined,
        redirectUri: form.get("redirect_uri") ?? undefined,
        codeVerifier: form.get("code_verifier") ?? undefined,
        refreshToken,
        scope: form.get("scope") ?? undefined,
        authMethod,
      });

      if (current.tokenError) return json(res, 400, current.tokenError);

      // A refresh replays the ORIGINAL claims and the ORIGINAL nonce. Re-minting a nonce
      // here would let a test pass that production must not.
      const priorClaims = refreshToken ? pendingByRefresh.get(refreshToken) : undefined;
      const codeState = code ? codeToState.get(code) : undefined;

      // The nonce is bound at /authorize time and simply echoed at /token time, which is
      // what a real AS does. On refresh it is the ORIGINAL nonce, replayed.
      const nonce =
        grantType === "refresh_token"
          ? ((priorClaims?.nonce as string | undefined) ?? undefined)
          : (codeState?.nonce ?? undefined);

      const claims: Record<string, unknown> = {
        ...defaultIdTokenClaims(),
        ...(grantType === "refresh_token" ? stripEphemeral(priorClaims ?? {}) : {}),
        ...(nonce ? { nonce } : {}),
        ...current.claims,
      };

      const idToken = await signIdToken(claims);
      const body: Record<string, unknown> = {
        access_token: `at-${Date.now()}-${Math.random().toString(36).slice(2)}`,
        token_type: "Bearer",
        expires_in: 300,
        scope: (current.grantedScopes ?? ["openid", "profile", "email", "offline_access"]).join(
          " ",
        ),
        id_token: idToken,
      };
      if (!current.omitRefreshToken) {
        counter += 1;
        const next = `rt-${counter}-${Date.now()}-${Math.random().toString(36).slice(2)}`;
        pendingByRefresh.set(next, claims);
        body.refresh_token = next;
      }
      return json(res, 200, body);
    }

    if (path === "/end-session") {
      res.writeHead(302, { Location: "/" });
      res.end();
      return;
    }

    return json(res, 404, { error: "not_found", path });
  }

  function stripEphemeral(claims: Record<string, unknown>): Record<string, unknown> {
    const copy = { ...claims };
    delete copy.iss;
    delete copy.aud;
    delete copy.azp;
    delete copy.iat;
    delete copy.exp;
    return copy;
  }

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${(server.address() as { port: number }).port}`;

  return {
    issuer: `${base}${ISSUER_PATH}`,
    url: base,
    requests,
    setOptions(next) {
      current = { ...current, ...next };
    },
    setJwks(keys) {
      publishedKeys = keys;
    },
    async close() {
      await new Promise<void>((resolve) => server.close(() => resolve()));
    },
  };
}

function json(res: http.ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

function readBody(req: http.IncomingMessage): Promise<string> {
  return new Promise((resolve) => {
    let body = "";
    req.on("data", (chunk) => {
      body += chunk.toString("utf-8");
    });
    req.on("end", () => resolve(body));
  });
}

/** Read a JWT payload WITHOUT verifying it. Test-only. */
export function decodeJwtPayload(token: string): Record<string, unknown> | undefined {
  try {
    const part = token.split(".")[1];
    if (!part) return undefined;
    return JSON.parse(Buffer.from(part, "base64url").toString("utf-8")) as Record<string, unknown>;
  } catch {
    return undefined;
  }
}
