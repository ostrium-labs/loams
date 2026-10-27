# Inputs (Azure)

`deploy/byoc/render.sh azure` fills these into `cloud-init.yaml`. None is a secret.

| Option | Default | Meaning |
|---|---|---|
| `--container` | required | The Blob container. The engine uses the prefix `loams/` in it. |
| `--storage-account` | required | The StorageV2 account that holds the container. |
| `--vault` | required | The Key Vault holding the secrets. |
| `--client-id` | none | The user-assigned identity's client ID (required when the VM has more than one identity). |
| `--version` | `0.1.2` | The Loams release. The bundle's signature and checksum are verified. |
| `--install` | `loams` | Names the secrets: `loams-<install>-tailnet-authkey`, `-setup-token`, `-api-keys` (optional). |
| `--secret-prefix` | `loams-<install>` | Overrides the whole prefix. |
| `--profiles` | none | `COMPOSE_PROFILES`: `tikv`, `rustfs`, ... Choose before the first start. |
| `--hostname` | `loams` | The engine's tailnet name. |
| `--data-device` | none | A disk to format ext4 and mount at `/var/lib/docker`, for example `/dev/disk/azure/scsi1/lun0`. |
| `--confluent-integrations` | none | Comma-separated Confluent integrations (design 80 §7.2). For each name `<i>` the VM fetches `loams-<install>-confluent-<i>-api-key` and `-api-secret` (and, if present, `-sr-key`, `-sr-secret`, `-metrics-key`, `-metrics-secret`) into the connect role's secret files under `secrets/connect/0/confluent-<i>/`, mode 600, never into `.env`. Create the keys with `loams byoc confluent key create`. |
| `--gzip` | off | gzip the output (not needed on Azure: custom data allows 64 KB). |
