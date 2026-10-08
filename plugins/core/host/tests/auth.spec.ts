/**
 * Authentication against Authentik.
 *
 * These run against a REAL fake IdP over real HTTP, with the genuine `openid-client` and
 * `jose` doing the protocol work. That is a deliberate constraint: every trap being tested
 * for is caught somewhere inside those libraries' error paths, so a mock of them would sit
 * exactly on top of the code that has to survive them.
 *
 * The suite is organised by the failure mode rather than by the unit under test, because
 * the failure modes are the deliverable: an operator's session either works or produces a
 * message that names the Authentik setting to change.
 */

import { describe, it, expect, afterEach, beforeEach } from "vite-plus/test";
import { SignJWT, generateKeyPair, exportJWK, type JWK } from "jose";
import { startFakeIdP, CLIENT_ID, type FakeIdP, type FakeIdPOptions } from "./fake-idp.js";
import { captureLogs, createAuthApp, flushLogs, logText, type AuthApp } from "./auth-harness.js";
import { AuthError } from "../src/auth/errors.js";
import { loadAuthConfig, resolveAllowedOrigins, validateAuthConfig } from "../src/auth/config.js";
import { MemoryAuthStore, type AuthSessionRecord } from "../src/auth/session.js";
import type { PluginManifest } from "../src/types.js";

/** Every IdP start is torn down, even when an assertion throws. */
let idp: FakeIdP | undefined;
let app: AuthApp | undefined;

afterEach(async () => {
  await app?.close();
  await idp?.close();
  app = undefined;
  idp = undefined;
});

/** An IdP whose grants include the admin scope, which most admin tests need. */
const ADMIN_GRANTS = ["openid", "profile", "email", "offline_access", "bi:admin"];

async function boot(
  options: FakeIdPOptions & { env?: Record<string, string | undefined> } = {},
): Promise<{ idp: FakeIdP; app: AuthApp }> {
  idp = await startFakeIdP({ grantedScopes: ADMIN_GRANTS, ...options });
  app = await createAuthApp({ idp, env: options.env });
  return { idp, app };
}

function plugin(overrides: Partial<PluginManifest> & { id: string }): PluginManifest {
  return {
    name: overrides.id,
    description: `${overrides.id} plugin`,
    version: "1.0.0",
    uiPath: `/plugins/${overrides.id}`,
    ...overrides,
  } as PluginManifest;
}

/* ========================================================================== */
describe("configuration errors are legible, not generic 500s", () => {
  it("reports the trailing-slash requirement when OIDC_ISSUER lacks one", async () => {
    // Pure-config path: this is caught at boot, before anybody tries to log in.
    const config = loadAuthConfig({
      OIDC_ISSUER: "https://idp.example.com/application/o/bi",
      OIDC_CLIENT_ID: CLIENT_ID,
    });
    expect(config.enabled).toBe(true);
    let thrown: AuthError | undefined;
    try {
      validateAuthConfig(config);
    } catch (err) {
      thrown = err as AuthError;
    }
    expect(thrown).toBeInstanceOf(AuthError);
    expect(thrown?.code).toBe("oidc_issuer_not_slash_terminated");
    expect(thrown?.message).toContain("trailing slash");
    // It must name the concrete Authentik URL shape, not just complain.
    expect(thrown?.message).toContain("/application/o/<slug>/");
  });

  it("surfaces a discovery issuer mismatch as a configuration error, not a 500", async () => {
    // The IdP's metadata advertises an issuer WITHOUT the trailing slash while we are
    // configured with it -- the exact deployment mistake, since the two look identical to
    // an operator. openid-client compares them byte for byte and throws. Set before boot,
    // because discovery is memoised.
    idp = await startFakeIdP({ grantedScopes: ADMIN_GRANTS });
    idp.setOptions({ issuerOverride: idp.issuer.replace(/\/$/, "") });
    app = await createAuthApp({ idp });

    const response = await app.fetch("/api/auth/login");
    expect(response.status).toBe(500);
    const body = await response.json();
    expect(body.code).toBe("oidc_issuer_mismatch");
    // The whole point: legible, and naming what to change.
    expect(body.error).toContain("byte for byte");
    expect(body.error).toContain("trailing slash");
    // An AuthError status, not a generic internal failure.
    expect(body.error).not.toMatch(/no applicable keys/i);
  });

  it("rejects a redirect URI carrying a query string", () => {
    const config = loadAuthConfig({
      OIDC_ISSUER: "https://idp.example.com/application/o/bi/",
      OIDC_CLIENT_ID: CLIENT_ID,
      OIDC_REDIRECT_URI: "https://app.example.com/api/auth/callback?tenant=acme",
    });
    expect(() => validateAuthConfig(config)).toThrowError(/path-only/);
  });

  it("refuses a wildcard CORS origin, which would make the cookie unusable", () => {
    const config = loadAuthConfig({
      OIDC_ISSUER: "https://idp.example.com/application/o/bi/",
      OIDC_CLIENT_ID: CLIENT_ID,
      CORS_ALLOWED_ORIGINS: "*",
    });
    expect(() => resolveAllowedOrigins(config)).toThrowError(/may not contain/);
  });

  it("always allows the redirect URI's own origin", () => {
    const config = loadAuthConfig({
      OIDC_ISSUER: "https://idp.example.com/application/o/bi/",
      OIDC_CLIENT_ID: CLIENT_ID,
      OIDC_REDIRECT_URI: "https://app.example.com/api/auth/callback",
    });
    expect(resolveAllowedOrigins(config)).toContain("https://app.example.com");
  });
});

