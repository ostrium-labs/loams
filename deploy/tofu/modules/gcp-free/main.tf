# App BYOC on Google Cloud's free tier (design 71 §7.3, D1395, D1400, D1402):
# Cloud Run with request-based billing and no minimum instances, the project's
# (default) Firestore database, Cloud Tasks, optionally Cloud Scheduler, GCS and an
# Artifact Registry repository. The budget is created first; every other resource
# depends on it. No VPC connector, no NAT, no static IP.

data "google_project" "this" {
  project_id = var.project_id
}

locals {
  fullname = "loams-${var.name}"
  labels = merge(
    var.labels,
    { "loams-app" = var.name, "loams-managed-by" = "opentofu" },
    var.run_id == null ? {} : { "loams-bench" = var.run_id },
  )
  apis = concat(
    ["run.googleapis.com", "firestore.googleapis.com", "iam.googleapis.com", "billingbudgets.googleapis.com"],
    var.create_queue ? ["cloudtasks.googleapis.com"] : [],
    var.schedule_cron == null ? [] : ["cloudscheduler.googleapis.com"],
    var.create_bucket ? ["storage.googleapis.com"] : [],
    var.create_repository ? ["artifactregistry.googleapis.com"] : [],
  )
  queue_name = "${local.fullname}-queue"
}

resource "google_project_service" "this" {
  for_each           = var.enable_apis ? toset(local.apis) : toset([])
  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}

# --- the budget, first (D1402) -------------------------------------------------
resource "google_billing_budget" "this" {
  billing_account = var.billing_account
  display_name    = "${local.fullname} (${var.project_id})"

  budget_filter {
    projects               = ["projects/${data.google_project.this.number}"]
    credit_types_treatment = "EXCLUDE_ALL_CREDITS" # alert on cost before credits
    calendar_period        = "MONTH"
  }

  amount {
    specified_amount {
      currency_code = var.budget_currency
      units         = tostring(var.budget_amount)
    }
  }

  dynamic "threshold_rules" {
    for_each = [0.5, 0.8, 1.0]
    content {
      threshold_percent = threshold_rules.value
      spend_basis       = "CURRENT_SPEND"
    }
  }
  threshold_rules {
    threshold_percent = 1.0
    spend_basis       = "FORECASTED_SPEND"
  }

  all_updates_rule {
    monitoring_notification_channels = var.budget_notification_channels
    disable_default_iam_recipients   = false
  }

  depends_on = [google_project_service.this]
}

# --- the app's identity ------------------------------------------------------------
resource "google_service_account" "app" {
  project      = var.project_id
  account_id   = local.fullname
  display_name = "Loams app ${var.name}"
  depends_on   = [google_billing_budget.this]
}

resource "google_project_iam_member" "items" {
  project = var.project_id
  role    = "roles/datastore.user"
  member  = google_service_account.app.member
}

resource "google_project_iam_member" "enqueue" {
  count   = var.create_queue ? 1 : 0
  project = var.project_id
  role    = "roles/cloudtasks.enqueuer"
  member  = google_service_account.app.member
}

# Tasks carry an OIDC token for the app's own account, so the app must act as it.
resource "google_service_account_iam_member" "act_as_self" {
  count              = var.create_queue || var.schedule_cron != null ? 1 : 0
  service_account_id = google_service_account.app.name
  role               = "roles/iam.serviceAccountUser"
  member             = google_service_account.app.member
}

# --- items: the (default) Firestore database ----------------------------------------
resource "google_firestore_database" "default" {
  count                             = var.create_firestore ? 1 : 0
  project                           = var.project_id
  name                              = "(default)" # the only database with a free quota
  location_id                       = var.region
  type                              = "FIRESTORE_NATIVE"
  delete_protection_state           = "DELETE_PROTECTION_DISABLED"
  deletion_policy                   = "DELETE"
  point_in_time_recovery_enablement = "POINT_IN_TIME_RECOVERY_DISABLED" # no backups
  depends_on                        = [google_billing_budget.this]
}

# --- blobs: GCS, optional --------------------------------------------------------------
resource "google_storage_bucket" "blobs" {
  count                       = var.create_bucket ? 1 : 0
  project                     = var.project_id
  name                        = "${local.fullname}-${data.google_project.this.number}"
  location                    = upper(var.region)
  storage_class               = "STANDARD"
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = var.bucket_force_destroy
  labels                      = local.labels
  depends_on                  = [google_billing_budget.this]
}

