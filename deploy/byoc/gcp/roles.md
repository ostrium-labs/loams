# Roles (Google Cloud)

The VM runs as a dedicated service account (`loams-vm`). It has these roles and no others, and **no key is ever created for it**: the engine and `loams-byoc-secrets` read its token from the metadata server.

| Principal | Role | On |
|---|---|---|
| `loams-vm` service account | `roles/storage.objectUser` | the one bucket |
| `loams-vm` service account | `roles/secretmanager.secretAccessor` | each of the install's secrets (`loams-<install>-tailnet-authkey`, `-setup-token`, `-api-keys`) |
| the Cloud Storage service agent (`service-<project-number>@gs-project-accounts.iam.gserviceaccount.com`) | `roles/cloudkms.cryptoKeyEncrypterDecrypter` | the key that is the bucket's default CMEK |

`roles/storage.objectUser` lets the engine read, write, delete and list objects. The engine uses create-only writes (`ifGenerationMatch=0`), which this role allows.

The VM's access scope is `cloud-platform`: Secret Manager's API needs it, and a narrower scope such as `storage-rw` would make the secrets fetch fail with 403. The scope does not widen anything; the IAM roles above are what limit the account.