/* ========================================================================== */
describe("the HS256 / missing-signing-key trap", () => {
  it("names the RS256 certificate-key pair when the JWKS publishes no asymmetric key", async () => {
    // What Authentik serves when no Signing Key is configured.
    const { app: server } = await boot({ jwks: "empty" });

    const response = await server.fetch("/api/auth/login");
    expect(response.status).toBe(500);
    const body = await response.json();
    expect(body.code).toBe("oidc_no_signing_key");
    expect(body.error).toContain("Certificate-key pair (RS256)");
    // It explains WHY, because "your IdP is broken" is not an actionable message.
    expect(body.error).toContain("HS256");
    expect(body.error).toContain("no applicable keys");
  });

  it("names the RS256 certificate-key pair when the JWKS is symmetric-only", async () => {
    const { app: server } = await boot({ jwks: "oct" });
    const response = await server.fetch("/api/auth/login");
    const body = await response.json();
    expect(body.code).toBe("oidc_no_signing_key");
    expect(body.error).toContain("Certificate-key pair (RS256)");
  });

  it("names the RS256 certificate-key pair when the provider only advertises HS256", async () => {
    const { app: server } = await boot({ idTokenAlg: "HS256" });
    const response = await server.fetch("/api/auth/login");
    const body = await response.json();
    expect(body.error).toContain("Certificate-key pair (RS256)");
  });

  it("reports an unverifiable HS256-signed ID token as a configuration error", async () => {
    // The full end-to-end version: the JWKS is perfectly valid RS256, so the probe passes
    // and the failure happens at signature-verification time inside openid-client, exactly
    // as it does in production when an operator swaps in an HS256 provider by hand.
    const { app: server } = await boot({
      jwks: "rsa",
      idTokenAlg: "HS256",
      advertisedAlgs: ["RS256", "HS256"],
    });

    const outcome = await server.login();
    expect(outcome.callbackResponse.status).toBe(500);
    expect(outcome.callbackBody.code).toBe("oidc_unverifiable_token");
    expect(outcome.callbackBody.error).toContain("Certificate-key pair (RS256)");
    expect(outcome.callbackBody.error).toContain("cannot verify HS256 ID tokens");
  });
});

/* ========================================================================== */
describe("the offline_access trap", () => {
  it("warns about the scope mapping when no refresh token comes back", async () => {
    const { app: server } = await boot({ omitRefreshToken: true });

    const logs = await captureLogs(server);

    const outcome = await server.login();
    await flushLogs();
    // The login still SUCCEEDS. This is the trap: nothing errors, the session just cannot
    // ever be renewed.
    expect(outcome.callbackResponse.status).toBe(302);
    const text = logText(logs);
    expect(text).toContain("offline_access");
    expect(text).toContain("Scope Mapping");
    expect(text).toContain("intersects");
  });

  it("does not send `scope` on refresh unless it contains offline_access", async () => {
    const { app: server, idp: fake } = await boot();
    await server.login();

    const store = server.store;
    await expireAccessToken(store);
    const [session] = await allSessions(store);
    await server.auth!.ensureFreshSession(session);

    const refresh = fake.requests.token.find((entry) => entry.grantType === "refresh_token");
    expect(refresh).toBeDefined();
    // offline_access IS in our scope, so we are allowed to send it -- and must, or the
    // provider may narrow the grant and drop the refresh token.
    expect(refresh!.scope).toContain("offline_access");
  });

  it("omits `scope` on refresh when offline_access was never requested", async () => {
    const { app: server, idp: fake } = await boot({
      env: { OIDC_SCOPE: "openid profile email" },
    });
    await server.login();

    await expireAccessToken(server.store);
    const [session] = await allSessions(server.store);
    await server.auth!.ensureFreshSession(session);

    const refresh = fake.requests.token.find((entry) => entry.grantType === "refresh_token");
    expect(refresh).toBeDefined();
    // Re-sending a scope set without offline_access can make the provider drop the refresh
    // token, so the parameter must be omitted entirely rather than sent narrow.
    expect(refresh!.scope).toBeUndefined();
  });
});

