# Inputs (AWS)

`deploy/byoc/render.sh aws` fills these into `cloud-init.yaml`. None is a secret.

| Option | Default | Meaning |
|---|---|---|
| `--bucket` | required | The S3 bucket. The engine uses the prefix `loams/` in it. |
| `--region` | required | The bucket's and the secrets' region, for example `us-east-1`. |
| `--version` | `0.1.3` | The Loams release. The bundle's signature and checksum are verified. |
| `--install` | `loams` | Names the secrets: `loams/<install>/tailnet-authkey`, `setup-token`, `api-keys` (optional). |
| `--secret-prefix` | `loams/<install>` | Overrides the whole prefix. |
| `--profiles` | none | `COMPOSE_PROFILES`: `tikv`, `rustfs`, ... Choose before the first start. |
| `--hostname` | `loams` | The engine's tailnet name. |
| `--data-device` | none | A disk to format ext4 and mount at `/var/lib/docker`, for example `/dev/nvme1n1`. |
| `--confluent-integrations` | none | Comma-separated Confluent integrations (design 80 §7.2). For each name `<i>` the VM fetches `loams/<install>/confluent-<i>-api-key` and `-api-secret` (and, if present, `-sr-key`, `-sr-secret`, `-metrics-key`, `-metrics-secret`) into the connect role's secret files under `secrets/connect/0/confluent-<i>/`, mode 600, never into `.env`. Create the keys with `loams byoc confluent key create`. |
| `--gzip` | off | gzip the output. AWS limits user-data to 16 KB and the plain file is about 20 KB; always use it on AWS. |
