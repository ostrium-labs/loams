output "instance_name" {
  description = "The Compute Engine instance."
  value       = google_compute_instance.this.name
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
  description = "The VM's service account (it has no key)."
  value       = google_service_account.vm.email
}

output "secret_prefix" {
  description = "The Secret Manager name prefix the install reads."
  value       = local.prefix
}

output "tailnet_url_hint" {
  description = "Where the engine appears once the VM has joined your tailnet."
  value       = "https://${var.hostname}.<your-tailnet>.ts.net (see journalctl -u loams-byoc on the VM)"
}

output "estimated_monthly_usd" {
  description = "A rough monthly cost in USD (us-central1 prices of 2026-10-21; excludes data transfer)."
  value       = local.estimated_monthly_usd
}

output "tailnet_authkey_secret" {
  description = "The secret the VM reads the Tailscale auth key from (it must exist before apply; its value is never in this configuration)."
  value       = data.google_secret_manager_secret.tailnet_authkey.id
}