/* ========================================================================== */
describe("the protocol is Authorization Code + PKCE S256 + state + nonce", () => {
  it("sends all of PKCE S256, state and nonce, and uses client_secret_basic", async () => {
    const { app: server, idp: fake } = await boot();

    await server.login();

    const authorize = fake.requests.authorize[0];
    expect(authorize.codeChallengeMethod).toBe("S256");
    expect(authorize.codeChallenge).toBeTruthy();
    expect(authorize.state).toBeTruthy();
    expect(authorize.nonce).toBeTruthy();
    expect(authorize.responseType).toBe("code");

    const token = fake.requests.token[0];
    // v6 defaults to ClientSecretPost; RFC 6749 2.3.1 prefers Basic, so we are explicit.
    expect(token.authMethod).toBe("client_secret_basic");
    expect(token.codeVerifier).toBeTruthy();
    // Path-only, matching what validateAuthConfig enforces.
    expect(token.redirectUri).toBe(`${server.baseUrl}/api/auth/callback`);
  });

  it("rejects a callback whose state was never issued", async () => {
    const { app: server } = await boot();
    const response = await server.fetch("/api/auth/callback?code=code-1&state=forged-state");
    expect(response.status).toBe(400);
    const body = await response.json();
    expect(body.code).toBe("oauth_unknown_state");
  });

  it("consumes state once, so a replayed callback cannot reuse it", async () => {
    const { app: server } = await boot();

    // Driven by hand so the exact callback URL can be presented a second time.
    const authorizeUrl = (await server.fetch("/api/auth/login")).headers.get("location")!;
    const callbackUrl = (await fetch(authorizeUrl, { redirect: "manual" })).headers.get(
      "location",
    )!;
    expect(callbackUrl).toBeTruthy();

    const first = await server.fetch(callbackUrl);
    expect(first.status).toBe(302);
    expect(first.headers.getSetCookie()[0]).toContain("bi_session=");

    // The SAME callback URL again: state was consumed, so nothing can match it.
    const replay = await server.fetch(callbackUrl);
    expect(replay.status).toBe(400);
    expect((await replay.json()).code).toBe("oauth_unknown_state");
  });

  it("never returns a token to the browser", async () => {
    const { app: server } = await boot();
    await server.login();
    const me = await server.fetch("/api/auth/me");
    const raw = await me.text();
    expect(raw).not.toContain("access_token");
    expect(raw).not.toContain("id_token");
    expect(raw).not.toContain("refresh_token");
    expect(raw).not.toContain("eyJ"); // a JWT starts with this
  });
});

/* ========================================================================== */
describe("the session cookie", () => {
  it("is HttpOnly, Secure and SameSite=Lax -- and specifically NOT Strict", async () => {
    const { app: server } = await boot({ env: { SESSION_COOKIE_SECURE: "true" } });
    const outcome = await server.login();

    const cookie = outcome.setCookie!;
    expect(cookie).toContain("HttpOnly");
    expect(cookie).toContain("Secure");
    expect(cookie).toContain("SameSite=Lax");
    // Strict would withhold the cookie on the top-level GET that the code flow depends
    // on, so the session would silently never be established.
    expect(cookie).not.toContain("SameSite=Strict");
    expect(cookie).toContain("Path=/");
  });

  it("carries an opaque id, and stores only its hash", async () => {
    const { app: server } = await boot();
    const outcome = await server.login();
    const cookieValue = outcome.setCookie!.split(";")[0].split("=")[1];
    expect(cookieValue).toBeTruthy();
    expect(cookieValue.length).toBeGreaterThanOrEqual(32);

    const [session] = await allSessions(server.store);
    // The stored key is a hash, not the cookie value.
    expect(session.idHash).not.toBe(cookieValue);
    expect(session.idHash).toHaveLength(64);
  });

  it("is cleared on logout", async () => {
    const { app: server } = await boot();
    await server.login();
    const response = await server.fetch("/api/auth/logout");
    const cookie = response.headers.getSetCookie()[0];
    expect(cookie).toContain("bi_session=");
    expect(cookie).toMatch(/Max-Age=0|Expires=Thu, 01 Jan 1970/);
    expect(await allSessions(server.store)).toHaveLength(0);
  });
});

