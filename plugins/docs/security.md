# The security model

What is enforced where, and which traps are easy to get wrong.

This document is about _this_ repository's enforcement points. How to configure
an identity provider is in [auth-setup.md](auth-setup.md).

---

## The one-paragraph version

There is **no implicit trust anywhere**. A plugin declares what it needs
(`requiredScopes`); the host enforces that **per request**, against the
principal presenting credentials _right now_; and the console toggle is the only
thing that decides whether a plugin's code is running at all. Because "off" means
the Cordis fiber is disposed and the routes are gone, authorization on a
disabled plugin is not a special case — there is nothing left to authorize.

---

## Enforcement points

| Surface                                | Enforced in                                                                             | Behaviour                                                     |
| -------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| Plugin routes                          | `host.ts` → wraps every registered `RouteSpec` in a guard derived from `requiredScopes` | 403, naming the missing scopes                                |
| `GET /api/plugins`, `/api/plugins/:id` | `registry.ts` → `listFor` / `findFor`                                                   | `state: "blocked"` + `missingScopes`, **per session**         |
| `POST /api/plugins/:id/enable`         | `host.ts` → `enable(id, principal)`                                                     | returns `blocked`; the plugin is **not** loaded               |
| `POST /a2a/v1/message:send`            | `a2a.ts` → `sendMessage`                                                                | 403; also binds agent + skill for service tokens              |
| `POST /api/plugins/:id/disable`        | `api.ts` → `requireScope(request, ADMIN_SCOPE)`                                         | 403; `admin_unavailable` when auth is off                     |
| Agent cards                            | `a2a.ts`                                                                                | a disabled plugin's card and skills are refused as not-loaded |

### Why per request and not per enable

This is the single most important thing to understand before you change it.

An enable-time check looks correct and is wrong. A session created **before your
plugin was deployed** predates it, was never granted its scopes, and would sail
straight through the enable-time gate and then operate on the plugin for its
whole lifetime. `core/host/tests/auth.spec.ts` asserts exactly this: the
plugin is enabled by a fully-privileged admin, and a _later_ session without the
scope is still refused.

### Why `blocked` is computed, not stored

`blocked` is a property of the **(plugin, session) pair**. Storing it would make
one under-privileged user's console bleed into everyone else's, and would leave a
stale `blocked` behind after re-authentication. It is derived per request
instead.

---

## Sessions

Server-side, with an **opaque random id** in the cookie. The cookie carries
nothing but that id; the store keeps only `sha256(id)`.

**Not an encrypted cookie, deliberately.** An encrypted-cookie session cannot be
revoked server-side, and the plugin console must be able to revoke — "disable
this plugin now" has to be immediate. A stateless token would make that
non-immediate.

Cookie: `HttpOnly; Secure; SameSite=Lax; Path=/`.

**Lax, not Strict.** The authorization-code callback is a **top-level cross-site
GET** navigation. `SameSite=Strict` would withhold the cookie on exactly that
navigation, so login would appear to succeed and then leave the browser signed
out. Lax still blocks the CSRF-vulnerable cross-site POST, which is what the
flag is for.

### Refresh rotation is compare-and-swap, always

Upstream refresh thresholds commonly default to `0`, i.e. the token is _always_
renewed and the previous refresh token dies immediately. The stored token set is
therefore replaced **atomically**, via a compare-and-swap on the refresh token
(`rotateAuthSessionTokens`). Two concurrent requests refreshing one session would
otherwise both present the same token: one would win and the other would get
`invalid_grant` — an intermittent login loop that only reproduces under load.
The loser re-reads instead of clobbering the winner's newer token set.

### Refresh reuse destroys one session, not all of them

On `invalid_grant` from a refresh, the whole session row is destroyed and the
user is forced to log in again. Other sessions belonging to the same user are
deliberately left alone — the provider has not revoked them, so signing a user
out of every device would exceed the evidence.

**Watch the IdP's event log for the refresh-reuse event.** That is the intrusion
signal.

### Tokens never reach the SPA

Access tokens are always JWTs and always large (groups, email, picture, uid,
azp), so they stay server-side. `GET /api/auth/me` returns the user and scopes
and nothing else.

---

## Back-channel logout

openid-client v6 has **no** back-channel logout implementation, so
`src/auth/backchannel.ts` hand-rolls it against `jose`. Each of these checks
exists to stop a specific forgery, and they are not interchangeable:

- **`events` must contain `http://schemas.openid.net/event/backchannel-logout`.**
  Without it, any ordinary ID token from the same issuer and audience — a
  perfectly valid signed JWT — would be accepted as a logout token and destroy
  live sessions. **This is the most important check in the file.**
