output "cloud" {
  value = "azure"
}

output "region" {
  value = var.location
}

output "url" {
  description = "The function app's URL (host.json sets the route prefix to \"\")."
  value       = module.app.url
}

output "resource_group" {
  value = local.rg
}

output "function_app_name" {
  value = module.app.function_app_name
}

output "function_app_id" {
  value = local.function_app_id
}

output "storage_account" {
  value = module.app.storage_account
}

output "results_url" {
  description = "Where the loadgen writes the raw results (done.json last)."
  value       = "${data.azurerm_storage_account.app.primary_blob_endpoint}${azurerm_storage_container.results.name}/${local.prefix}"
}

output "results_container" {
  value = azurerm_storage_container.results.name
}

output "loadgen_vm_id" {
  value = azurerm_linux_virtual_machine.loadgen.id
}

output "name_prefix" {
  value = local.name
}

output "free_tier" {
  value = module.app.free_tier
}