/* ========================================================================== */
describe("requiredScopes is enforced at REQUEST time", () => {
  const scoped = plugin({ id: "zulip", requiredScopes: ["bi:zulip"] });

  it("blocks a plugin whose scopes the session lacks", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(scoped);

    // Logged in, but WITHOUT bi:zulip.
    const list = await server.fetch("/api/plugins");
    const body = await list.json();
    const entry = body.plugins.find((p: any) => p.id === "zulip");
    expect(entry.state).toBe("blocked");
    expect(entry.missingScopes).toEqual(["bi:zulip"]);

    // And its routes refuse rather than answer.
    const ping = await server.fetch("/api/zulip/ping");
    expect(ping.status).toBe(403);
    expect((await ping.json()).error).toContain("bi:zulip");
  });

  it("lets the same plugin through once the session holds the scope", async () => {
    const { app: server, idp: fake } = await boot({
      grantedScopes: [...ADMIN_GRANTS, "bi:zulip"],
    });
    await server.registerPlugin(scoped);
    await server.login();

    const ping = await server.fetch("/api/zulip/ping");
    expect(ping.status).toBe(200);
    expect(fake.requests.token.length).toBeGreaterThan(0);
  });

  it("does NOT let an earlier enable stand in for a session created afterwards", async () => {
    // The plugin is deployed and enabled BEFORE the user ever logs in. The session is
    // therefore minted without bi:zulip, because it could not have had it.
    const { app: server } = await boot();
    await server.registerPlugin(scoped);
    // Enabled at boot (defaultEnabled) with nobody logged in.
    expect(server.host.isLoaded("zulip")).toBe(true);

    await server.login();

    // Enabling earlier changed nothing: the decision is made against THIS session.
    const ping = await server.fetch("/api/zulip/ping");
    expect(ping.status).toBe(403);
    const body = await (await server.fetch("/api/plugins")).json();
    const entry = body.plugins.find((p: any) => p.id === "zulip");
    expect(entry.state).toBe("blocked");
    expect(entry.missingScopes).toEqual(["bi:zulip"]);
  });

  it("an enable by a fully-privileged admin does not help a lesser session afterwards", async () => {
    // The strongest form of the request-time requirement: the plugin really IS enabled,
    // by a session that legitimately holds every scope. A second session without
    // bi:zulip must still be refused, because the decision is taken per request.
    const { app: server, idp: fake } = await boot({
      grantedScopes: [...ADMIN_GRANTS, "bi:zulip"],
    });
    await server.registerPlugin(plugin({ ...scoped, defaultEnabled: false }));

    await server.login();
    const enabled = await server.fetch("/api/plugins/zulip/enable", { method: "POST" });
    expect(enabled.status).toBe(200);
    expect((await enabled.json()).state).toBe("loaded");
    expect(server.host.isLoaded("zulip")).toBe(true);

    // That session can use it.
    expect((await server.fetch("/api/zulip/ping")).status).toBe(200);

    // A different session, created when the provider no longer grants bi:zulip.
    await server.fetch("/api/auth/logout");
    fake.setOptions({ grantedScopes: ADMIN_GRANTS });
    await server.login();

    // Still refused, even though the plugin is enabled and loaded.
    const ping = await server.fetch("/api/zulip/ping");
    expect(ping.status).toBe(403);
    expect((await ping.json()).error).toContain("bi:zulip");
    const listed = await (await server.fetch("/api/plugins")).json();
    expect(listed.plugins.find((p: any) => p.id === "zulip").state).toBe("blocked");
  });

  it("refuses to enable a plugin whose requiredScopes the caller lacks", async () => {
    const { app: server } = await boot({
      env: { OIDC_SCOPE: "openid profile email offline_access bi:admin" },
    });
    // defaultEnabled:false, so the plugin is NOT already loaded at boot and this really
    // is a test of enable() rather than of boot.
    await server.registerPlugin(
      plugin({ id: "zulip", requiredScopes: ["bi:zulip"], defaultEnabled: false }),
    );
    await server.login();

    const enabled = await server.fetch("/api/plugins/zulip/enable", { method: "POST" });
    expect(enabled.status).toBe(200);
    const body = await enabled.json();
    expect(body.state).toBe("blocked");
    expect(body.missingScopes).toEqual(["bi:zulip"]);
    // Crucially: it was not turned on.
    expect(server.host.isLoaded("zulip")).toBe(false);
  });
});

