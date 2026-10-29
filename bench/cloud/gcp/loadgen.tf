# The in-region loadgen (plan §5.4): e2-standard-2 on Debian 12 in its own VPC
# with no firewall rule (Google's implied rules deny all ingress), an ephemeral
# external IPv4 for egress (no Cloud NAT). Its service account may update only
# the bench service (the cold scenario) and write to the results bucket. The VM
# is deleted by Compute Engine at the deadline (max_run_duration), and powers
# itself off sooner when the run finishes.

resource "google_compute_network" "loadgen" {
  count                   = var.loadgen ? 1 : 0
  name                    = "${local.name}-loadgen"
  auto_create_subnetworks = false
  depends_on              = [module.app, google_project_service.bench]
}

resource "google_compute_subnetwork" "loadgen" {
  count         = var.loadgen ? 1 : 0
  name          = "${local.name}-loadgen"
  network       = google_compute_network.loadgen[0].id
  region        = var.region
  ip_cidr_range = "10.90.0.0/24"
}

resource "google_service_account" "loadgen" {
  count        = var.loadgen ? 1 : 0
  account_id   = "${local.name}-lg"
  display_name = "CB1 loadgen ${var.run_id}"
  depends_on   = [module.app, google_project_service.bench]
}

# gcloud run services update: run.developer on the service, and actAs on the
# service's own account (a new revision runs as it).
resource "google_cloud_run_v2_service_iam_member" "loadgen" {
  count    = var.loadgen ? 1 : 0
  name     = module.app.service_name
  location = var.region
  role     = "roles/run.developer"
  member   = "serviceAccount:${google_service_account.loadgen[0].email}"
}

resource "google_service_account_iam_member" "loadgen_act_as_app" {
  count              = var.loadgen ? 1 : 0
  service_account_id = "projects/${var.project_id}/serviceAccounts/${module.app.service_account}"
  role               = "roles/iam.serviceAccountUser"
  member             = "serviceAccount:${google_service_account.loadgen[0].email}"
}

resource "google_storage_bucket_iam_member" "loadgen" {
  count  = var.loadgen ? 1 : 0
  bucket = google_storage_bucket.results.name
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.loadgen[0].email}"
}

locals {
  loadgen_files = {
    for f in ["run-scenarios.sh", "cold-gcp.sh", "upload-gcp.sh"] :
    f => base64gzip(file("${path.module}/../loadgen/${f}"))
  }
  loadgen_env = merge(var.bench_env, {
    BENCH_CLOUD           = "gcp"
    BENCH_RUN_ID          = var.run_id
    BENCH_ROW             = var.row
    BENCH_URL             = module.app.url
    BENCH_REGION          = var.region
    BENCH_INSTANCE_TYPE   = var.loadgen_machine_type
    BENCH_COMMIT          = var.commit
    BENCH_SERVICE         = module.app.service_name
    BENCH_BUCKET          = google_storage_bucket.results.name
    CLOUDSDK_CORE_PROJECT = var.project_id
  })
}

resource "google_compute_instance" "loadgen" {
  count        = var.loadgen ? 1 : 0
  name         = "${local.name}-loadgen"
  machine_type = var.loadgen_machine_type
  zone         = var.zone

  boot_disk {
    auto_delete = true
    initialize_params {
      image = "debian-cloud/debian-12"
      size  = 10
      type  = "pd-balanced"
    }
  }

  network_interface {
    subnetwork = google_compute_subnetwork.loadgen[0].id
    access_config {} # an ephemeral external IPv4, egress only (no firewall rule admits ingress)
  }

  service_account {
    email  = google_service_account.loadgen[0].email
    scopes = ["cloud-platform"] # narrowed by the account's roles above
  }

  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }

  # The hard deadline (plan §7.2): Compute Engine deletes the VM after
  # max_minutes even if nothing else does. (verify: standard VMs accept
  # max_run_duration with DELETE in the project's region.)
  scheduling {
    provisioning_model          = "STANDARD"
    automatic_restart           = false
    on_host_maintenance         = "MIGRATE"
    instance_termination_action = "DELETE"
    max_run_duration {
      seconds = var.max_minutes * 60
    }
  }

  metadata = {
    block-project-ssh-keys = "true"
    startup-script = templatefile("${path.module}/../loadgen/boot.sh.tftpl", {
      cloud       = "gcp"
      max_minutes = var.max_minutes
      oha_url     = var.oha_url
      oha_sha256  = var.oha_sha256
      files       = local.loadgen_files
      env         = local.loadgen_env
    })
  }

  depends_on = [
    module.app,
    google_cloud_run_v2_service_iam_member.loadgen,
    google_service_account_iam_member.loadgen_act_as_app,
    google_storage_bucket_iam_member.loadgen,
  ]
}
