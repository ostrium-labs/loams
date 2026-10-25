# auth.loams.dev

This directory holds the bundle for the hosted identity provider at
`auth.loams.dev`. That provider is **Zitadel** (design
[§76](../../../docs/design/76-identity-and-authorization.md), D1600 and D1615).
The bundle (Zitadel, Zitadel Login, their own Postgres and a TLS front, plus a
`setup.sh` that creates the `loams-console`, `loams-desktop`, `loams-mobile`
and `headscale` applications and the GitHub and Google identity providers)
lands with IA1, after the IA0 spike
([plan](../../../docs/plans/2026-10-21-ia1-identity-authz.md)). It is not here
yet.

Until then, a Loams install signs people in without it:

- **Built-in single-org mode** (D1613), the default: `tailscale` mode, built-in
  console sessions with first-run setup, password and TOTP, and `lk_` keys.
- **`oidc` mode** against any OIDC issuer you run: `LOAMS_AUTH_MODE=oidc`,
  `LOAMS_OIDC_ISSUER`, `LOAMS_OIDC_AUDIENCE` and an allow-list
  (`../.env.example`).

Operator notes: [`docs/platform/deploy/zitadel.md`](../../../docs/platform/deploy/zitadel.md).
