# Zitadel for auth.loams.dev

> **Status: planned (IA1).** Nothing on this page is deployed yet, and there is no deploy bundle to run. The design is [§76](../../design/76-identity-and-authorization.md) (D1600–D1621) and the work is plan [IA1](../../plans/2026-10-21-ia1-identity-authz.md). This page says what will run and what to use until it does. Commands arrive with the bundle.

Owner runbook, to be. On 2026-10-21 the owner ruled a full cutover to Zitadel from the previous identity provider (D1600): no migration, no compatibility period and no dual running, because there are no users. Zitadel will be the identity provider for any install that chooses `auth.loams.dev` as its issuer (D1615; `console.loams.dev`, which D1615 also named, is retired by D1705). OpenFGA answers authorization (D1603); Zitadel only says who someone is (D1601).

## 1. What auth.loams.dev will run

Per §76 §12.3, on one host:

| Component | What |
|---|---|
| Zitadel (API) | The official image, unmodified (Zitadel is AGPL-3.0-only; D1602), pinned by digest at a release at least 14 days old |
| Zitadel Login | The Next.js login app, shipped by Zitadel as its own component |
| Postgres | Zitadel's own database, not shared with anything else (D1617) |
| TLS front | Caddy, or a Cloudflare Tunnel |
| Backups | Nightly, covering the Postgres database |

The bundle will live at [`deploy/release/auth/`](../../../deploy/release/auth), which holds only a README until IA1's auth.loams.dev task lands. The local IA0 spike stack (Zitadel, Zitadel Login, Postgres and OpenFGA on loopback) goes to `conformance/identity/` (IA0 Task 1). It is a test stack, not a deploy.

## 2. Public and tailnet-only surfaces

| Surface | Reach |
|---|---|
| OIDC endpoints (discovery, authorize, token, JWKS, userinfo) and the login UI | Public, on `auth.loams.dev` |
| Zitadel's admin console and management APIs | **Never public.** Tailnet only (D1617, carrying D596's rule over to Zitadel); see [`tailnet.md`](tailnet.md) |
| OpenFGA | Not on this host. Each install runs its own, internal only (D1615) |

The tailnet tag for the host is `tag:zitadel`. The node that will publish the admin console on the tailnet comes with IA1's auth.loams.dev task.

## 3. Applications and identity providers

IA1's setup will create these applications in Zitadel:

| Application | For |
|---|---|
| `loams-console` | The web console (code + PKCE) |
| `loams-desktop` | Loams Desktop (loopback redirect) |
| `loams-mobile` | The phone apps (system browser) |
| `headscale` | Headscale's sign-in ([`headscale.md`](headscale.md)) |

Social sign-in (GitHub and Google) is configured as Zitadel identity providers. There is no Clerk, no WorkOS and no Zitadel Cloud (Q1201).

## 4. How an install trusts it

An install that chooses `auth.loams.dev` runs the engine's `oidc` mode:

```sh
LOAMS_AUTH_MODE=oidc
LOAMS_OIDC_ISSUER=https://auth.loams.dev
LOAMS_OIDC_AUDIENCE=<the Zitadel project id>
```

The audience is the Zitadel project id; IA0 verifies that value (§76 §12.3). The install still decides membership and authorization itself, so Ostrium never sees its tuples.

**Token lifetime (D1630).** Set the instance's access-token lifetime to 900 s (Default settings → OIDC token lifetimes, or `PUT /admin/v1/settings/oidc` with `{"accessTokenLifetime":"900s"}`). agentd writes each installed agent's Zitadel access token to its `LOAMS_TOKEN_FILE` (D1629), so this is the life of an agent's token; agentd warns when tokens live longer.

**agentd** reads `LOAMS_AGENT_ISSUER` (this issuer) and `LOAMS_AGENT_PROJECT` (the project id) to vend agent tokens, and, optionally, `LOAMS_AGENT_ZITADEL_TOKEN_FILE` (a token that may deactivate the org's machine users and remove their keys) so that uninstalling an agent deactivates its machine user (D1632).

## 5. What to use today

Until Zitadel answers at `auth.loams.dev`:

- **Built-in single-org mode** (D1613), the default: the principal store, built-in console sessions with first-run setup, password and TOTP, `lk_` keys, and `tailscale` auth mode by default. It needs no identity provider.
- **Any OIDC issuer** you run or trust: `LOAMS_AUTH_MODE=oidc` with `LOAMS_OIDC_ISSUER` (and `LOAMS_OIDC_AUDIENCE`, plus the subject, email or group allowlists `LOAMS_OIDC_ALLOWED_*`). The engine verifies RS256 or EdDSA JWTs against the issuer's JWKS.

## 6. Not decided here

The runbook proper (start, upgrade, backup and restore drill, break-glass for the admin console) is written with the bundle, after IA0's spike report. Branding goes through Zitadel's settings (Q1211).
