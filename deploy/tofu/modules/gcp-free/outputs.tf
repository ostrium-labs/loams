output "url" {
  description = "The app's Cloud Run URL."
  value       = google_cloud_run_v2_service.this.uri
}

output "service_name" {
  description = "The Cloud Run service."
  value       = google_cloud_run_v2_service.this.name
}

output "service_account" {
  description = "The app's service account."
  value       = google_service_account.app.email
}

output "items_collection" {
  description = "The Firestore collection behind the items port, in the (default) database."
  value       = "${local.fullname}-items"
}

output "queue" {
  description = "The Cloud Tasks queue behind the queue port (null without one)."
  value       = var.create_queue ? google_cloud_tasks_queue.this[0].id : null
}

output "bucket" {
  description = "The blobs bucket (null without one)."
  value       = var.create_bucket ? google_storage_bucket.blobs[0].name : null
}

output "repository" {
  description = "The Docker repository's host path (null without one)."
  value       = var.create_repository ? "${var.region}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.this[0].repository_id}" : null
}

output "budget" {
  description = "The Cloud Billing budget."
  value       = google_billing_budget.this.id
}

output "labels" {
  description = "The labels on every resource that takes labels."
  value       = local.labels
}

output "free_tier" {
  description = "What this configuration uses of the free tier (design 71 §7.3)."
  value = {
    free = concat(
      ["Cloud Run: 2M requests, 180,000 vCPU-s, 360,000 GiB-s a month (billing account)",
      "Firestore (default): 1 GiB; 50,000 reads, 20,000 writes, 20,000 deletes a day"],
      var.create_queue ? ["Cloud Tasks: 1M operations a month"] : [],
      var.schedule_cron == null ? [] : ["Cloud Scheduler: 3 jobs per billing account"],
      var.create_bucket ? ["GCS: 5 GB-month in us-central1, us-east1, us-west1"] : [],
      var.create_repository ? ["Artifact Registry: 0.5 GB"] : [],
    )
    not_free = contains(["us-central1", "us-east1", "us-west1"], var.region) ? [] : ["GCS outside the free US regions"]
  }
}
