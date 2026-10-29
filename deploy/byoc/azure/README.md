# Loams on one Azure VM (your subscription)

Status: **new, tested without a cloud account** (cloud-init schema, the secrets fetcher against a local mock of IMDS and Key Vault, role checks). A real-subscription run is still to be done.

Creates, in your subscription: one VM (Ubuntu 24.04) with a user-assigned managed identity and a network security group with **no inbound allow rule**. The storage account, container, Key Vault and secrets are yours and are created by you first. Loams 0.1.3's light default stack runs on the VM (single node: back up the disk, keep blob versioning and soft delete on). The engine is reached over your tailnet only. No key is created; see [roles.md](roles.md).

## Use it

```bash
RG=loams; LOC=eastus; ACCOUNT=myloamsstorage; CONTAINER=loams; VAULT=my-loams-vault

# 1. Secrets (values from your own tailnet admin console; never commit them).
az keyvault secret set --vault-name "$VAULT" --name loams-prod-tailnet-authkey --value "$TS_AUTHKEY" >/dev/null
az keyvault secret set --vault-name "$VAULT" --name loams-prod-setup-token --value "$(openssl rand -hex 32)" >/dev/null

# 2. The identity and its roles (roles.md).
az identity create -g "$RG" -n loams
PRINCIPAL=$(az identity show -g "$RG" -n loams --query principalId -o tsv)
CLIENT_ID=$(az identity show -g "$RG" -n loams --query clientId -o tsv)
ACCOUNT_ID=$(az storage account show -g "$RG" -n "$ACCOUNT" --query id -o tsv)
az role assignment create --assignee-object-id "$PRINCIPAL" --assignee-principal-type ServicePrincipal \
  --role "Storage Blob Data Contributor" --scope "$ACCOUNT_ID/blobServices/default/containers/$CONTAINER"
az role assignment create --assignee-object-id "$PRINCIPAL" --assignee-principal-type ServicePrincipal \
  --role "Key Vault Secrets User" --scope "$(az keyvault show -n "$VAULT" --query id -o tsv)"

# 3. The user-data.
deploy/byoc/render.sh azure --container "$CONTAINER" --storage-account "$ACCOUNT" --vault "$VAULT" \
  --client-id "$CLIENT_ID" --install prod > user-data.yaml

# 4. The VM: --nsg-rule NONE leaves the NSG with Azure's default rules, which deny inbound from the internet.
az vm create -g "$RG" -n loams --image Canonical:ubuntu-24_04-lts:server:latest --size Standard_D2s_v5 \
  --assign-identity "$(az identity show -g "$RG" -n loams --query id -o tsv)" \
  --os-disk-size-gb 64 --nsg-rule NONE --public-ip-sku Standard \
  --admin-username loams --generate-ssh-keys --custom-data user-data.yaml
```

The public IP is only for outbound traffic (the tailnet, GitHub, Docker's registry); no rule allows anything in. For a shell use `az vm run-command invoke` or Azure Bastion. Watch the first boot with `az vm run-command invoke -g "$RG" -n loams --command-id RunShellScript --scripts 'journalctl -u cloud-final -u loams-byoc --no-pager | tail -50'`.

## Limits and status

- One node, one disk. Not highly available.
- Not yet run against a real subscription in CI.

See [variables.md](variables.md), [CHANGELOG.md](CHANGELOG.md), and design 70 (`docs/design/70-byoc-clouds.md`).
