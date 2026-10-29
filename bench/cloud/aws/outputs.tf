output "cloud" {
  value = "aws"
}

output "region" {
  value = var.region
}

output "url" {
  description = "The app's Function URL."
  value       = module.app.url
}

output "function_name" {
  value = module.app.function_name
}

output "items_table" {
  value = module.app.items_table
}

output "queue_url" {
  value = module.app.queue_url
}

output "log_group" {
  value = "/aws/lambda/${module.app.function_name}"
}

output "memory_mb" {
  value = local.memory_mb
}

output "results_url" {
  description = "Where the loadgen writes the raw results (done.json last)."
  value       = "s3://${aws_s3_bucket.results.bucket}/${local.prefix}"
}

output "loadgen_instance_id" {
  value = aws_instance.loadgen.id
}

output "name_prefix" {
  description = "The prefix of every named resource (sweep.sh lists by it as well as by tag)."
  value       = local.name
}

output "free_tier" {
  value = module.app.free_tier
}
