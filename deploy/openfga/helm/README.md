# OpenFGA on Kubernetes (Helm values)

`values.yaml` here configures the upstream chart **openfga/openfga 0.3.16** (Apache-2.0,
`https://openfga.github.io/helm-charts`) for Loams (design 76 §12). The chart is referenced,
not vendored, and the image is OpenFGA's own, v1.22.0, pinned by digest and unmodified. The
compose bundle's `authz` profile (`deploy/release/compose.yaml`) is the single-user
equivalent.

What the values set, and why:

| Setting | Value | Why |
|---|---|---|
| Datastore | Postgres, from Secret `loams-openfga-datastore` (key `uri`) | SQLite is for one user's install only (D1626); design 76 §12 runs Postgres on CloudNativePG |
| Authentication | preshared key, from Secret `loams-openfga-preshared-keys` (key `keys`) | every call is authenticated; the engine sends the key as `LOAMS_AUTHZ_OPENFGA_KEY` |
| Caches | check-query and iterator caches off | one decision cache, in the engine: allows only, at most 5 s (D1625) |
| Exposure | ClusterIP, no ingress, no playground, no CORS origin | OpenFGA is never public (D1617) |
| NetworkPolicy | ingress only from pods labelled `loams.dev/authz-client: "true"` in the namespace (8080, 8081), and metrics (2112) from the `monitoring` namespace | nothing else in the cluster reaches it |
| Pods | two replicas, non-root (65532), read-only root, no capabilities, `RuntimeDefault` seccomp | |

## Install

```sh
kubectl create namespace loams-authz
kubectl -n loams-authz create secret generic loams-openfga-datastore \
  --from-literal=uri='postgres://openfga:<password>@<host>:5432/openfga?sslmode=verify-full'
kubectl -n loams-authz create secret generic loams-openfga-preshared-keys \
  --from-literal=keys="$(openssl rand -hex 32)"
helm install openfga openfga --repo https://openfga.github.io/helm-charts --version 0.3.16 \
  --namespace loams-authz -f deploy/openfga/helm/values.yaml
```

The NetworkPolicy needs a CNI that enforces it (Cilium, Calico); without one, nothing but the
preshared key keeps other pods out.

## The store and the model

Create the `loams` store and write model v1 once, from a pod labelled
`loams.dev/authz-client: "true"`, with the pinned OpenFGA CLI (`openfga/cli` v0.8.1, as in
`conformance/identity/fga.sh`) and `deploy/openfga` mounted at `/model`:

```sh
fga store create --name loams --model /model/fga.mod \
  --api-url http://openfga.loams-authz:8080 --api-token "$KEY"
```

It prints the store id and the model id. Give them to the engine with
`LOAMS_AUTHZ=openfga`, `LOAMS_AUTHZ_OPENFGA_GRPC=http://openfga.loams-authz:8081`,
`LOAMS_AUTHZ_OPENFGA_STORE`, `LOAMS_AUTHZ_OPENFGA_MODEL` and `LOAMS_AUTHZ_OPENFGA_KEY`, and
label the engine's pods `loams.dev/authz-client: "true"`. When model v1 changes, write it
again (`fga model write --store-id … --file /model/fga.mod`) and update
`LOAMS_AUTHZ_OPENFGA_MODEL`.

OpenFGA's tuples come from the identity sync (plan IA1, IA3). Until it runs, keep the engine on
`LOAMS_AUTHZ=builtin`: on OpenFGA without tuples it grants nothing to anyone but the
operator's own credentials.
