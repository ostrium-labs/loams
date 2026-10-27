# Offline tests (mock providers): `tofu test` in this directory.

mock_provider "google" {
  mock_data "google_project" {
    defaults = { number = "123456789012" }
  }
  mock_resource "google_service_account" {
    defaults = {
      email  = "loams-t@p.iam.gserviceaccount.com"
      member = "serviceAccount:loams-t@p.iam.gserviceaccount.com"
      name   = "projects/p/serviceAccounts/loams-t@p.iam.gserviceaccount.com"
    }
  }
  mock_resource "google_cloud_run_v2_service" {
    defaults = { uri = "https://loams-t-abc-uc.a.run.app" }
  }
}

variables {
  project_id      = "p"
  name            = "t"
  run_id          = "cb1-r1"
  billing_account = "012345-6789AB-CDEF01"
  image           = "us-central1-docker.pkg.dev/p/loams-t/app@sha256:0000000000000000000000000000000000000000000000000000000000000000"
}

run "free_by_default_with_caps" {
  command = plan

  assert {
    condition     = google_cloud_run_v2_service.this.scaling[0].max_instance_count == 2 && google_cloud_run_v2_service.this.template[0].scaling[0].max_instance_count == 2
    error_message = "Cloud Run is capped at 2 instances, on the service and the revision"
  }
  assert {
    condition     = google_cloud_run_v2_service.this.template[0].scaling[0].min_instance_count == 0
    error_message = "no minimum instances: scale to zero"
  }
  assert {
    condition     = google_cloud_run_v2_service.this.template[0].containers[0].resources[0].cpu_idle && !google_cloud_run_v2_service.this.template[0].containers[0].resources[0].startup_cpu_boost
    error_message = "request-based billing, no startup boost"
  }
  assert {
    condition     = google_firestore_database.default[0].name == "(default)" && google_firestore_database.default[0].point_in_time_recovery_enablement == "POINT_IN_TIME_RECOVERY_DISABLED"
    error_message = "the free (default) Firestore database, no PITR"
  }
  assert {
    condition     = google_firestore_database.default[0].deletion_policy == "DELETE"
    error_message = "destroy deletes the database it created"
  }
  assert {
    condition     = google_cloud_tasks_queue.this[0].rate_limits[0].max_concurrent_dispatches == 2 && google_cloud_tasks_queue.this[0].rate_limits[0].max_dispatches_per_second == 10
    error_message = "the queue's dispatch is capped"
  }
  assert {
    condition     = google_cloud_run_v2_service.this.invoker_iam_disabled
    error_message = "public by default (D1409), without an allUsers binding"
  }
  assert {
    condition     = length(google_storage_bucket.blobs) == 0 && length(google_cloud_scheduler_job.this) == 0
    error_message = "no bucket and no scheduler job unless asked"
  }
}

run "budget_alerts_and_project_filter" {
  command = plan

  assert {
    condition     = length(google_billing_budget.this.threshold_rules) == 4
    error_message = "four alerts: 50, 80, 100 actual and 100 forecast"
  }
  assert {
    condition     = contains(google_billing_budget.this.budget_filter[0].projects, "projects/123456789012")
    error_message = "the budget is scoped to the project"
  }
  assert {
    condition     = google_billing_budget.this.amount[0].specified_amount[0].units == "1"
    error_message = "a budget of 1 by default"
  }
}

run "every_labelled_resource_is_labelled" {
  command = plan
  variables {
    create_bucket     = true
    create_repository = true
  }

  assert {
    condition = alltrue([for l in [
      google_cloud_run_v2_service.this.labels, google_cloud_run_v2_service.this.template[0].labels,
      google_storage_bucket.blobs[0].labels, google_artifact_registry_repository.this[0].labels,
    ] : l["loams-bench"] == "cb1-r1" && l["loams-app"] == "t"])
    error_message = "every resource that takes labels carries loams-bench and loams-app"
  }
  assert {
    condition     = google_storage_bucket.blobs[0].public_access_prevention == "enforced" && google_storage_bucket.blobs[0].location == "US-CENTRAL1"
    error_message = "the bucket is private and in a free region"
  }
}

run "non_free_region_refused" {
  command = plan
  variables {
    region = "europe-west1"
  }
  expect_failures = [var.region]
}

run "too_many_instances_refused" {
  command = plan
  variables {
    max_instances = 10
  }
  expect_failures = [var.max_instances]
}

run "existing_firestore_is_not_managed" {
  command = plan
  variables {
    create_firestore = false
  }

  assert {
    condition     = length(google_firestore_database.default) == 0
    error_message = "an existing (default) database is used, never created or deleted"
  }
}
