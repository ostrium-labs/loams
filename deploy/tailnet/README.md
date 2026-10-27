# Private network (tailnet)

Pluggable provider: **Tailscale** (hosted, bring your own key; the default) or
**Headscale** (self-hosted). Switching is configuration only
(`TAILNET_PROVIDER`, `TAILNET_LOGIN_SERVER`, the auth key). Procedure:
[`docs/platform/deploy/tailnet.md`](../../docs/platform/deploy/tailnet.md). Design: OSS
`docs/design/43-private-networking.md`.

```sh
python3 policy/lint.py                  # offline check of the shared policy
DOCKER=podman headscale/policy/check.sh # Headscale's evaluator, with the tests
./check-env.sh path/to/.env             # TAILNET_PROVIDER and TAILNET_LOGIN_SERVER agree
```

| Path | What |
|---|---|
| `policy/` | `policy.hujson`, shared by both providers |
| `headscale/` | the alternative provider |

The tailnet node that keeps the identity provider's admin console off the
public internet (`tag:zitadel`, D596's rule under design 76 D1615) comes with
the Zitadel bundle for `auth.loams.dev` (IA1).
