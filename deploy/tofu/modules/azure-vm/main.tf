data "azurerm_client_config" "current" {}

resource "random_string" "suffix" {
  length  = 5
  upper   = false
  special = false
}

locals {
  create_rg    = var.resource_group_name == null
  rg_name      = local.create_rg ? azurerm_resource_group.this[0].name : var.resource_group_name
  create_vault = var.key_vault_name == null
  prefix       = coalesce(var.secret_prefix, "loams-${var.name}")
  account_name = "loams${var.name}${random_string.suffix.result}"
  vault_name   = local.create_vault ? "loams-${var.name}-${random_string.suffix.result}" : var.key_vault_name
  vault_id     = local.create_vault ? azurerm_key_vault.this[0].id : data.azurerm_key_vault.existing[0].id
  container    = "loams"
  vm_size      = coalesce(var.vm_size, var.size == "small" ? "Standard_D2s_v5" : "Standard_D4s_v5")
  tags         = merge({ "loams-install" = var.name, "loams-managed-by" = "opentofu" }, var.tags)
  setup_token  = var.create_setup_token ? random_id.setup_token[0].hex : null

  # Estimates, East US pay-as-you-go prices of 2026-10-21 (design 70 §6.3): the VM
  # for 730 hours, the OS disk, 50 GB of Blob, Key Vault operations, a public IPv4.
  hourly = {
    "Standard_D2s_v5" = 0.096
    "Standard_D4s_v5" = 0.192
    "Standard_B2ms"   = 0.0832
  }
  estimated_monthly_usd = floor(lookup(local.hourly, local.vm_size, 0.1) * 730 + var.disk_size_gb * 0.075 + 1.04 + 0.1 + 3.65)
}

resource "azurerm_resource_group" "this" {
  count    = local.create_rg ? 1 : 0
  name     = "loams-${var.name}"
  location = var.location
  tags     = local.tags
}

# --- identities: the VM's (no key anywhere) and the storage account's --------------
resource "azurerm_user_assigned_identity" "vm" {
  name                = "loams-${var.name}"
  resource_group_name = local.rg_name
  location            = var.location
  tags                = local.tags
}

resource "azurerm_user_assigned_identity" "storage" {
  name                = "loams-${var.name}-storage"
  resource_group_name = local.rg_name
  location            = var.location
  tags                = local.tags
}

# --- the Key Vault: secrets, and the key for the storage account ---------------------
resource "azurerm_key_vault" "this" {
  count                      = local.create_vault ? 1 : 0
  name                       = local.vault_name
  resource_group_name        = local.rg_name
  location                   = var.location
  tenant_id                  = data.azurerm_client_config.current.tenant_id
  sku_name                   = "standard"
  rbac_authorization_enabled = true
  purge_protection_enabled   = var.purge_protection
  soft_delete_retention_days = 30
  tags                       = local.tags
}

data "azurerm_key_vault" "existing" {
  count               = local.create_vault ? 0 : 1
  name                = var.key_vault_name
  resource_group_name = local.rg_name
}

# The deployer creates the key and the setup token.
resource "azurerm_role_assignment" "deployer" {
  count                = local.create_vault ? 1 : 0
  scope                = local.vault_id
  role_definition_name = "Key Vault Administrator"
  principal_id         = data.azurerm_client_config.current.object_id
}

resource "azurerm_key_vault_key" "storage" {
  name         = "loams-${var.name}-storage"
  key_vault_id = local.vault_id
  key_type     = "RSA"
  key_size     = 2048
  key_opts     = ["unwrapKey", "wrapKey"]
  depends_on   = [azurerm_role_assignment.deployer]
}

resource "azurerm_role_assignment" "storage_key" {
  scope                = local.vault_id
  role_definition_name = "Key Vault Crypto Service Encryption User"
  principal_id         = azurerm_user_assigned_identity.storage.principal_id
}

resource "azurerm_role_assignment" "vm_secrets" {
  scope                = local.vault_id
  role_definition_name = "Key Vault Secrets User"
  principal_id         = azurerm_user_assigned_identity.vm.principal_id
}

resource "random_id" "setup_token" {
  count       = var.create_setup_token ? 1 : 0
  byte_length = 32
}

resource "azurerm_key_vault_secret" "setup_token" {
  count        = var.create_setup_token ? 1 : 0
  name         = "${local.prefix}-setup-token"
  value        = local.setup_token
  key_vault_id = local.vault_id
  depends_on   = [azurerm_role_assignment.deployer]
}

