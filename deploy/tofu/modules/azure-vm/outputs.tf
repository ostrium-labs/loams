output "vm_id" {
  description = "The virtual machine."
  value       = azurerm_linux_virtual_machine.this.id
}

output "bucket_url" {
  description = "The engine's LOAMS_BUCKET."
  value       = "az://${local.container}/loams"
}

output "storage_account" {
  description = "The storage account (AZURE_STORAGE_ACCOUNT_NAME)."
  value       = azurerm_storage_account.this.name
}

output "key_vault" {
  description = "The Key Vault holding the secrets. Create <secret_prefix>-tailnet-authkey in it after apply."
  value       = local.vault_name
}

output "secret_prefix" {
  description = "The Key Vault secret name prefix the install reads."
  value       = local.prefix
}

output "tailnet_url_hint" {
  description = "Where the engine appears once the VM has joined your tailnet."
  value       = "https://${var.hostname}.<your-tailnet>.ts.net (see journalctl -u loams-byoc on the VM)"
}

output "estimated_monthly_usd" {
  description = "A rough monthly cost in USD (East US prices of 2026-10-21; excludes data transfer)."
  value       = local.estimated_monthly_usd
}