/* ========================================================================== */
describe("the console is admin-gated", () => {
  it("refuses enable without the admin scope, and allows it with", async () => {
    // The provider grants NO bi:admin, and we do not even ask for it.
    const { app: server } = await boot({
      grantedScopes: ["openid", "profile", "email", "offline_access"],
      env: { OIDC_SCOPE: "openid profile email offline_access" },
    });
    await server.registerPlugin(plugin({ id: "zulip", defaultEnabled: false }));

    // A session WITHOUT bi:admin.
    await server.login();

    const denied = await server.fetch("/api/plugins/zulip/enable", { method: "POST" });
    expect(denied.status).toBe(403);
    expect((await denied.json()).error).toContain("bi:admin");
    expect(server.host.isLoaded("zulip")).toBe(false);

    const deniedDisable = await server.fetch("/api/plugins/zulip/disable", { method: "POST" });
    expect(deniedDisable.status).toBe(403);

    // Re-login as an admin.
    await server.fetch("/api/auth/logout");
    idp!.setOptions({ grantedScopes: ADMIN_GRANTS });
    await server.login();

    const allowed = await server.fetch("/api/plugins/zulip/enable", { method: "POST" });
    expect(allowed.status).toBe(200);
    expect(server.host.isLoaded("zulip")).toBe(true);

    const allowedDisable = await server.fetch("/api/plugins/zulip/disable", { method: "POST" });
    expect(allowedDisable.status).toBe(200);
    expect(server.host.isLoaded("zulip")).toBe(false);
  });

  it("refuses anonymous callers entirely", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(plugin({ id: "zulip", defaultEnabled: false }));
    const response = await server.fetch("/api/plugins/zulip/enable", { method: "POST" });
    expect(response.status).toBe(401);
  });

  it("never grants admin powers when auth is not configured at all", async () => {
    // No OIDC_ISSUER/OIDC_CLIENT_ID: the app boots, and nothing is privileged.
    idp = await startFakeIdP({ grantedScopes: ADMIN_GRANTS });
    app = await createAuthApp({ idp, env: { OIDC_ISSUER: "", OIDC_CLIENT_ID: "" } });
    await app.registerPlugin(plugin({ id: "zulip", defaultEnabled: false }));

    expect(app.auth!.enabled).toBe(false);
    const response = await app.fetch("/api/plugins/zulip/enable", { method: "POST" });
    expect(response.status).toBe(403);
    expect((await response.json()).error).toContain("authentication is not configured");
  });
});

