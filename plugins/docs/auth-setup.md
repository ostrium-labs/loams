# Auth setup (OIDC)

Authentication is **optional in development and mandatory in production**. With
no `OIDC_ISSUER` and `OIDC_CLIENT_ID` set, the server logs why and runs
unauthenticated — every request is an anonymous principal, no session can be
created, and the console's enable/disable endpoints refuse. That is deliberate:
a development server must not be able to reconfigure itself.

The model itself is in [security.md](security.md). This document is about getting
an identity provider wired up correctly. Any OpenID Connect provider works. Loams'
own is Zitadel at `auth.loams.dev`, which is planned (IA1, design 76) and not
deployed yet; until then, use any OIDC provider you run or trust.

Implementation: `core/host/src/auth/`. Tests:
`core/host/tests/auth.spec.ts`, which run against a **real fake IdP over real
HTTP** using the genuine `openid-client` and `jose`. Every failure mode below is
caught inside those libraries, so mocking them would sit on top of the code that
has to survive them.

## Configuration

| Variable                        | Required | Default                                   | Notes                                                                  |
| ------------------------------- | -------- | ----------------------------------------- | ---------------------------------------------------------------------- |
| `OIDC_ISSUER`                   | yes      | —                                         | Exactly the discovery document's `issuer`. See trap 2.                 |
| `OIDC_CLIENT_ID`                | yes      | —                                         | Unique at the identity provider.                                       |
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

Providers differ, and some of them accept each of these mistakes without an
error. That is what makes them dangerous, and why each is translated into a
message that names the setting to change.

### 1. No asymmetric signing key ⇒ HS256, which is not verifiable

A provider client with no asymmetric signing key may still issue tokens, signed
**HS256** with the client secret as the HMAC key and with **no `kid`**.

This code probes the JWKS right after discovery (`src/auth/jwks.ts`) and refuses
to continue if no asymmetric key is published. The fix is to give the client an
RS256 key at the provider.

### 2. `OIDC_ISSUER` must match discovery byte for byte

The `issuer` in the provider's discovery document must equal `OIDC_ISSUER`
**byte for byte** (OIDC Discovery 4.3). A difference as small as a trailing
slash counts: `https://idp.example.com/x/` and `https://idp.example.com/x` are
different issuers, and so are `https://auth.loams.dev` and
`https://auth.loams.dev/`. No slash convention is imposed and nothing normalises
the value; some providers' issuers end in `/`, others (a bare origin) do not.
Discovery is fetched from `<issuer>/.well-known/openid-configuration` with no
double slash either way, and on a mismatch every login fails with
`oidc_issuer_mismatch`, naming both values. Copy the `issuer` field from that
document. At boot `validateAuthConfig` only checks that `OIDC_ISSUER` is an
absolute http(s) URL with no query string and no fragment.

Never construct endpoint URLs by hand: discovery is the only supported source.

### 3. `offline_access` can be dropped without an error

A provider may intersect the requested scope set with what the client is allowed
and return the remainder **with no error**. If `offline_access` is not allowed,
it simply disappears and no refresh token comes back.

Fix: allow refresh tokens (`offline_access`) for the client at the provider.
When no refresh token arrives, this code logs a warning saying so.

### 4. `groups` is read plain and unnamespaced

The claim read is **`claims.groups`**, a `string[]` of group names. A provider
that puts groups or roles under a namespaced claim of its own is not read, and
the principal has no groups.

Whether nested group memberships appear in `groups` depends on the provider.
Use **leaf groups**, or map the claim you need into `groups` at the provider.

### 5. `email_verified` is not trustworthy everywhere

Some providers always send `false`. Never gate login or authorization on it.
Nothing in this implementation reads it.

---

## Protocol

Authorization Code + PKCE **S256** + `state` + `nonce`. All three, always.

A provider may not _require_ PKCE, which is precisely why it is not optional: an
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

**Every login fails at discovery with "issuer mismatch".** Trap 2. Compare
`OIDC_ISSUER` with the discovery document's `issuer`, trailing slash included.

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
