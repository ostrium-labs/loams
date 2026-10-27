# deploy/browser: the Loams headless browser service

This directory packages **Obscura**, the one browser engine Loams agents use, to run as the `browser` service of a self-hosted or cloud deployment. The same release runs locally when agentd or Desktop launches it on a laptop: D2110–D2114 and [docs/headless-browser.md](../../docs/headless-browser.md).

**Status: preview, not yet run end to end.** The manifests and the compose profile are checked as configuration only. The run against the real image is lane BR-b's parity job.

| File | What it is |
|---|---|
| `Containerfile` | Builds an image from the pinned release tarball, after checking its sha256, on distroless `nonroot`. Use it if you build your own images. |
| `k8s/` | A Deployment, a ClusterIP Service and a NetworkPolicy, applied with `kubectl apply -k`. |
| [`../release/compose.yaml`](../release/compose.yaml) | The `browser` profile of the self-hosted stack. |

## The engine

| | |
|---|---|
| Engine | [Obscura](https://github.com/h4ckf0r0day/obscura) 0.2.3 (Apache-2.0), released 2026-09-20 |
| Image | `docker.io/h4ckf0r0day/obscura:0.2.3@sha256:475def3ddf1ec513b3d1bc36e8ad15f0d192538cb15f814c77215aa70c418ca2` |
| Tarballs | x86_64 `sha256:1534d1e6ddaf3d080ec4091eb41d0a4d8cc042a48b607d3c410fc13b482a9eec`, aarch64 `sha256:5ecf980bca3060236a7a86ec7ed83d943e6598ee87caa46d20325d90bc75f979` |
| Runs as | uid/gid 65532, no shell, read-only root, no capabilities |
| Listens | 9222 (CDP), with `OBSCURA_CDP_TOKEN` required as a bearer token |

Change the pin in four places together, and only to a release at least 14 days old:

- this file
- `deploy/release/compose.yaml`
- `k8s/deployment.yaml`
- `PINNED_OBSCURA_VERSION` in `loams-web-bridge` and `loams-sidebar-browser`

## Never public

The browser fetches whatever an agent asks for, so whoever can reach it has a browser inside your network. It must never be published.

- **Compose:**
  - The service publishes no port.
  - It sits on the internal `browser` network, which only agentd shares, and on a `browser-egress` network of its own for the internet.
  - It refuses to start without `LOAMS_BROWSER_TOKEN`, which must be at least 32 bytes.
- **Kubernetes:**
  - The Service is ClusterIP only.
  - The NetworkPolicy admits only `loams-agentd` and `loams-gateway` pods.
  - It lets the browser out only to DNS and to public addresses on ports 80 and 443. Private, link-local (cloud metadata) and CGNAT ranges are excluded.
- **loams-gateway:** there is no CDP WebSocket route yet (design 57). Until there is one, agentd connects to the service directly on the private network, and the gateway is only admitted by the policy ahead of time.

The provider adds the same egress policy, in code, on every request the page makes (D2112). The network rules are the second layer.

## Compose

```sh
# .env
COMPOSE_PROFILES=browser               # plus whatever else you run
LOAMS_BROWSER_TOKEN=$(openssl rand -hex 32)
LOAMS_BROWSER_PROVIDER=service
```

agentd then reaches `ws://browser:9222/devtools/browser`. It reads the token from `LOAMS_WEB_BRIDGE_SECRET_BROWSER_TOKEN`, which compose fills from `LOAMS_BROWSER_TOKEN`.

## Kubernetes

```sh
kubectl -n loams create secret generic loams-browser-token \
  --from-literal=token="$(openssl rand -hex 32)"
kubectl apply -k deploy/browser/k8s -n loams
```

Give agentd these settings:

- the same secret, as `LOAMS_WEB_BRIDGE_SECRET_BROWSER_TOKEN`
- `LOAMS_BROWSER_PROVIDER=service`
- `LOAMS_BROWSER_SERVICE_URL=ws://loams-browser:9222/devtools/browser`
- `LOAMS_BROWSER_SERVICE_TOKEN_REF=env:browser-token`

Label the agentd pods `app.kubernetes.io/name: loams-agentd` so the NetworkPolicy admits them.

## Licence

Obscura is Apache-2.0. It is used unmodified, as the upstream image or the upstream release binary, and none of its source is copied into this repository.
