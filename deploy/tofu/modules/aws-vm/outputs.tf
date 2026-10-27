output "instance_id" {
  description = "The EC2 instance."
  value       = aws_instance.this.id
}

output "bucket_url" {
  description = "The engine's LOAMS_BUCKET."
  value       = "s3://${local.bucket}/loams"
}

output "kms_key_arn" {
  description = "The bucket's key (null if none)."
  value       = local.key_arn
}

output "secret_prefix" {
  description = "The Secrets Manager prefix the install reads."
  value       = local.prefix
}

output "tailnet_url_hint" {
  description = "Where the engine appears once the VM has joined your tailnet."
  value       = "https://${var.hostname}.<your-tailnet>.ts.net (see journalctl -u loams-byoc on the VM)"
}

output "estimated_monthly_usd" {
  description = "A rough monthly cost in USD (us-east-1 prices of 2026-10-21; excludes data transfer)."
  value       = local.estimated_monthly_usd
}

output "tailnet_authkey_secret_arn" {
  description = "The secret the VM reads the Tailscale auth key from (it must exist before apply; its value is never in this configuration)."
  value       = data.aws_secretsmanager_secret.tailnet_authkey.arn
}

output "cloud" {
  description = "aws (for modules that must sit next to the VM, such as confluent)."
  value       = "aws"
}

output "region" {
  description = "The VM's region (for modules that must sit next to it, such as confluent)."
  value       = local.region
}
