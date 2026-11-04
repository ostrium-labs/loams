output "instance_id" {
  description = "The EC2 instance."
  value       = aws_instance.this.id
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
  value       = "s3://${local.bucket}/loams"
}

output "kms_key_arn" {
  description = "The bucket's and secrets' key (null if none)."
  value       = local.key_arn
}

output "secret_prefix" {
  description = "The Secrets Manager prefix the VM reads."
  value       = local.prefix
}

output "tailnet_authkey_secret_arn" {
  description = "The secret the VM reads the Tailscale auth key from (it must exist before apply; its value is never in this configuration)."
  value       = data.aws_secretsmanager_secret.tailnet_authkey.arn
}

output "cloud" {
  description = "aws."
  value       = "aws"
}

output "region" {
  description = "The VM's region."
  value       = local.region
}
