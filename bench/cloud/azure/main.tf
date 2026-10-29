# CB1 root module for the azure-free row (plan §5.1): the azure-free module
# (resource group and its budget first; Functions Flex, Cosmos DB free tier,
# Queue Storage) plus a results container and the in-region loadgen VM, all in
# the module's resource group so that destroying it removes everything.
# Applied and destroyed by bench/cloud/run.sh; never left running.

provider "azurerm" {
  features {
    resource_group {
      # The group is the run's: deleting it with whatever is left in it is the point.
      prevent_deletion_if_contains_resources = false
    }
  }
  storage_use_azuread = true # the storage account has shared keys turned off
}

data "azurerm_client_config" "current" {}

locals {
  name   = "b-${var.run_id}"
  prefix = "${var.run_id}/${var.row}/"
  tags   = { "loams-bench" = var.run_id, "loams-app" = local.name }
  rg     = module.app.resource_group
}

module "app" {
  source        = "../../../deploy/tofu/modules/azure-free"
  name          = local.name
  location      = var.location
  run_id        = var.run_id
  budget_emails = var.budget_emails
  package_path  = var.package_path
  max_instances = 2 # plan §5.1
}

data "azurerm_storage_account" "app" {
  name                = module.app.storage_account
  resource_group_name = local.rg
}

# --- the results container, in the app's storage account (paid, cents) -----------------
resource "azurerm_storage_container" "results" {
  name                  = "bench-results"
  storage_account_id    = data.azurerm_storage_account.app.id
  container_access_type = "private"
  depends_on            = [module.app]
}

# run.sh downloads the results as the identity that applied the run.
resource "azurerm_role_assignment" "results_reader" {
  scope                = azurerm_storage_container.results.id
  role_definition_name = "Storage Blob Data Reader"
  principal_id         = data.azurerm_client_config.current.object_id
}
