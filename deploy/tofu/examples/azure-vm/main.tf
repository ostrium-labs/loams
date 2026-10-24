# A minimal root module for ../../modules/azure-vm. After `tofu apply`, put the
# Tailscale auth key in the vault yourself (never in a variable or in this directory):
#   az keyvault secret set --vault-name "$(tofu output -raw key_vault)" \
#     --name loams-prod-tailnet-authkey --value "$TS_AUTHKEY"
# The VM retries until the secret exists.

terraform {
  required_version = ">= 1.6"
  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 5.0"
    }
  }
}

variable "subscription_id" {
  type = string
}

variable "admin_ssh_public_key" {
  type = string
}

provider "azurerm" {
  subscription_id = var.subscription_id
  features {}
}

module "loams" {
  source               = "../../modules/azure-vm"
  name                 = "prod"
  admin_ssh_public_key = var.admin_ssh_public_key
}

output "bucket_url" {
  value = module.loams.bucket_url
}

output "key_vault" {
  value = module.loams.key_vault
}

output "estimated_monthly_usd" {
  value = module.loams.estimated_monthly_usd
}