# --- the storage account: private data plane auth only, versioned, with the key -------
resource "azurerm_storage_account" "this" {
  name                            = local.account_name
  resource_group_name             = local.rg_name
  location                        = var.location
  account_kind                    = "StorageV2"
  account_tier                    = "Standard"
  account_replication_type        = "LRS"
  min_tls_version                 = "TLS1_2"
  https_traffic_only_enabled      = true
  allow_nested_items_to_be_public = false
  shared_access_key_enabled       = false
  default_to_oauth_authentication = true
  tags                            = local.tags

  identity {
    type         = "UserAssigned"
    identity_ids = [azurerm_user_assigned_identity.storage.id]
  }

  customer_managed_key {
    key_vault_key_id          = azurerm_key_vault_key.storage.versionless_id
    user_assigned_identity_id = azurerm_user_assigned_identity.storage.id
  }

  blob_properties {
    versioning_enabled = true
    delete_retention_policy {
      days = 30
    }
    container_delete_retention_policy {
      days = 30
    }
  }

  depends_on = [azurerm_role_assignment.storage_key]
}

resource "azurerm_storage_container" "this" {
  name                  = local.container
  storage_account_id    = azurerm_storage_account.this.id
  container_access_type = "private"
}

resource "azurerm_role_assignment" "vm_blobs" {
  scope                = azurerm_storage_container.this.id
  role_definition_name = "Storage Blob Data Contributor"
  principal_id         = azurerm_user_assigned_identity.vm.principal_id
}

# --- the network: an NSG with no allow rule (Azure's defaults deny inbound from the internet)
resource "azurerm_virtual_network" "this" {
  name                = "loams-${var.name}"
  resource_group_name = local.rg_name
  location            = var.location
  address_space       = [var.address_space]
  tags                = local.tags
}

resource "azurerm_subnet" "this" {
  name                 = "loams"
  resource_group_name  = local.rg_name
  virtual_network_name = azurerm_virtual_network.this.name
  address_prefixes     = [var.address_space]
}

resource "azurerm_network_security_group" "this" {
  name                = "loams-${var.name}"
  resource_group_name = local.rg_name
  location            = var.location
  tags                = local.tags
  # No security_rule block: nothing can connect in.
}

resource "azurerm_subnet_network_security_group_association" "this" {
  subnet_id                 = azurerm_subnet.this.id
  network_security_group_id = azurerm_network_security_group.this.id
}

resource "azurerm_public_ip" "this" {
  name                = "loams-${var.name}"
  resource_group_name = local.rg_name
  location            = var.location
  allocation_method   = "Static"
  sku                 = "Standard"
  tags                = local.tags
}

resource "azurerm_network_interface" "this" {
  name                = "loams-${var.name}"
  resource_group_name = local.rg_name
  location            = var.location
  tags                = local.tags

  ip_configuration {
    name                          = "primary"
    subnet_id                     = azurerm_subnet.this.id
    private_ip_address_allocation = "Dynamic"
    public_ip_address_id          = azurerm_public_ip.this.id # outbound only: the NSG allows no inbound
  }
}

# --- the VM ----------------------------------------------------------------------------------
module "cloud_init" {
  source                 = "../shared"
  cloud                  = "azure"
  release                = var.release
  container              = local.container
  storage_account        = local.account_name
  vault                  = local.vault_name
  client_id              = azurerm_user_assigned_identity.vm.client_id
  secret_prefix          = local.prefix
  profiles               = var.profiles
  hostname               = var.hostname
  confluent_integrations = var.confluent_integrations
}

resource "azurerm_linux_virtual_machine" "this" {
  name                            = "loams-${var.name}"
  resource_group_name             = local.rg_name
  location                        = var.location
  size                            = local.vm_size
  admin_username                  = "loams"
  disable_password_authentication = true
  network_interface_ids           = [azurerm_network_interface.this.id]
  custom_data                     = base64encode(module.cloud_init.user_data)
  secure_boot_enabled             = true
  vtpm_enabled                    = true
  tags                            = local.tags

  admin_ssh_key {
    username   = "loams"
    public_key = var.admin_ssh_public_key
  }

  identity {
    type         = "UserAssigned"
    identity_ids = [azurerm_user_assigned_identity.vm.id]
  }

  os_disk {
    caching              = "ReadWrite"
    storage_account_type = "StandardSSD_LRS"
    disk_size_gb         = var.disk_size_gb
  }

  source_image_reference {
    publisher = "Canonical"
    offer     = "ubuntu-24_04-lts"
    sku       = "server"
    version   = "latest"
  }

  # A new image release must not replace a running install: upgrade with `loams upgrade`.
  lifecycle {
    ignore_changes = [custom_data, source_image_reference[0].version]
  }

  depends_on = [
    azurerm_role_assignment.vm_secrets,
    azurerm_role_assignment.vm_blobs,
    azurerm_key_vault_secret.setup_token,
  ]
}
