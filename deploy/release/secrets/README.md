# secrets/

Mounted read-only into the engine at `/etc/loams/secrets`. Nothing here is
committed except this file (see `.gitignore`).

- `api-keys`: the hashed API keys for `LOAMS_AUTH_MODE=apikey` and for the CLI
  and SDKs beside OIDC (`LOAMS_API_KEYS_FILE`). Its format belongs to the
  engine's auth module. The engine reads it as uid and gid 10001: keep it
  yours, in the engine's group with mode 0640, never world-readable (see "API
  keys" in the release README).
- `engine.env`: optional extra environment for the engine and the `migrate`
  service, one `KEY=value` per line, such as `AWS_ACCESS_KEY_ID`,
  `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` and `AWS_ENDPOINT_URL` (plus
  `AWS_ALLOW_HTTP` and `AWS_VIRTUAL_HOSTED_STYLE_REQUEST` when needed) for an
  `s3://` `LOAMS_BUCKET`, `GOOGLE_SERVICE_ACCOUNT_KEY` for `gs://`, or
  `AZURE_STORAGE_ACCOUNT_NAME` and `AZURE_STORAGE_ACCOUNT_KEY` for `az://` (or, on an Azure VM with a managed identity, `AZURE_CREDENTIAL_TYPE=managed_identity` and no key). See
  "The bucket" in `docs/release/preview.md` for AWS, R2, MinIO/RustFS, GCS and
  Azure.
- `pg-api-keys.json`: the API keys of `pg-control` (`--profile postgres-control`),
  `{"keys": [<entry>, …]}`, each entry made by `loams pg-api-key` (see the
  release README). It holds the keys' hashes, not the keys. `pg-control`
  reads it at `/etc/loams-pg-control/api-keys.json`; the engine does not.
