output "cloud" {
  value = "gcp"
}

output "region" {
  value = var.region
}

output "project_id" {
  value = var.project_id
}

output "url" {
  description = "The app's Cloud Run URL."
  value       = module.app.url
}

output "service_name" {
  value = module.app.service_name
}

output "items_collection" {
  value = module.app.items_collection
}

output "queue" {
  value = module.app.queue
}

output "repository" {
  description = "Where run.sh pushes the bench image."
  value       = module.app.repository
}

output "results_url" {
  description = "Where the loadgen writes the raw results (done.json last)."
  value       = "gs://${google_storage_bucket.results.name}/${local.prefix}"
}

output "loadgen_instance" {
  value = var.loadgen ? google_compute_instance.loadgen[0].name : null
}

output "name_prefix" {
  value = local.name
}

output "free_tier" {
  value = module.app.free_tier
}