/* ========================================================================== */
describe("message:send accepts a session or a scoped service token", () => {
  const agent = plugin({
    id: "zulip",
    agent: {
      name: "Zulip Agent",
      description: "Reads Zulip.",
      version: "1.0.0",
      skills: [
        { id: "listChannels", name: "listChannels", description: "List channels." },
        { id: "sendMessage", name: "sendMessage", description: "Send a message." },
      ],
    },
  });

  function sendBody(agentId: string, skill: string) {
    return {
      message: {
        kind: "message",
        id: `m-${Math.random()}`,
        role: "user",
        parts: [{ kind: "data", data: { agent: agentId, skill } }],
      },
    };
  }

  async function post(app_: AuthApp, body: unknown, bearer?: string) {
    return app_.fetch("/a2a/v1/message:send", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        ...(bearer ? { Authorization: `Bearer ${bearer}` } : {}),
      },
      body: JSON.stringify(body),
    });
  }

  it("accepts a service token scoped to that agent's skills", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(agent);

    const { token } = await server.auth!.issueServiceToken({
      name: "ci-runner",
      agentId: "zulip",
      skills: ["listChannels"],
    });

    const ok = await post(server, sendBody("zulip", "listChannels"), token);
    expect(ok.status).toBe(200);
    expect((await ok.json()).status.state).toBe("completed");
  });

  it("rejects a service token scoped to a DIFFERENT agent", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(agent);
    await server.registerPlugin(
      plugin({ id: "forgejo", agent: { ...agent.agent!, name: "Forgejo Agent" } }),
    );

    const { token } = await server.auth!.issueServiceToken({
      name: "zulip-runner",
      agentId: "zulip",
      skills: ["listChannels"],
    });

    const denied = await post(server, sendBody("forgejo", "listChannels"), token);
    expect(denied.status).toBe(403);
    const body = await denied.json();
    expect(body.error.message).toContain("scoped to agent");
  });

  it("rejects a service token that lacks the specific skill", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(agent);

    const { token } = await server.auth!.issueServiceToken({
      name: "reader-only",
      agentId: "zulip",
      skills: ["listChannels"],
    });

    const denied = await post(server, sendBody("zulip", "sendMessage"), token);
    expect(denied.status).toBe(403);
    expect((await denied.json()).error.message).toContain("not granted skill");
  });

  it("rejects a service token aimed at a disabled agent", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(agent);

    const { token } = await server.auth!.issueServiceToken({
      name: "runner",
      agentId: "zulip",
      skills: ["listChannels"],
    });

    await server.registry.disable("zulip");
    const denied = await post(server, sendBody("zulip", "listChannels"), token);
    // Refused as "not loaded" rather than as a permissions problem: the agent does not
    // exist as far as the bus is concerned.
    expect(denied.status).toBe(404);
    expect((await denied.json()).error.message).toContain("not loaded");
  });

  it("accepts a user session, and refuses anonymous", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(agent);

    expect((await post(server, sendBody("zulip", "listChannels"))).status).toBe(401);

    await server.login();
    const ok = await post(server, sendBody("zulip", "listChannels"));
    expect(ok.status).toBe(200);
  });

  it("refuses a revoked service token", async () => {
    const { app: server } = await boot();
    await server.registerPlugin(agent);
    const created = await server.auth!.issueServiceToken({
      name: "runner",
      agentId: "zulip",
      skills: ["listChannels"],
    });
    expect((await post(server, sendBody("zulip", "listChannels"), created.token)).status).toBe(200);

    await server.store.revokeServiceToken(created.record.id);
    expect((await post(server, sendBody("zulip", "listChannels"), created.token)).status).toBe(401);
  });

  it("stores service tokens hashed, so the plaintext is unrecoverable", async () => {
    const { app: server } = await boot();
    const created = await server.auth!.issueServiceToken({ name: "runner", agentId: "zulip" });
    const [stored] = await server.store.listServiceTokens();
    expect(stored.tokenHash).not.toBe(created.token);
    expect(stored.tokenHash).toHaveLength(64);
  });
});

/* ========================================================================== */
describe("refresh rotates the token set atomically", () => {
  it("replaces the refresh token and re-reads rather than clobbering on a lost race", async () => {
    const { app: server } = await boot();
    await server.login();
    await expireAccessToken(server.store);
    const [before] = await allSessions(server.store);
    const originalRefreshToken = before.refreshToken!;
    expect(originalRefreshToken).toBeTruthy();

    await server.auth!.ensureFreshSession(before);

    const [after] = await allSessions(server.store);
    // Authentik's refresh_token_threshold defaults to 0: the token ALWAYS changes.
    expect(after.refreshToken).not.toBe(originalRefreshToken);
    expect(after.refreshToken).toBeTruthy();

    // Now simulate the lost race: the row already holds a newer token, and a straggler
    // tries to write the value it read. The compare-and-swap must refuse.
    const won = await server.store.rotateAuthSessionTokens(after.idHash, originalRefreshToken, {
      refreshToken: "stale-write-should-not-land",
    });
    expect(won).toBe(false);
    const [final] = await allSessions(server.store);
    expect(final.refreshToken).toBe(after.refreshToken);
  });

  it("destroys the WHOLE session on invalid_grant and forces re-login", async () => {
    const { app: server } = await boot();
    await server.login();
    await expireAccessToken(server.store);
    const [session] = await allSessions(server.store);

    idp!.setOptions({ tokenError: { error: "invalid_grant", request_id: "req-abc" } });

    await expect(server.auth!.ensureFreshSession(session)).rejects.toThrowError(
      /could not be renewed/,
    );
    // The row is gone, not merely stripped of tokens.
    expect(await allSessions(server.store)).toHaveLength(0);
    // And the cookie no longer resolves to a principal.
    const me = await server.fetch("/api/auth/me");
    expect((await me.json()).authenticated).toBe(false);
  });

  it("logs authentik's request_id, which is the only diagnostic handle", async () => {
    const { app: server } = await boot();
    await server.login();
    await expireAccessToken(server.store);
    const [session] = await allSessions(server.store);
    idp!.setOptions({ tokenError: { error: "invalid_grant", request_id: "req-xyz-123" } });

    const logs = await captureLogs(server);

    await expect(server.auth!.ensureFreshSession(session)).rejects.toThrow();
    await flushLogs();
    const text = logText(logs);
    expect(text).toContain("SUSPICIOUS_REQUEST");
    expect(text).toContain("invalid_grant");
    // authentik's request_id is the only handle an operator has on a token failure.
    expect(text).toContain("req-xyz-123");
  });
});

