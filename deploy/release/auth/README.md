# The hosted auth service (auth.loams.dev)

For Loams 0.1.1, Ostrium hosts two things: the console and this
auth service. Everything else runs on the user's machines, reached over their own
tailnet. This directory deploys the auth service. It is
[Authentik](https://goauthentik.io) 2026.8.3, open-source edition, behind Caddy.

## Why Authentik

- **It was already chosen.** Design §38 (D447) makes Authentik the identity provider,
  and it is the documented, tested issuer.
- **Little to run.** Since 2025.10 it needs only Postgres (no Redis). The whole
  service is four containers, configured by blueprints kept in Git.
- **It covers what preview needs.** OIDC Authorization Code with PKCE for a public
  client, exact (strict) redirect URIs, refresh tokens, RS256 signing with a published
  JWKS, sign-up with email verification, MFA and passkeys, and RP-initiated logout.
- **No licence key.** We use only code outside `authentik/enterprise/` (D458).

We considered and rejected:

- **A hosted IdP (Auth0, Clerk).** Simpler to run, but it is a paid dependency, and
  §38 rules it out.
- **Our own OIDC server.** Too much new security-critical code for a preview.
- **Keycloak.** Heavier (JVM), and §38 replaced it.

## What it serves

| | |
|---|---|
| Issuer | `https://auth.loams.dev/`, global for every client (`issuer_mode: global`) |
| Discovery | `https://auth.loams.dev/.well-known/openid-configuration` (Caddy rewrites this to the `loams-console` application's document) |
| Signing | RS256, with the key published at the `jwks_uri` |
| Console client | `loams-console`: a public client using PKCE, with no secret |
| Redirect URIs | Exactly `https://console.loams.dev/`, for both sign-in and logout. These are strict matches, never wildcards. |
| Scopes | `openid profile email offline_access` |
| Token lifetimes | Access 10 min, refresh 14 days, code 1 min |

The engine side of the contract, which the default compose stack sets:

```
LOAMS_AUTH_MODE=oidc
LOAMS_OIDC_ISSUER=https://auth.loams.dev
LOAMS_OIDC_AUDIENCE=loams-console          # Authentik's `aud` is the client id
LOAMS_OIDC_ALLOWED_EMAILS=you@example.com  # required: this issuer signs tokens for every user
```

The engine verifies each token's signature against the JWKS, which it caches. It then
checks `iss` (with or without the trailing `/`), `aud`, `exp` and `nbf`. Finally it
admits the user only if their verified email, `sub` or group is on its allowlist.
Anyone can sign up at auth.loams.dev, so a valid signature alone proves nothing about
who may use a particular engine.

**Why `email_verified` can be trusted.** Sign-up uses Authentik's enrollment flow with
email verification, and an account stays inactive until its address is confirmed.
`setup.sh` turns off email and username changes, so nobody can claim an address after
sign-up. The `Loams: email` scope mapping then reports `email_verified: true` for
active users. Authentik's own email mapping always reports `false`.

## Deploy

On a VM with Docker, a DNS record for `AUTH_DOMAIN`, and ports 80 and 443 open:

```sh
cd deploy/release/auth
cp .env.example .env
$EDITOR .env            # secrets, SMTP, bootstrap admin, LOAMS_CONSOLE_ORIGIN
docker compose up -d
./setup.sh              # turns off email changes, then checks the issuer and JWKS
```

Then:

1. Sign in at `https://auth.loams.dev/if/admin/` as `akadmin`, using the bootstrap
   password.
2. Enable MFA for the admin.
3. Remove `AUTHENTIK_BOOTSTRAP_PASSWORD` from `.env`.

The blueprints in `blueprints/` are applied automatically on start and again whenever
they change:

- `loams-console.yaml` sets up:
  - the `loams-console` provider and application;
  - the `Loams: email` scope mapping;
  - sign-up with email verification, linked from the login page.
- `loams-confidential-client.yaml` is a template for a server-side client that keeps a
  secret. It is not applied automatically. Copy it, set its context, and apply it by
  hand. To have the engine admit its tokens, add its client id to
  `LOAMS_OIDC_AUDIENCE`.

**Signing key.** The provider signs with Authentik's built-in self-signed key pair. To
rotate it:

1. Create a new key pair in the admin interface.
2. Point the provider at the new key pair.
3. Keep the old key pair for one access-token lifetime.

Engines fetch the JWKS again when they see an unknown `kid`.

**Backups.** The state lives in the `database` volume (Postgres) and the `data`
volume. Back up both.

## Your own issuer

An engine can trust any OIDC issuer instead of auth.loams.dev, or alongside it.
`LOAMS_OIDC_ISSUER` takes a comma list, for example your own Authentik (this same
stack), Keycloak, Auth0, Google, or a Cloudflare Access team domain. Set the matching
client ids in `LOAMS_OIDC_AUDIENCE`. If an issuer has no discovery document, give its
JWKS URL as the matching entry of `LOAMS_OIDC_JWKS_URL`.

## Coming soon: self-hosted consoles

In 0.1.1, the hosted console at `https://console.loams.dev` is the only browser
client registered with auth.loams.dev. Running the console on your own Vercel,
Cloudflare or custom domain against auth.loams.dev is coming soon. A signed-in page at
`https://auth.loams.dev/register` will create a public PKCE client for your console's
exact URL. Until then, a self-hosted console can use:

- **Tailscale identity:** set `LOAMS_AUTH_MODE=tailscale` on the engine. The
  tailnet login that `tailscale serve` passes on is the credential, and there is no
  OIDC sign-in.
- **your own OIDC issuer**, set as `LOAMS_AUTH_ISSUER` on the console and listed in the
  engine's `LOAMS_OIDC_ISSUER`.
