variable "name" {
  description = "Names the VM, identities, network and (with a random suffix) the storage account and vault."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z][a-z0-9]{1,9}$", var.name))
    error_message = "name is 2 to 10 lowercase letters and digits, starting with a letter (storage account names allow no dash)."
  }
}

variable "location" {
  description = "The Azure region."
  type        = string
  default     = "eastus"
}

variable "resource_group_name" {
  description = "An existing resource group. null creates loams-<name>."
  type        = string
  default     = null
}

variable "release" {
  description = "The Loams release to install. The bundle's signature is checked on the VM."
  type        = string
  default     = "0.1.4"
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release is a version such as 0.1.4, never latest."
  }
}

variable "size" {
  description = "small (Standard_D2s_v5, 2 vCPU and 8 GiB) or medium (Standard_D4s_v5, 4 vCPU and 16 GiB)."
  type        = string
  default     = "small"
  validation {
    condition     = contains(["small", "medium"], var.size)
    error_message = "size is small or medium."
  }
}

variable "vm_size" {
  description = "Overrides size's VM size (x86-64 only until every image is arm64, D1384; Standard_B2ms suits an idle trial)."
  type        = string
  default     = null
}

variable "disk_size_gb" {
  description = "The Standard SSD OS disk, which holds Docker's volumes (the stores)."
  type        = number
  default     = 64
  validation {
    condition     = var.disk_size_gb >= 30 && var.disk_size_gb <= 4095
    error_message = "disk_size_gb is between 30 and 4095."
  }
}

variable "admin_ssh_public_key" {
  description = "An SSH public key for the VM's admin user (Azure requires one). Nothing can reach SSH: the NSG allows no inbound; use `az vm run-command` or Bastion for a shell."
  type        = string
}

variable "profiles" {
  description = "COMPOSE_PROFILES (tikv, rustfs, ...). Choose before the first start: the engine refuses to switch stores on existing data."
  type        = string
  default     = ""
}

variable "hostname" {
  description = "The engine's tailnet name (https://<hostname>.<tailnet>.ts.net)."
  type        = string
  default     = "loams"
}

variable "key_vault_name" {
  description = "An existing Key Vault (Azure RBAC mode) that holds the secrets; the deployer needs Key Vault Administrator on it. null creates one (and the bucket's key in it)."
  type        = string
  default     = null
}

variable "purge_protection" {
  description = "Purge protection on the vault the module creates. The storage account's customer-managed key requires it; destroying the vault then only soft-deletes it."
  type        = bool
  default     = true
}

variable "secret_prefix" {
  description = "Key Vault secret names are <prefix>-tailnet-authkey (create it after apply with your Tailscale auth key, never in this module's variables), <prefix>-setup-token and optionally <prefix>-api-keys. null is loams-<name>."
  type        = string
  default     = null
}

variable "create_setup_token" {
  description = "Create <prefix>-setup-token with a random value. The value is in this configuration's state; set false to create the secret yourself."
  type        = bool
  default     = true
}

variable "address_space" {
  description = "The new virtual network's range."
  type        = string
  default     = "10.20.0.0/24"
}

variable "tags" {
  description = "Extra tags."
  type        = map(string)
  default     = {}
}

variable "confluent_integrations" {
  description = "Confluent integrations whose keys the VM fetches from the secret store into the connect role's secret files (names only; design 80 §7.2). Create the keys with `loams byoc confluent key create`."
  type        = list(string)
  default     = []
}