/* ========================================================================== */
describe("back-channel logout", () => {
  let idpKeys: { privateKey: unknown; jwk: JWK };
  const KID = "bc-key";

  beforeEach(async () => {
    const pair = await generateKeyPair("RS256", { extractable: true });
    idpKeys = {
      privateKey: pair.privateKey,
      jwk: { ...(await exportJWK(pair.publicKey)), kid: KID, alg: "RS256", use: "sig" },
    };
  });

  /** A `logout_token` minted by the fake IdP's key, with the claims under test. */
  async function logoutToken(overrides: Record<string, unknown> = {}): Promise<string> {
    const claims = {
      iss: idp!.issuer,
      aud: CLIENT_ID,
      iat: Math.floor(Date.now() / 1000),
      jti: `jti-${Math.random()}`,
      events: ["http://schemas.openid.net/event/backchannel-logout"],
      ...overrides,
    };
    return new SignJWT(claims)
      .setProtectedHeader({ alg: "RS256", kid: KID })
      .sign(idpKeys.privateKey as never);
  }

  function post(server: AuthApp, body: string) {
    return server.fetch("/api/auth/backchannel-logout", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body,
    });
  }

  it("validates a well-formed token and destroys the matching sessions", async () => {
    const { app: server } = await boot();
    await server.login();
    expect(await allSessions(server.store)).toHaveLength(1);

    // Publish the signing key at the fake IdP so verification succeeds.
    publishKey(idpKeys.jwk);
    const token = await logoutToken({ sub: "user-1" });

    const response = await post(server, `logout_token=${encodeURIComponent(token)}`);
    expect(response.status).toBe(200);
    expect(await allSessions(server.store)).toHaveLength(0);

    // The session really is dead.
    const me = await server.fetch("/api/auth/me");
    expect((await me.json()).authenticated).toBe(false);
  });

  it("rejects a token with the WRONG audience", async () => {
    const { app: server } = await boot();
    await server.login();
    publishKey(idpKeys.jwk);

    const token = await logoutToken({ aud: "some-other-client" });
    const response = await post(server, `logout_token=${encodeURIComponent(token)}`);
    // Refused. jwtVerify already enforces `aud`, so this is caught as a verification
    // failure rather than reaching our own audience check.
    expect(response.status).toBe(401);
    expect((await response.json()).code).toBe("logout_token_invalid");
    // The session must survive a rejected token.
    expect(await allSessions(server.store)).toHaveLength(1);
  });

  it("rejects a token with no logout EVENT claim", async () => {
    const { app: server } = await boot();
    await server.login();
    publishKey(idpKeys.jwk);

    // A perfectly valid ID token from the same issuer and audience. Without the `events`
    // check this would destroy a live session.
    const token = await logoutToken({
      events: undefined,
      sub: "user-1",
      nonce: "n",
      auth_time: 1,
    });
    const response = await post(server, `logout_token=${encodeURIComponent(token)}`);
    expect(response.status).toBe(400);
    const body = await response.json();
    expect(body.code).toBe("logout_missing_event");
    expect(body.error).toContain("ordinary ID token");
    expect(await allSessions(server.store)).toHaveLength(1);
  });

  it("rejects a token signed by an unknown key", async () => {
    const { app: server } = await boot();
    await server.login();
    publishKey(idpKeys.jwk);

    const stranger = await generateKeyPair("RS256", { extractable: true });
    const forged = await new SignJWT({
      iss: idp!.issuer,
      aud: CLIENT_ID,
      iat: Math.floor(Date.now() / 1000),
      jti: "forged",
      events: ["http://schemas.openid.net/event/backchannel-logout"],
      sub: "user-1",
    })
      .setProtectedHeader({ alg: "RS256", kid: KID })
      .sign(stranger.privateKey as never);

    const response = await post(server, `logout_token=${encodeURIComponent(forged)}`);
    expect(response.status).toBe(401);
    expect((await response.json()).code).toBe("logout_token_invalid");
    expect(await allSessions(server.store)).toHaveLength(1);
  });

  it("rejects a replayed token", async () => {
    const { app: server } = await boot();
    await server.login();
    publishKey(idpKeys.jwk);
    const token = await logoutToken({ sub: "user-1" });

    expect((await post(server, `logout_token=${encodeURIComponent(token)}`)).status).toBe(200);
    // Same token again: rejected as a replay rather than silently accepted.
    const replay = await post(server, `logout_token=${encodeURIComponent(token)}`);
    expect(replay.status).toBe(400);
    expect((await replay.json()).code).toBe("logout_token_replayed");
  });

  it("rejects a request with no logout_token at all", async () => {
    const { app: server } = await boot();
    const response = await post(server, "something=else");
    expect(response.status).toBe(400);
    expect((await response.json()).code).toBe("logout_missing_token");
  });
});

