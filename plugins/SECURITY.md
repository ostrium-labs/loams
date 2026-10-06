# Security policy

## Reporting a vulnerability

**Do not open a public issue.** Use GitHub's private advisory form:

> https://github.com/ostrium-labs/loams-plugins/security/advisories/new

Please include:

- what an attacker can do, not what you think they cannot,
- the exact request or configuration that reaches it,
- the version or commit SHA you tested,
- whether you needed a credential, and whose.

You will get an acknowledgement within 72 hours. There is no bounty and no
formal SLA; if you need one for a disclosure decision, say so up front and we
will talk.

## Scope

**In scope.** This repository: the Cordis plugin host, console, agent bus, A2A
endpoint, auth layer, and the read-only upstream adapters.

**Out of scope.** The Loams platform itself
(<https://github.com/ostrium-labs/loams>) — report those through that project's
own process. Its metering, billing and commercial APIs are proprietary to it and
none of that code is present here.

Also out of scope:

- **Findings in an upstream service.** We call Zulip, Forgejo, Langfuse,
  OpenPanel, GlitchTip, Matomo, It's a Plan, Superset and Loams over HTTP. A
  vulnerability in one of those is that project's to fix.
- **Findings that require an operator to misconfigure their own deployment on
  purpose** — for example leaving `OIDC_INSECURE=true` in production, or setting
  `LOAMS_ALLOW_SQL=true` deliberately. Those knobs are documented as dangerous.
- **Missing hardening with no reachable path.** Tell us anyway; we would rather
  know.

## Known and documented

These are deliberate, recorded, and not vulnerabilities:

- **Agent cards are unsigned.** Served unsigned by design pending a JWS over an
  RFC 8785 canonicalised payload. See
  [docs/a2a.md](docs/a2a.md#documented-deviations-from-strict-a2a-v10).
- **`message:send` answers "unknown agent" before authentication**, so anonymous
  callers can enumerate registered agent ids. No scope information is disclosed.
  The trade is documented in [docs/a2a.md](docs/a2a.md).
- **`GET /api/plugins` names the environment variables each adapter wants.** A
  console that cannot say which variable is missing is useless, but it does mean
  the plugin list is not a secret.
- **The console is a development-mode convenience.** With auth disabled, the
  enable/disable endpoints return 403 `admin_unavailable` — a dev server cannot
  reconfigure itself. Run this behind an authenticating proxy.

## Security-relevant documentation

- [docs/security.md](docs/security.md) — what is enforced where, and the traps.
- [docs/auth-setup.md](docs/auth-setup.md) — Authentik OIDC, and five
  configuration mistakes that fail silently.
- [docs/upstreams.md](docs/upstreams.md) — one subsection per upstream: auth
  mechanism, API quirks, rate limits.
- [NOTICE](NOTICE) — vendored third-party source, and the upstreams we call over
  HTTP without shipping a line of.
