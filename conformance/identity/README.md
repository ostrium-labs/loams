# IA0: Zitadel and OpenFGA on one laptop

The spike stack for [design 76](../../docs/design/76-identity-and-authorization.md) and plan
[IA1](../../docs/plans/2026-10-21-ia1-identity-authz.md) (IA0 Tasks 1-5). The results are in
[the IA0 report](../../docs/plans/ia0-spike-report.md).

Nothing here is part of a Loams release. Everything binds `127.0.0.1` only.

| Profile | Services | Images (pinned by digest) |
|---|---|---|
| `identity` | `zitadel`, `zitadel-login`, `zitadel-db` | Zitadel v4.19.4 (AGPL-3.0-only, run unmodified), Zitadel Login v4.19.4, Postgres 17.10-alpine |
| `authz` | `openfga-migrate`, `openfga` (SQLite) | OpenFGA v1.22.0 (Apache-2.0) |
| `spike` | `ia0-authz` | the spike service from `spikes/ia0-authz`, mounted into a Debian 13 runtime |

## Run it

```sh
cd conformance/identity
./up.sh                     # writes .env (secrets), starts identity + authz, runs bootstrap.sh
cargo build --release --manifest-path ../../spikes/ia0-authz/Cargo.toml
export IA0_SPIKE_BIN=<shared target>/release/ia0-authz
docker compose --profile identity --profile authz --profile spike up -d --wait ia0-authz
./actions.sh                # Actions v2 target + event executions -> ia0-authz
set -a; source .env; set +a
cd ../.. && IA0_FGA_GRPC=http://127.0.0.1:18082 "$IA0_SPIKE_BIN" scenario
```

`smoke.sh` does all of it from nothing and tears it down (CI job `identity-smoke`).

| Endpoint | URL |
|---|---|
| Zitadel issuer, OIDC, APIs | `http://localhost:18080` |
| Zitadel Login | `http://localhost:13000/ui/v2/login` |
| OpenFGA HTTP / gRPC | `127.0.0.1:18081` / `127.0.0.1:18082` |
| Spike service | `http://127.0.0.1:18090` |

## What `bootstrap.sh` creates

- org `acme` (`org:<id>` in OpenFGA);
- project `loams`, whose id is the audience every listener checks, and its OIDC app
  `loams-console` (code + PKCE, JWT access tokens);
- person `alice` (password in `.env`);
- agent `agent-1`, a machine user with JWT access tokens and a private-key JWT key in
  `.secrets/agent-1.json`;
- 15-minute access tokens for the instance;
- an OpenFGA store with model v1 (`deploy/openfga/fga.mod`) written by the OpenFGA CLI.

It writes `.env.ia0` with the ids. `.env`, `.env.ia0`, `.secrets/` and `results/` are
git-ignored.

## Other scripts

- `fga.sh`: the OpenFGA CLI v0.8.1 in a container, with `deploy/openfga` at `/model`.
- `cross_org_scan.py`: `no_cross_org_tuples` over a store or a JSON file (`--self-test`).
- `actions.sh`: registers the spike's webhook and the event executions.

## Notes

- Zitadel answers "instance not found" unless the Host header is `localhost:18080`.
- Zitadel's HTTP client deny list blocks loopback and every private range by default; the
  compose file narrows it so the webhook can reach `ia0-authz` (see the report, F2).
- Under rootless Podman a container cannot reach the host's loopback, which is why the spike
  service runs in the compose network.