resource "google_storage_bucket_iam_member" "blobs" {
  count  = var.create_bucket ? 1 : 0
  bucket = google_storage_bucket.blobs[0].name
  role   = "roles/storage.objectUser"
  member = google_service_account.app.member
}

# --- Artifact Registry, optional --------------------------------------------------------
resource "google_artifact_registry_repository" "this" {
  count         = var.create_repository ? 1 : 0
  project       = var.project_id
  location      = var.region
  repository_id = local.fullname
  format        = "DOCKER"
  labels        = local.labels
  depends_on    = [google_billing_budget.this]
}

# --- compute: Cloud Run, request-based billing, scale to zero ---------------------------
resource "google_cloud_run_v2_service" "this" {
  project              = var.project_id
  name                 = local.fullname
  location             = var.region
  ingress              = "INGRESS_TRAFFIC_ALL"
  invoker_iam_disabled = var.public
  deletion_protection  = false
  labels               = local.labels

  scaling {
    max_instance_count = var.max_instances # the cap across every revision
  }

  template {
    service_account                  = google_service_account.app.email
    max_instance_request_concurrency = var.concurrency
    timeout                          = "${var.timeout_s}s"
    labels                           = local.labels

    scaling {
      min_instance_count = 0
      max_instance_count = var.max_instances
    }

    containers {
      image = var.image
      resources {
        limits            = { cpu = var.cpu, memory = var.memory }
        cpu_idle          = true # request-based billing: CPU only while serving
        startup_cpu_boost = false
      }
      dynamic "env" {
        for_each = merge(var.environment, {
          LOAMS_APP_TARGET   = "gcp"
          LOAMS_PROJECT      = var.project_id
          LOAMS_REGION       = var.region
          LOAMS_ITEMS        = "${local.fullname}-items"
          LOAMS_QUEUE        = var.create_queue ? "projects/${var.project_id}/locations/${var.region}/queues/${local.queue_name}" : ""
          LOAMS_BUCKET       = var.create_bucket ? google_storage_bucket.blobs[0].name : ""
          LOAMS_TASKS_SA     = google_service_account.app.email
          LOAMS_TASKS_TARGET = "/_loams/queue"
        })
        content {
          name  = env.key
          value = env.value
        }
      }
    }
  }

  depends_on = [google_project_iam_member.items, google_firestore_database.default]
}

resource "google_cloud_run_v2_service_iam_member" "self_invoke" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.this.name
  role     = "roles/run.invoker"
  member   = google_service_account.app.member
}

# --- queue: Cloud Tasks, pushing to the service ---------------------------------------
resource "google_cloud_tasks_queue" "this" {
  count    = var.create_queue ? 1 : 0
  project  = var.project_id
  name     = local.queue_name
  location = var.region

  rate_limits {
    max_dispatches_per_second = var.queue_max_dispatches_per_second
    max_concurrent_dispatches = var.queue_max_concurrent
  }

  retry_config {
    max_attempts = var.queue_max_retries
    min_backoff  = "1s"
    max_backoff  = "60s"
  }

  http_target {
    http_method = "POST"
    uri_override {
      scheme = "HTTPS"
      host   = trimprefix(google_cloud_run_v2_service.this.uri, "https://")
      path_override {
        path = "/_loams/queue"
      }
    }
    oidc_token {
      service_account_email = google_service_account.app.email
      audience              = google_cloud_run_v2_service.this.uri
    }
  }
}

# --- schedule: Cloud Scheduler, optional ------------------------------------------------
resource "google_cloud_scheduler_job" "this" {
  count     = var.schedule_cron == null ? 0 : 1
  project   = var.project_id
  region    = var.region
  name      = local.fullname
  schedule  = var.schedule_cron
  time_zone = "Etc/UTC"

  http_target {
    http_method = "POST"
    uri         = "${google_cloud_run_v2_service.this.uri}/_loams/scheduled"
    oidc_token {
      service_account_email = google_service_account.app.email
      audience              = google_cloud_run_v2_service.this.uri
    }
  }
}
