output "instance_name" {
  description = "The Compute Engine instance."
  value       = google_compute_instance.this.name
}

output "kubeconfig" {
  description = "An admin kubeconfig for the k3s API on the tailnet (valid one year). It works from a machine on the tailnet."
  value       = module.home.kubeconfig
  sensitive   = true
}

output "api_url" {
  description = "The k3s API on the tailnet."
  value       = module.home.api_url
}

output "cost_items" {
  description = "What the deployment costs per month, as priced lines (SKUs of deploy/tofu/prices.toml, units from its table); data transfer is not listed."
  value       = local.cost_items
}

output "bucket_url" {
  description = "The engine's LOAMS_BUCKET."
  value       = "gs://${local.bucket}/loams"
}

output "kms_key_id" {
  description = "The bucket's key (null if none)."
  value       = local.key_id
}

output "service_account_email" {
  description = "The VM's service account (it has no key); the home's workload identity."
  value       = google_service_account.vm.email
}

output "secret_prefix" {
  description = "The Secret Manager name prefix the VM reads."
  value       = local.prefix
}

output "tailnet_authkey_secret" {
  description = "The secret the VM reads the Tailscale auth key from (it must exist before apply; its value is never in this configuration)."
  value       = data.google_secret_manager_secret.tailnet_authkey.id
}

output "cloud" {
  description = "gcp."
  value       = "gcp"
}

output "region" {
  description = "The region of the subnet and the bucket."
  value       = var.region
}
