# CB1 root module for the gcp-free row (plan §5.1): the gcp-free module (budget
# first; Cloud Run, Firestore (default), Cloud Tasks, the image repository) plus
# a results bucket and the in-region loadgen VM. Applied and destroyed by
# bench/cloud/run.sh; never left running.

provider "google" {
  project = var.project_id
  region  = var.region
  # Every labelled resource carries the run's label, which sweep.sh lists.
  default_labels = { "loams-bench" = var.run_id }
}

locals {
  name   = "b-${var.run_id}"
  prefix = "${var.run_id}/${var.row}/"
}

module "app" {
  source            = "../../../deploy/tofu/modules/gcp-free"
  project_id        = var.project_id
  region            = var.region
  name              = local.name
  run_id            = var.run_id
  billing_account   = var.billing_account
  image             = var.image
  create_firestore  = var.create_firestore
  create_repository = true
  max_instances     = 2 # plan §5.1
  cpu               = "1"
  memory            = "512Mi"
  concurrency       = 80
}

resource "google_project_service" "bench" {
  for_each           = toset(["compute.googleapis.com", "storage.googleapis.com", "iam.googleapis.com"])
  service            = each.value
  disable_on_destroy = false # enabled APIs are a known leftover that costs nothing (plan §7.6)
}

# --- the results bucket ---------------------------------------------------------
resource "google_storage_bucket" "results" {
  name                        = "${var.project_id}-loams-bench-${var.run_id}"
  location                    = upper(var.region) # a free-tier region (5 GB-month)
  force_destroy               = true
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  lifecycle_rule {
    condition {
      age = 7
    }
    action {
      type = "Delete"
    }
  }
  depends_on = [module.app, google_project_service.bench]
}
