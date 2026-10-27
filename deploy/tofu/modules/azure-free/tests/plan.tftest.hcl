# Offline tests (mock providers): `tofu test` in this directory.

mock_provider "azurerm" {
  mock_resource "azurerm_resource_group" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-loams-t" }
  }
  mock_resource "azurerm_storage_account" {
    defaults = {
      id                     = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-loams-t/providers/Microsoft.Storage/storageAccounts/loamst0a1b2c3d"
      primary_blob_endpoint  = "https://loamst0a1b2c3d.blob.core.windows.net/"
      primary_queue_endpoint = "https://loamst0a1b2c3d.queue.core.windows.net/"
    }
  }
  mock_resource "azurerm_user_assigned_identity" {
    defaults = {
      id           = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-loams-t/providers/Microsoft.ManagedIdentity/userAssignedIdentities/id-loams-t"
      principal_id = "11111111-1111-1111-1111-111111111111"
      client_id    = "22222222-2222-2222-2222-222222222222"
    }
  }
  mock_resource "azurerm_cosmosdb_account" {
    defaults = {
      id       = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-loams-t/providers/Microsoft.DocumentDB/databaseAccounts/loams-t-0a1b2c3d"
      endpoint = "https://loams-t-0a1b2c3d.documents.azure.com:443/"
    }
  }
  mock_resource "azurerm_log_analytics_workspace" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-loams-t/providers/Microsoft.OperationalInsights/workspaces/log-loams-t" }
  }
  mock_resource "azurerm_service_plan" {
    defaults = { id = "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/rg-loams-t/providers/Microsoft.Web/serverFarms/asp-loams-t" }
  }
}
mock_provider "random" {
  mock_resource "random_id" {
    defaults = { hex = "0a1b2c3d" }
  }
}
mock_provider "time" {}

variables {
  name          = "t"
  run_id        = "cb1-r1"
  budget_emails = ["ops@example.com"]
  package_path  = "tests/package.zip"
}

run "free_by_default_with_caps" {
  command = plan

  assert {
    condition     = azurerm_cosmosdb_account.this.free_tier_enabled && azurerm_cosmosdb_account.this.capacity[0].total_throughput_limit == 1000
    error_message = "Cosmos DB uses the free tier and the account can never exceed 1,000 RU/s"
  }
  assert {
    condition     = azurerm_cosmosdb_sql_database.this.throughput == 1000 && length(azurerm_cosmosdb_sql_database.this.autoscale_settings) == 0
    error_message = "fixed, shared 1,000 RU/s; no autoscale"
  }
  assert {
    condition     = !azurerm_cosmosdb_account.this.local_authentication_enabled
    error_message = "Cosmos DB keys are off (Entra ID only)"
  }
  assert {
    condition     = azurerm_function_app_flex_consumption.this.maximum_instance_count == 2 && azurerm_function_app_flex_consumption.this.instance_memory_in_mb == 512
    error_message = "Flex is capped at 2 instances of 512 MB"
  }
  assert {
    condition     = length(azurerm_function_app_flex_consumption.this.always_ready) == 0
    error_message = "no always-ready instances (they have no free grant)"
  }
  assert {
    condition     = azurerm_service_plan.this.sku_name == "FC1"
    error_message = "the Flex Consumption plan"
  }
  assert {
    condition     = !azurerm_storage_account.this.shared_access_key_enabled && azurerm_storage_account.this.account_replication_type == "LRS"
    error_message = "storage without shared keys, LRS"
  }
  assert {
    condition     = azurerm_log_analytics_workspace.this[0].daily_quota_gb == 0.1 && azurerm_application_insights.this[0].daily_data_cap_in_gb == 0.1
    error_message = "log ingestion is capped at 0.1 GB a day"
  }
  assert {
    condition     = azurerm_cosmosdb_account.this.backup[0].type == "Periodic" && azurerm_cosmosdb_account.this.backup[0].storage_redundancy == "Local"
    error_message = "only the free periodic backup, locally redundant"
  }
}

run "budget_alerts_at_50_80_100_and_forecast" {
  command = plan

  assert {
    condition     = azurerm_consumption_budget_resource_group.this.amount == 1 && azurerm_consumption_budget_resource_group.this.time_grain == "Monthly"
    error_message = "a monthly budget of 1 by default"
  }
  assert {
    condition     = length(azurerm_consumption_budget_resource_group.this.notification) == 4
    error_message = "four alerts: 50, 80, 100 actual and 100 forecast"
  }
}

run "every_resource_is_tagged" {
  command = plan

  assert {
    condition = alltrue([for t in [
      azurerm_resource_group.this.tags, azurerm_user_assigned_identity.app.tags, azurerm_storage_account.this.tags,
      azurerm_cosmosdb_account.this.tags, azurerm_log_analytics_workspace.this[0].tags,
      azurerm_application_insights.this[0].tags, azurerm_service_plan.this.tags,
      azurerm_function_app_flex_consumption.this.tags,
    ] : t["loams-bench"] == "cb1-r1" && t["loams-app"] == "t"])
    error_message = "every taggable resource carries loams-bench and loams-app"
  }
}

run "over_free_throughput_refused" {
  command = plan
  variables {
    cosmos_throughput = 1200
  }
  expect_failures = [var.cosmos_throughput]
}

run "no_free_tier_refused" {
  command = plan
  variables {
    cosmos_free_tier = false
  }
  expect_failures = [var.cosmos_free_tier]
}

run "too_many_instances_refused" {
  command = plan
  variables {
    max_instances = 10
  }
  expect_failures = [var.max_instances]
}
