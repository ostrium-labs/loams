# Loams on Azure (one virtual machine)

Read [the overview](index.md) first (status, rules, costs). Files: `deploy/byoc/azure/` (with [roles.md](../../../deploy/byoc/azure/roles.md)) and `deploy/tofu/modules/azure-vm/`. Commands are copy-paste for a shell with the Azure CLI signed in (`az login`) to the subscription you want; no portal click paths.

```bash
export RG=loams LOC=eastus INSTALL=prod
export ACCOUNT=myloamsstorage VAULT=my-loams-vault CONTAINER=loams   # storage account: 3-24 lowercase letters and digits
az group create -n "$RG" -l "$LOC"
```

## 1. Before you start

- The Azure CLI, `jq` and `openssl`; a clone of `ostrium-labs/loams` at the tag you install. You need Owner, or Contributor plus User Access Administrator, on the resource group (the steps create role assignments).
- A tailnet with MagicDNS and HTTPS certificates on: [Self-hosting over Tailscale](https://loams.dev/docs/self-hosting/tailscale), steps 1 to 4.
- A Tailscale auth key (tagged `tag:loams`) in your shell only: `export TS_AUTHKEY=tskey-auth-...`.

## 2. The key vault and the key

Azure uses a Key Vault for both the secrets and the bucket's customer-managed key; it must use Azure RBAC.

```bash
az keyvault create -g "$RG" -n "$VAULT" -l "$LOC" --enable-rbac-authorization true --enable-purge-protection true
VAULT_ID=$(az keyvault show -n "$VAULT" --query id -o tsv)
ME=$(az ad signed-in-user show --query id -o tsv)
az role assignment create --assignee-object-id "$ME" --assignee-principal-type User --role "Key Vault Administrator" --scope "$VAULT_ID"
az keyvault key create --vault-name "$VAULT" -n "loams-$INSTALL-storage" --kty RSA --size 2048 --ops wrapKey unwrapKey   # allow a minute for the role assignment
```

## 3. The bucket (a storage account and container)

An identity for the account reads the key; shared keys are turned off, so access is by managed identity only.

```bash
az identity create -g "$RG" -n "loams-$INSTALL-storage"
STORAGE_PRINCIPAL=$(az identity show -g "$RG" -n "loams-$INSTALL-storage" --query principalId -o tsv)
STORAGE_IDENTITY=$(az identity show -g "$RG" -n "loams-$INSTALL-storage" --query id -o tsv)
az role assignment create --assignee-object-id "$STORAGE_PRINCIPAL" --assignee-principal-type ServicePrincipal --role "Key Vault Crypto Service Encryption User" --scope "$VAULT_ID"
az storage account create -g "$RG" -n "$ACCOUNT" -l "$LOC" --sku Standard_LRS --kind StorageV2 --min-tls-version TLS1_2 \
  --allow-blob-public-access false --allow-shared-key-access false --default-action Allow \
  --identity-type UserAssigned --user-identity-id "$STORAGE_IDENTITY" \
  --encryption-key-source Microsoft.Keyvault --encryption-key-vault "https://$VAULT.vault.azure.net" \
  --encryption-key-name "loams-$INSTALL-storage" --key-vault-user-identity-id "$STORAGE_IDENTITY"
az storage account blob-service-properties update -g "$RG" --account-name "$ACCOUNT" \
  --enable-versioning true --enable-delete-retention true --delete-retention-days 30 \
  --enable-container-delete-retention true --container-delete-retention-days 30
az storage container-rm create --storage-account "$ACCOUNT" -g "$RG" -n "$CONTAINER"
```

## 4. The secrets

```bash
az keyvault secret set --vault-name "$VAULT" -n "loams-$INSTALL-tailnet-authkey" --value "$TS_AUTHKEY" >/dev/null
az keyvault secret set --vault-name "$VAULT" -n "loams-$INSTALL-setup-token" --value "$(openssl rand -hex 32)" >/dev/null
```

`loams-$INSTALL-api-keys` is optional. Read the setup token back with `az keyvault secret show --vault-name $VAULT -n loams-$INSTALL-setup-token --query value -o tsv`.

## 5. The identity

A user-assigned managed identity with only these roles, and **no key** ([roles.md](../../../deploy/byoc/azure/roles.md)):

```bash
az identity create -g "$RG" -n "loams-$INSTALL"
PRINCIPAL=$(az identity show -g "$RG" -n "loams-$INSTALL" --query principalId -o tsv)
CLIENT_ID=$(az identity show -g "$RG" -n "loams-$INSTALL" --query clientId -o tsv)
IDENTITY_ID=$(az identity show -g "$RG" -n "loams-$INSTALL" --query id -o tsv)
ACCOUNT_ID=$(az storage account show -g "$RG" -n "$ACCOUNT" --query id -o tsv)
az role assignment create --assignee-object-id "$PRINCIPAL" --assignee-principal-type ServicePrincipal --role "Storage Blob Data Contributor" --scope "$ACCOUNT_ID/blobServices/default/containers/$CONTAINER"
az role assignment create --assignee-object-id "$PRINCIPAL" --assignee-principal-type ServicePrincipal --role "Key Vault Secrets User" --scope "$VAULT_ID"
```

The engine reads this identity through IMDS (`AZURE_CREDENTIAL_TYPE=managed_identity`, `AZURE_STORAGE_CLIENT_ID`); no account key exists to leak.

## 6. The VM

```bash
deploy/byoc/render.sh azure --container "$CONTAINER" --storage-account "$ACCOUNT" --vault "$VAULT" --client-id "$CLIENT_ID" --install "$INSTALL" > user-data.yaml
az vm create -g "$RG" -n "loams-$INSTALL" --image Canonical:ubuntu-24_04-lts:server:latest --size Standard_D2s_v5 \
  --assign-identity "$IDENTITY_ID" --os-disk-size-gb 64 --storage-sku StandardSSD_LRS \
  --nsg-rule NONE --public-ip-sku Standard --admin-username loams --generate-ssh-keys \
  --custom-data user-data.yaml
```

`--nsg-rule NONE` leaves the network security group with Azure's default rules, which deny inbound traffic from the internet. The public IP is for outbound traffic only; the SSH key is created but nothing can reach SSH. Add `--profiles tikv` to `render.sh` for TiKV, TiDB and PD (and use `Standard_D4s_v5`). On an idle trial, `Standard_B2ms` is cheaper.

Already have a VM with that identity? `deploy/release/byoc/install.sh --cloud azure --bucket az://$CONTAINER/loams --storage-account $ACCOUNT --vault $VAULT ubuntu@HOST` installs onto it over SSH.

**OpenTofu instead of steps 2 to 6:** `cd deploy/tofu/examples/azure-vm && tofu init && tofu apply -var subscription_id=... -var "admin_ssh_public_key=$(cat ~/.ssh/id_ed25519.pub)"`, then put the tailnet key in the vault (the example's header has the command); the VM retries until it exists.

## 7. Check it

```bash
az vm run-command invoke -g "$RG" -n "loams-$INSTALL" --command-id RunShellScript \
  --scripts 'journalctl -u cloud-final -u loams-byoc --no-pager | tail -40; docker compose --project-directory /opt/loams/release ps'
```

The first boot takes a few minutes; the unit retries every 30 seconds until the secrets exist. When the sidecar has joined, the engine is at `https://loams.<your-tailnet>.ts.net`; the console asks for the setup token. The bucket capability probe (`loams store probe`) is not built yet; check the bucket by hand if you need to.

## 8. Backups

Blob versioning and soft delete (step 3) protect the bucket; protect the OS disk (it holds the metastore, Live and Durable) with Azure Backup:

```bash
az backup vault create -g "$RG" -n "loams-$INSTALL-backup" -l "$LOC"
az backup protection enable-for-vm -g "$RG" --vault-name "loams-$INSTALL-backup" --vm "loams-$INSTALL" --policy-name DefaultPolicy
```

`DefaultPolicy` takes a daily backup and keeps 30 days. To restore, restore the disk to a new VM from the backup, or boot a new VM from the same cloud-init file with the restored disk. A backup of a running VM is crash-consistent unless the stack was stopped first (`sudo systemctl stop loams-byoc`).

## 9. Upgrades

Run commands with `az vm run-command` (step 7) or Bastion and follow [upgrades](../../release/upgrades.md): `cd /opt/loams/release`, install the target release's `loams` CLI (checked against that release's `SHA256SUMS`), then `loams upgrade check --to <version>` and `loams upgrade --to <version>`. Embedded stacks are upgraded one release at a time and by hand for now.

## 10. Private reach

An internal load balancer with Private Link for other networks is design 69 and a later BY1 milestone; it is not set up here. Use your tailnet.

## 11. Costs

About $82 a month for `Standard_D2s_v5`, about $72 for `B2ms` (see [the overview](index.md#costs-estimates-small-install-one-month-2026-10-21-list-prices)).

## 12. Remove it

```bash
az vm delete -g "$RG" -n "loams-$INSTALL" --yes
az network public-ip delete -g "$RG" -n "loams-${INSTALL}PublicIP"; az network nic delete -g "$RG" -n "loams-${INSTALL}VMNic"
az identity delete -g "$RG" -n "loams-$INSTALL"
az keyvault secret delete --vault-name "$VAULT" -n "loams-$INSTALL-tailnet-authkey"; az keyvault secret delete --vault-name "$VAULT" -n "loams-$INSTALL-setup-token"
```

Your data is in the storage account and the backups: keep them, or delete the container and account. The vault has purge protection on, so deleting it only soft-deletes it for the retention period. Deleting the resource group (`az group delete -n $RG`) removes everything, data included. Remove the machine from your Tailscale admin console.
