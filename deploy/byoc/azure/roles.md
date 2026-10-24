# Roles (Azure)

The VM has a **user-assigned managed identity** (`loams`). No account key, SAS token or client secret is created: the engine asks IMDS for a token (`AZURE_CREDENTIAL_TYPE=managed_identity`, plus `AZURE_STORAGE_CLIENT_ID` for the identity), and `loams-byoc-secrets` asks IMDS for a Key Vault token.

| Principal | Role | On |
|---|---|---|
| the VM's managed identity | `Storage Blob Data Contributor` | the one container |
| the VM's managed identity | `Key Vault Secrets User` | the vault (or each secret) |
| the storage account's own identity (for a customer-managed key) | `Key Vault Crypto Service Encryption User` | the key |

The engine supports managed identity: `object_store`'s Azure builder resolves it through IMDS when `AZURE_CREDENTIAL_TYPE=managed_identity` (design 70 §16's account-key fallback is not needed).

The vault must use Azure RBAC (`--enable-rbac-authorization true`), not access policies.
