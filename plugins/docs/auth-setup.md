# Auth setup (Authentik OIDC)

Authentication is **optional in development and mandatory in production**. With
no `OIDC_ISSUER` and `OIDC_CLIENT_ID` set, the server logs why and runs
unauthenticated — every request is an anonymous principal, no session can be
created, and the console's enable/disable endpoints refuse. That is deliberate:
a development server must not be able to reconfigure itself.

The model itself is in [security.md](security.md). This document is about getting
an identity provider wired up correctly.

Implementation: `packages/core/src/auth/`. Tests:
`packages/core/tests/auth.spec.ts`, which run against a **real fake IdP over real
HTTP** using the genuine `openid-client` and `jose`. Every failure mode below is
caught inside those libraries, so mocking them would sit on top of the code that
has to survive them.

## Configuration

| Variable                        | Required | Default                                   | Notes                                                                  |
| ------------------------------- | -------- | ----------------------------------------- | ---------------------------------------------------------------------- |
| `OIDC_ISSUER`                   | yes      | —                                         | **Must end with `/`.** See trap 2.                                     |
| `OIDC_CLIENT_ID`                | yes      | —                                         | Globally unique across all Authentik providers.                        |
| `OIDC_CLIENT_SECRET`            | no       | —                                         | Sent as `client_secret_basic`. Omit for a public PKCE client.          |
| `OIDC_REDIRECT_URI`             | no       | `http://localhost:3001/api/auth/callback` | **Path only, no query string.**                                        |
| `OIDC_SCOPE`                    | no       | `openid profile email`                    | Must contain `openid`.                                                 |
| `OIDC_POST_LOGOUT_REDIRECT_URI` | no       | `http://localhost:3001/`                  | Must be registered as a **logout**-type redirect URI.                  |
| `OIDC_INSECURE`                 | no       | `false`                                   | Allows plain HTTP. Honoured **only** when `NODE_ENV !== 'production'`. |
| `ADMIN_SCOPE`                   | no       | `bi:admin`                                | Gates the plugin console's enable/disable. See the note below.         |
| `SESSION_COOKIE_NAME`           | no       | `bi_session`                              |                                                                        |
| `SESSION_COOKIE_SECURE`         | no       | `true`                                    | Set `false` only for plain-http local dev.                             |
| `SESSION_TTL_MS`                | no       | 12h                                       |                                                                        |
| `AUTH_REQUEST_TTL_MS`           | no       | 10 min                                    | How long a pending login stays valid.                                  |
| `OIDC_TIMEOUT_MS`               | no       | 10000                                     | Applied to discovery and every token request.                          |
| `SUB_MODE`                      | no       | `hashed_user_id`                          | Or `sub`.                                                              |
| `INSTALL_ID`                    | no       | `bi`                                      | Mixed into the hashed session key.                                     |
| `CORS_ALLOWED_ORIGINS`          | no       | —                                         | Comma-separated. `*` is **rejected**.                                  |

> **On the `bi:` prefixes.** `ADMIN_SCOPE`, `SESSION_COOKIE_NAME` and `INSTALL_ID`
> still default to names from this repository's earlier `@bi/*` era. They are
> overridable environment variables, so set them to something namespaced for your
> deployment. The defaults are kept rather than silently changed because
> `ADMIN_SCOPE` is asserted by name in issued tokens and in `auth.spec.ts`: a
> silent rename is a breaking change to every existing grant.

Confirm your provider publishes an asymmetric key:

```sh
curl -s "$OIDC_ISSUER/.well-known/openid-configuration" | jq -r .jwks_uri
curl -s "$(curl -s "$OIDC_ISSUER/.well-known/openid-configuration" | jq -r .jwks_uri)" \
  | jq '.keys[] | {kid, alg, kty}'   # want kty RSA, alg RS256
```

## The five silent-failure traps

None of these raise an error on the Authentik side. That is what makes them
dangerous, and why each is translated into a message that names the setting to
change.

### 1. No Signing Key ⇒ HS256, which is not verifiable

An Authentik OAuth2 provider with **no Signing Key** does not refuse to issue
tokens. It signs ID tokens with **HS256** using the client secret as the HMAC key
and **omits `kid`** entirely.

This code probes the JWKS right after discovery (`src/auth/jwks.ts`) and refuses
to continue if no asymmetric key is published, naming the fix:
`configure a Certificate-key pair (RS256) on your Authentik OAuth2 provider`.

### 2. `OIDC_ISSUER` must end with `/`

```
https://idp.example.com/application/o/<slug>/
```

openid-client compares the discovered `issuer` to the configured one **byte for
byte**, so a missing trailing slash fails every login at discovery with
`discovered metadata issuer does not match the expected issuer`. Nothing
normalises it; `validateAuthConfig` rejects it at boot instead.

