# deploy/byoc

Loams on one VM in your own cloud account (design 70, plan BY1/BY0). One directory per cloud: `aws/`, `gcp/`, `azure/`.

- `<cloud>/cloud-init.yaml`: the user-data for Ubuntu 24.04. **Generated** by `gen-cloud-init.sh` from `cloud-init.tmpl.yaml`, `lib/` and `<cloud>/config.env`; edit those.
- `render.sh`: fills the `@@placeholders@@` with checked values (`render.sh <cloud> --help`).
- `lib/loams-byoc-bootstrap`: first boot: Docker, the signed compose bundle, `.env`, the service.
- `lib/loams-byoc-secrets`: reads the tailnet key and setup token from the cloud's secret store with the VM's identity (no cloud CLI, no key).
- `lib/loams-byoc.service`: runs `loams-byoc-secrets`, then `docker compose up -d`; `systemctl reload loams-byoc` copies rotated Confluent keys without a restart.
- `lib/loams-byoc-confluent.{service,timer}`: every 5 minutes, `loams-byoc-secrets --confluent-only` copies the Confluent keys into the connect role's files (design 80 §7.2; a no-op without `CONFLUENT_INTEGRATIONS`).
- `<cloud>/README.md`, `variables.md`, `roles.md` or `instance-policy.json`, `CHANGELOG.md`.

Rules (design 70 §4, deploy-buttons.md): nothing opens an inbound port; the engine is reached over your tailnet; no static cloud key lands on the VM; the release is pinned and its signature checked against `docs/release/loams-release-key.asc`.

Tests: `tests/byoc/static/run.sh` (see its header for the tools).
