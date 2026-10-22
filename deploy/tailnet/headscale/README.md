# Headscale (private network of the hosted service)

Operating procedure: [`docs/platform/deploy/headscale.md`](../../../docs/platform/deploy/headscale.md).
Design: OSS `docs/design/43-private-networking.md`.

```sh
./check-config.sh            # headscale configtest (HEADSCALE_BIN=... or Docker)
./policy/check.sh            # policy + its tests, against a seeded scratch database
cd compose && docker compose up -d --build
```

Versions are pinned by digest in `compose/docker-compose.yml`, `k8s/headscale.yaml`,
`check-config.sh` and `policy/check.sh`; bump all four together.