Note that `authorize/`, `token/` and `userinfo/` are **global** to the Authentik
instance while `jwks/` and `end-session/` are **per-application**. Never construct
endpoint URLs by hand — discovery is the only supported source.

### 3. `offline_access` is silently intersected away

Authentik intersects the requested scope set with the provider's assigned scopes
and returns the remainder **with no error**. If `offline_access` is not mapped, it
simply disappears and no refresh token comes back.

Fix: add a **Scope Mapping** on the provider mapping `offline_access` →
`offline_access`. When no refresh token arrives, this code logs a warning naming
exactly that change.

### 4. `groups` is plain and unnamespaced, and DIRECT only

The claim is **`claims.groups`**, a `string[]` of group names. There is **no
`goauthentik.io/...` namespace for claims**; reading one returns `undefined`
forever.

It contains **direct memberships only** — recursion is a separate `all_groups()` —
so a user who belongs to a nested subgroup does **not** appear in the parent's
list. Use **leaf groups**, or add a custom scope mapping on the provider and read
the claim from there.

### 5. `email_verified` is hard-coded `false` (Authentik 2025.10+)

Never gate login or authorization on it. Nothing in this implementation reads it.

---

## Protocol

Authorization Code + PKCE **S256** + `state` + `nonce`. All three, always.

Authentik never _requires_ PKCE, which is precisely why it is not optional: an
admin could turn the provider setting off, and PKCE would then be the only
remaining binding between the returned code and the request that asked for it.

openid-client v6 semantics that bite:

- **`expectedState: undefined` ASSERTS that no state is present.** It is not
  "skip the check". Same for `expectedNonce: undefined`. Both are always passed
  through from the stored authorization request; `ExchangeParams` types them as
  `string`, so omitting them is not expressible.
- v6 defaults to `ClientSecretPost`. RFC 6749 §2.3.1 prefers `client_secret_basic`,
  so `ClientSecretBasic(secret)` is passed **explicitly**.
- The `redirect_uri` used at the token endpoint is derived by **stripping
  searchParams and the hash**, and a bare origin may gain a trailing slash. Hence:
  path-only, no query.
- HTTP timeout defaults to **30 s** for discovery and every later request. Set
  explicitly.
- `allowInsecureRequests` only when `OIDC_INSECURE=true` **and**
  `NODE_ENV !== 'production'`.
- **`enableNonRepudiationChecks` is on and must stay on.** In the plain
  authorization-code flow the ID token signature is otherwise never verified. See
  [security.md](security.md).

## Routes

| Method | Path                           | Notes                                                                           |
| ------ | ------------------------------ | ------------------------------------------------------------------------------- |
| `GET`  | `/api/auth/login`              | 302 to the authorization endpoint. `return_to` is **local-path only**.          |
| `GET`  | `/api/auth/callback`           | Exchanges the code, creates a session, 302 onward.                              |
| `GET`  | `/api/auth/logout`             | Ends the session, 302 to the provider's end-session endpoint.                   |
| `GET`  | `/api/auth/me`                 | `{ authenticated, authEnabled, user, scopes, isAdmin, adminScope }`. No tokens. |
| `POST` | `/api/auth/backchannel-logout` | Form-encoded `logout_token`; validates and destroys sessions.                   |

### Using it

```ts
const user = await ctx.auth.user(request); // AuthUser | undefined
const scopes = await ctx.auth.scopes(request); // string[]
await ctx.auth.hasScope(request, ADMIN_SCOPE); // boolean
await ctx.auth.requireUser(request); // AuthUser, else 401
await ctx.auth.requireScope(request, ADMIN_SCOPE); // AuthPrincipal, else 403 naming what is missing
```

All of these are **async**, deliberately: session state lives in the store, so
resolving a principal means a read. A synchronous API would need a second path
that answers "anonymous" on a request's first read and "user" on its second — a
race that produces a wrong authorization decision.

---

## Troubleshooting

**Every login fails at discovery with "issuer does not match".** Trap 2. The
trailing slash.

**Login appears to succeed and then the browser is signed out.** `SameSite`.
The default is `Lax` for exactly this reason; see [security.md](security.md).

**Sessions drop after a few minutes under load.** Refresh rotation is
compare-and-swap. If you are seeing intermittent `invalid_grant`, something is
presenting a stale refresh token — check for more than one server process
sharing a store without a lock.

**No refresh token ever comes back.** Trap 3.

**Groups are missing nested members.** Trap 4.

**Tokens are rejected as "no signing key".** Trap 1.

**`vp check` reports an error you did not introduce.** Run `vp env doctor` and
include the output when asking for help — that is what the tool's own review
checklist asks for.
