# Inputs (Google Cloud)

`deploy/byoc/render.sh gcp` fills these into `cloud-init.yaml`. None is a secret.

| Option | Default | Meaning |
|---|---|---|
| `--bucket` | required | The GCS bucket. The engine uses the prefix `loams/` in it. |
| `--project` | required | The project holding the secrets. |
| `--version` | `0.1.2` | The Loams release. The bundle's signature and checksum are verified. |
| `--install` | `loams` | Names the secrets: Secret Manager names cannot hold `/`, so `loams-<install>-tailnet-authkey`, `-setup-token`, `-api-keys` (optional). |
| `--secret-prefix` | `loams-<install>` | Overrides the whole prefix. |
| `--profiles` | none | `COMPOSE_PROFILES`: `tikv`, `rustfs`, ... Choose before the first start. |
| `--hostname` | `loams` | The engine's tailnet name. |
| `--data-device` | none | A disk to format ext4 and mount at `/var/lib/docker`, for example `/dev/disk/by-id/google-loams-data`. |
| `--gzip` | off | gzip the output (not needed on Google Cloud: metadata values allow 256 KB). |
