# Authentik (open-source edition) for auth.loams.dev

Ops config for the hosted Loams identity provider. Start with the runbook:
[`docs/platform/deploy/authentik.md`](../../docs/platform/deploy/authentik.md).

- `compose/`: single VM, Docker Compose + cloudflared + nightly backups
- `helm/`, `kustomization.yaml`: k3s with the official chart
- `blueprints/`: groups, flows (sign in with admin MFA, enrollment, recovery), OIDC clients (`loams-cloud`, `loams-desktop`, `loams-mobile`), brand
- `branding/`: placeholder logo and favicon

Open-source edition only. The Authentik Helm chart is GPL-3.0 and is referenced, not copied.
