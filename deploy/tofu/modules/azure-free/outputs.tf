output "url" {
  description = "The function app's base URL (HTTP routes are under /api unless the app's host.json changes the prefix)."
  value       = "https://${azurerm_function_app_flex_consumption.this.default_hostname}"
}

output "function_app_name" {
  description = "The function app."
  value       = azurerm_function_app_flex_consumption.this.name
}

output "resource_group" {
  description = "The resource group that holds everything (destroying it removes all)."
  value       = azurerm_resource_group.this.name
}

output "identity_client_id" {
  description = "The app's user-assigned identity."
  value       = azurerm_user_assigned_identity.app.client_id
}

output "cosmos_endpoint" {
  description = "The Cosmos DB account behind the items port."
  value       = azurerm_cosmosdb_account.this.endpoint
}

output "items_container" {
  description = "The Cosmos DB container behind the items port."
  value       = azurerm_cosmosdb_sql_container.items.name
}

output "storage_account" {
  description = "The storage account (host, package, queue, blobs)."
  value       = azurerm_storage_account.this.name
}

output "queue" {
  description = "The storage queue behind the queue port (null without one)."
  value       = var.create_queue ? local.queue_name : null
}

output "budget_id" {
  description = "The resource group's budget."
  value       = azurerm_consumption_budget_resource_group.this.id
}

output "tags" {
  description = "The tags on every resource."
  value       = local.tags
}

output "free_tier" {
  description = "What this configuration uses of the free allowances, and what is not free (design 71 §7.4)."
  value = {
    free = concat(
      ["Functions Flex on-demand: 250,000 executions and 100,000 GB-s a month (subscription)"],
      var.cosmos_free_tier ? ["Cosmos DB free tier: ${var.cosmos_throughput} RU/s and 25 GB (one account per subscription)"] : [],
      var.create_app_insights ? ["Log Analytics: 5 GB a month (capped at ${var.log_daily_cap_gb} GB a day)"] : [],
    )
    not_free = ["Storage account: host, package and queue transactions (cents a month)"]
  }
}