- **`aud` must equal our `client_id`**, so another client on the same IdP cannot
  log our users out.
- **`sub` or `sid` must be present.**
- Signature verified against the discovered **`jwks_uri`**, with `alg` taken from
  the JWK set rather than the header, so an attacker cannot choose the algorithm.
- **`iat` freshness and `jti` replay protection.**

`sid` is preferred over `sub` so that one subject signed in to two browsers has
only the ended session destroyed. A `sub`-only token is looked up by the **raw**
claim, never by the hashed `sessionKey` — comparing against the hash would
silently match nothing and the endpoint would still answer 200.

---

## Signature verification is opt-in, and it is enabled here

In the plain `response_type=code` flow, the ID token signature is **not**
verified by default: the client decodes the token and checks its claims (`iss`,
`aud`, `exp`, `nonce`), but verification happens only through
`enableNonRepudiationChecks`. The JARM, hybrid and implicit flows verify by
default, which is exactly why the omission is easy to miss — tests of _those_
flows pass while an authorization-code login silently trusts whatever the token
endpoint returned.

`core/host/tests/auth.spec.ts` proves this both ways: a token forged with an
unrelated key is **rejected**, and a genuine RS256 login still succeeds.

Do not remove that flag without those two tests still passing.

---

## Service tokens for `message:send`

`POST /a2a/v1/message:send` accepts a **user session** or a **per-agent service
token**. A service token is scoped to one `agentId` **and** to a subset of that
agent's **declared** skills. Both bounds are enforced: a token minted for agent A
cannot drive agent B, and cannot reach a skill A never declared — including one a
later plugin version adds.

Tokens are random, shown once, and stored hashed. An agent whose plugin is
disabled is refused as not-loaded.

---

## Admin gating

`POST /api/plugins/{enable,disable}` requires `ADMIN_SCOPE`. Enabling or
disabling a plugin reconfigures what the whole system can do, so it is gated
separately from the plugin's own `requiredScopes`.

**Admin powers are never granted while auth is disabled.** `auth.adminScope` is
`undefined` in that mode, so the toggle endpoints return 403 with
`admin_unavailable` — no principal can be _proven_ to hold the admin scope, and a
development server must not be able to reconfigure itself. This is why a fresh
checkout with no `OIDC_ISSUER` still shows the console but refuses the toggle.

---

## CORS

`Access-Control-Allow-Origin: *` has been **replaced by an explicit allowlist**
(`CORS_ALLOWED_ORIGINS`). This is mandatory rather than polish: per the CORS
specification a wildcard origin is **incompatible with credentialed requests**, so
while it stood the session cookie could never have been sent by a browser at all.

A wildcard in the configuration is **rejected at boot**. Allowed origins receive
`Access-Control-Allow-Credentials: true` and `Vary: Origin`; the redirect URI's
own origin is always allowed. Requests with no `Origin` header (curl,
same-origin) get no CORS headers, which is correct — CORS only constrains
browsers.

---

## Router ordering: plugins cannot shadow the platform

Plugin routes are consulted **after** the ConnectRPC bridge and the original REST
routes, so a plugin can never shadow either. A plugin that has been disabled falls
straight through to the server's 404 — and a 404 is the intended observable
proof that unloading worked, not an accident.

---

## Upstream read-only

Every adapter is a read-only HTTP client. See [upstreams.md](upstreams.md) for
the per-upstream argument. The security-relevant consequences:

- No upstream credential is ever sent anywhere but its own host.
- The Loams adapter sends **no** `Authorization` header by default, because
  Loams has no authentication today; its optional token exists for a
  proxy-fronted deployment.
- The Loams `/sql` endpoint is disabled unless `LOAMS_ALLOW_SQL` is explicitly
  `true` and has **no agent skill**, so editing config is the only way in. The
  statement is passed through verbatim — the read-only enforcement is real and
  server-side, and a client-side string check layered on top of it would
  manufacture false confidence.

---

## Things this repository does _not_ claim

- **Agent cards are unsigned.** The A2A card path is served unsigned today. Treat
  a card as an availability mechanism, not an integrity mechanism. See
  [a2a.md](a2a.md).
- **An adapter's "not configured" state is public.** `GET /api/plugins` names the
  environment variables each adapter wants. That is deliberate — a console that
  cannot say which variable is missing is useless — but it does mean the plugin
  list is not a secret. Run this behind an authenticating proxy if the shape of
  your deployment is sensitive.
- **Anonymous callers can enumerate agent ids** over `message:send`, because
  "unknown agent" is answered before authentication. They learn nothing about
  scopes. The trade is documented in [a2a.md](a2a.md).

---

## Reporting a vulnerability

See [SECURITY.md](../SECURITY.md). Not the public issue tracker.
