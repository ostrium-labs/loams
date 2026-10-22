# Private network (tailnet)

Pluggable provider: **Tailscale** (hosted, bring your own key; the default) or
**Headscale** (self-hosted). Switching is configuration only
(`TAILNET_PROVIDER`, `TAILNET_LOGIN_SERVER`, the auth key). Procedure:
[`docs/platform/deploy/tailnet.md`](../../docs/platform/deploy/tailnet.md). Design: OSS
`docs/design/43-private-networking.md`.

```sh
python3 policy/lint.py                  # offline check of the shared policy
DOCKER=podman headscale/policy/check.sh # Headscale's evaluator, with the tests
./check-env.sh ../authentik/compose/.env
kubectl kustomize tailscale             # k3s node for Authentik
```

| Path | What |
|---|---|
| `policy/` | `policy.hujson`, shared by both providers |
| `tailscale/` | k3s manifests (the compose node is the `tailscale` service, profile `tailnet`, in `../authentik/compose/docker-compose.yml`) |
| `headscale/` | the alternative provider |
| `authentik-tailnet/` | Authentik admin on the tailnet (both providers) |
