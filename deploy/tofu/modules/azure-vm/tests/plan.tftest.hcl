# Offline tests (mock providers): `tofu test` in this directory.

mock_provider "azurerm" {
  mock_data "azurerm_client_config" {
    defaults = {
      tenant_id = "00000000-0000-0000-0000-000000000001"
      object_id = "00000000-0000-0000-0000-000000000002"
    }
  }
  mock_data "azurerm_key_vault" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/my-rg/providers/Microsoft.KeyVault/vaults/my-existing-vault" }
  }
  mock_resource "azurerm_key_vault" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.KeyVault/vaults/loams-tt-ab12c" }
  }
  mock_resource "azurerm_user_assigned_identity" {
    defaults = {
      id           = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.ManagedIdentity/userAssignedIdentities/loams-tt"
      client_id    = "11111111-2222-3333-4444-555555555555"
      principal_id = "66666666-7777-8888-9999-000000000000"
    }
  }
  mock_resource "azurerm_subnet" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.Network/virtualNetworks/loams-tt/subnets/loams" }
  }
  mock_resource "azurerm_network_security_group" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.Network/networkSecurityGroups/loams-tt" }
  }
  mock_resource "azurerm_public_ip" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.Network/publicIPAddresses/loams-tt" }
  }
  mock_resource "azurerm_network_interface" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.Network/networkInterfaces/loams-tt" }
  }
  mock_resource "azurerm_storage_account" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.Storage/storageAccounts/loamsttab12c" }
  }
  mock_resource "azurerm_storage_container" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000003/resourceGroups/loams-tt/providers/Microsoft.Storage/storageAccounts/loamsttab12c/blobServices/default/containers/loams" }
  }
  mock_resource "azurerm_key_vault_key" {
    defaults = { versionless_id = "https://v.vault.azure.net/keys/k" }
  }
}
mock_provider "random" {
  mock_resource "random_string" {
    defaults = { result = "ab12c" }
  }
}

variables {
  name                 = "tt"
  admin_ssh_public_key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIH9OjnkE0o4EtaT13C4gFzGzsh7c8CnPzRLxozHphCnr test"
}

run "defaults_never_public" {
  command = plan

  assert {
    condition     = length(azurerm_network_security_group.this.security_rule) == 0
    error_message = "the NSG has no rule of its own; Azure's defaults deny inbound from the internet"
  }
  assert {
    condition     = azurerm_linux_virtual_machine.this.disable_password_authentication
    error_message = "no password login"
  }
  assert {
    condition     = azurerm_linux_virtual_machine.this.size == "Standard_D2s_v5"
    error_message = "small is Standard_D2s_v5"
  }
  assert {
    condition     = azurerm_linux_virtual_machine.this.secure_boot_enabled && azurerm_linux_virtual_machine.this.vtpm_enabled
    error_message = "trusted launch"
  }
}

run "storage_has_no_shared_key_and_is_private" {
  command = plan

  assert {
    condition     = azurerm_storage_account.this.shared_access_key_enabled == false
    error_message = "account keys are off: access is by managed identity only"
  }
  assert {
    condition     = azurerm_storage_account.this.allow_nested_items_to_be_public == false && azurerm_storage_account.this.min_tls_version == "TLS1_2"
    error_message = "no public blobs, TLS 1.2"
  }
  assert {
    condition     = azurerm_storage_account.this.blob_properties[0].versioning_enabled && azurerm_storage_account.this.blob_properties[0].delete_retention_policy[0].days == 30
    error_message = "versioning and soft delete"
  }
  assert {
    condition     = azurerm_role_assignment.vm_blobs.role_definition_name == "Storage Blob Data Contributor" && azurerm_role_assignment.vm_secrets.role_definition_name == "Key Vault Secrets User" && azurerm_role_assignment.storage_key.role_definition_name == "Key Vault Crypto Service Encryption User"
    error_message = "the three roles of roles.md"
  }
}

run "the_rendered_config_has_no_key" {
  command = plan

  assert {
    condition     = !can(regex("AZURE_STORAGE_ACCOUNT_KEY|AZURE_STORAGE_SAS|CLIENT_SECRET", module.cloud_init.user_data))
    error_message = "no account key, SAS or client secret in the user data"
  }
  assert {
    condition     = can(regex("AZURE_ACCOUNT=loamsttab12c", module.cloud_init.user_data)) && can(regex("LOAMS_BUCKET_URL=az://loams/loams", module.cloud_init.user_data)) && can(regex("AZURE_CLIENT_ID=11111111-2222-3333-4444-555555555555", module.cloud_init.user_data))
    error_message = "the rendered config names the account, container and identity"
  }
}

run "existing_vault_creates_none" {
  command = plan

  variables {
    key_vault_name      = "my-existing-vault"
    resource_group_name = "my-rg"
  }

  assert {
    condition     = length(azurerm_key_vault.this) == 0 && length(azurerm_resource_group.this) == 0
    error_message = "an existing vault and resource group are used, not created"
  }
}

run "rejects_a_latest_release" {
  command = plan

  variables {
    release = "latest"
  }

  expect_failures = [var.release]
}
