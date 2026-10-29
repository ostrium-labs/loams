# A minimal root module for ../../modules/azure-free: an app on Azure's free
# allowances (design 71 §7.4). Build the app's zip first (the app bundled with
# the Azure Functions shim), then:
#   tofu init && tofu apply -var package_path=dist/azure.zip -var 'budget_emails=["you@example.com"]'
# The subscription may hold only one Cosmos DB free-tier account.

terraform {
  required_version = ">= 1.9"
  required_providers {
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 5.0"
    }
  }
}

variable "package_path" {
  type = string
}

variable "budget_emails" {
  type = list(string)
}

provider "azurerm" {
  features {}
  storage_use_azuread = true # the storage account has shared keys turned off
}

module "app" {
  source        = "../../modules/azure-free"
  name          = "notes"
  package_path  = var.package_path
  budget_emails = var.budget_emails
}

output "url" {
  value = module.app.url
}

output "free_tier" {
  value = module.app.free_tier
}