/* ========================================================================== */
describe("the groups claim", () => {
  it("reads PLAIN `groups`, with no namespace, and does not gate on email_verified", async () => {
    const { app: server } = await boot();
    await server.login();
    const me = await (await server.fetch("/api/auth/me")).json();

    expect(me.user.groups).toEqual(["bi-users", "bi-admins"]);
    // There is no goauthentik.io/... namespace for claims; the key is plain.
    expect(me.user).not.toHaveProperty("goauthentik.io/auth/claims/groups");
    // Authentik hard-codes this false since 2025.10, and nothing here reads it.
    expect(me.user.email).toBe("ada@example.com");
  });

  it("handles a missing groups claim without failing login", async () => {
    const { app: server } = await boot({ claims: { groups: undefined } });
    const outcome = await server.login();
    expect(outcome.callbackResponse.status).toBe(302);
    const me = await (await server.fetch("/api/auth/me")).json();
    expect(me.user.groups).toEqual([]);
  });
});

/* ========================================================================== */
describe("auth is disable-able", () => {
  it("boots unauthenticated with a warning when the IdP is unset", async () => {
    idp = await startFakeIdP({ grantedScopes: ADMIN_GRANTS });
    app = await createAuthApp({ idp, env: { OIDC_ISSUER: "", OIDC_CLIENT_ID: "" } });

    expect(app.auth!.enabled).toBe(false);
    // Everything is anonymous, and the console still works read-only.
    const me = await (await app.fetch("/api/auth/me")).json();
    expect(me.authEnabled).toBe(false);
    expect(me.authenticated).toBe(false);
    expect(me.isAdmin).toBe(false);
    const plugins = await (await app.fetch("/api/plugins")).json();
    expect(Array.isArray(plugins.plugins)).toBe(true);
  });

  it("mounts the plugin platform with no AuthService at all", async () => {
    idp = await startFakeIdP();
    app = await createAuthApp({ idp, withoutAuth: true });
    const response = await app.fetch("/api/plugins");
    expect(response.status).toBe(200);
  });
});

/* -------------------------------------------------------------------------- */

/**
 * Age a session so its access token reads as expired.
 *
 * `ensureFreshSession` deliberately no-ops while the token is still comfortably valid, so a
 * refresh test must make it genuinely near expiry or it asserts nothing at all.
 */
async function expireAccessToken(
  store: MemoryAuthStore,
  at = Date.now() - 5 * 60_000,
): Promise<void> {
  for (const session of store.listSessions()) {
    await store.rotateAuthSessionTokens(session.idHash, session.refreshToken ?? null, {
      accessTokenExpiresAt: at,
    });
  }
}

/** Every session row, via the store's own public listing. */
function allSessions(store: MemoryAuthStore): AuthSessionRecord[] {
  return store.listSessions();
}

/**
 * Publish an extra key at the fake IdP's JWKS.
 *
 * The back-channel verifier checks signatures against the DISCOVERED jwks_uri, so a token
 * minted with a test key has to be verifiable there. Overwriting the served set is the
 * honest way to do that: it is the IdP that would hold the key in production.
 */
function publishKey(jwk: JWK): void {
  idp!.setJwks([jwk]);
}
