data "google_project" "this" {
  project_id = var.project
}

locals {
  create_bucket = var.bucket == null
  bucket        = local.create_bucket ? "loams-${var.name}-${var.project}" : var.bucket
  create_key    = var.kms_key_id == null && local.create_bucket
  key_id        = local.create_key ? google_kms_crypto_key.bucket[0].id : var.kms_key_id
  prefix        = coalesce(var.secret_prefix, "loams-${var.name}")
  machine_type  = coalesce(var.machine_type, var.size == "small" ? "e2-standard-2" : "e2-standard-4")
  labels        = merge({ loams-install = var.name, loams-managed-by = "opentofu" }, var.labels)

  # Estimates, us-central1 on-demand prices of 2026-10-21 (design 70 §6.2): the
  # machine for 730 hours, the pd-balanced disk, 50 GiB of GCS, a key, an IPv4.
  hourly = {
    "e2-standard-2" = 0.06701
    "e2-standard-4" = 0.13402
  }
  estimated_monthly_usd = floor(lookup(local.hourly, local.machine_type, 0.1) * 730 + var.disk_size_gb * 0.10 + 1.0 + (local.create_key ? 0.06 : 0) + 3.65)
}

resource "google_project_service" "apis" {
  for_each = var.enable_apis ? toset([
    "compute.googleapis.com",
    "storage.googleapis.com",
    "secretmanager.googleapis.com",
    "cloudkms.googleapis.com",
    "iam.googleapis.com",
  ]) : toset([])
  project            = var.project
  service            = each.value
  disable_on_destroy = false
}

# --- the identity: a service account with no key ----------------------------------
resource "google_service_account" "vm" {
  project      = var.project
  account_id   = "loams-${var.name}"
  display_name = "Loams ${var.name} VM"
  depends_on   = [google_project_service.apis]
}

# --- the bucket, private and versioned, with a customer-managed key ----------------
data "google_storage_project_service_account" "gcs" {
  project = var.project
}

resource "google_kms_key_ring" "bucket" {
  count      = local.create_key ? 1 : 0
  project    = var.project
  name       = "loams-${var.name}"
  location   = var.region
  depends_on = [google_project_service.apis]
}

resource "google_kms_crypto_key" "bucket" {
  count           = local.create_key ? 1 : 0
  name            = "bucket"
  key_ring        = google_kms_key_ring.bucket[0].id
  rotation_period = "7776000s"
  labels          = local.labels
}

resource "google_kms_crypto_key_iam_member" "gcs_agent" {
  count         = local.create_key ? 1 : 0
  crypto_key_id = google_kms_crypto_key.bucket[0].id
  role          = "roles/cloudkms.cryptoKeyEncrypterDecrypter"
  member        = data.google_storage_project_service_account.gcs.member
}

resource "google_storage_bucket" "this" {
  count                       = local.create_bucket ? 1 : 0
  project                     = var.project
  name                        = local.bucket
  location                    = var.region
  uniform_bucket_level_access = true
  public_access_prevention    = "enforced"
  force_destroy               = false
  labels                      = local.labels

  versioning {
    enabled = true
  }

  lifecycle_rule {
    condition {
      days_since_noncurrent_time = 30
      with_state                 = "ARCHIVED"
    }
    action {
      type = "Delete"
    }
  }

  encryption {
    default_kms_key_name = local.key_id
  }

  depends_on = [google_kms_crypto_key_iam_member.gcs_agent]
}

resource "google_storage_bucket_iam_member" "vm" {
  bucket = local.create_bucket ? google_storage_bucket.this[0].name : local.bucket
  role   = "roles/storage.objectUser"
  member = "serviceAccount:${google_service_account.vm.email}"
}

# --- the secrets ----------------------------------------------------------------------
# The Tailscale auth key is put here by you, outside this configuration.
data "google_secret_manager_secret" "tailnet_authkey" {
  project   = var.project
  secret_id = "${local.prefix}-tailnet-authkey"
}

resource "random_id" "setup_token" {
  count       = var.create_setup_token ? 1 : 0
  byte_length = 32
}

resource "google_secret_manager_secret" "setup_token" {
  count     = var.create_setup_token ? 1 : 0
  project   = var.project
  secret_id = "${local.prefix}-setup-token"
  labels    = local.labels
  replication {
    auto {}
  }
  depends_on = [google_project_service.apis]
}

resource "google_secret_manager_secret_version" "setup_token" {
  count       = var.create_setup_token ? 1 : 0
  secret      = google_secret_manager_secret.setup_token[0].id
  secret_data = random_id.setup_token[0].hex
}

# The VM reads the install's secrets and no others: a project-level grant whose
# condition limits it to secrets named <prefix>-... (the optional api-keys secret
# may not exist yet when this is applied).
resource "google_project_iam_member" "secrets" {
  project = var.project
  role    = "roles/secretmanager.secretAccessor"
  member  = "serviceAccount:${google_service_account.vm.email}"
  condition {
    title       = "loams-${var.name}-secrets"
    description = "Only the secrets of this Loams install"
    expression  = "resource.name.startsWith(\"projects/${data.google_project.this.number}/secrets/${local.prefix}-\")"
  }
}

# --- the network: a custom VPC and no firewall rule, so no ingress ------------------
resource "google_compute_network" "this" {
  project                 = var.project
  name                    = "loams-${var.name}"
  auto_create_subnetworks = false
  depends_on              = [google_project_service.apis]
}

resource "google_compute_subnetwork" "this" {
  project       = var.project
  name          = "loams-${var.name}"
  region        = var.region
  network       = google_compute_network.this.id
  ip_cidr_range = var.subnet_cidr
}

resource "google_compute_firewall" "iap_ssh" {
  count         = var.allow_iap_ssh ? 1 : 0
  project       = var.project
  name          = "loams-${var.name}-iap-ssh"
  network       = google_compute_network.this.name
  direction     = "INGRESS"
  source_ranges = ["35.235.240.0/20"]
  allow {
    protocol = "tcp"
    ports    = ["22"]
  }
}

# --- the VM -----------------------------------------------------------------------------
module "cloud_init" {
  source                 = "../shared"
  cloud                  = "gcp"
  release                = var.release
  bucket                 = local.bucket
  project                = var.project
  secret_prefix          = local.prefix
  profiles               = var.profiles
  hostname               = var.hostname
  confluent_integrations = var.confluent_integrations
}

resource "google_compute_instance" "this" {
  project      = var.project
  name         = "loams-${var.name}"
  zone         = var.zone
  machine_type = local.machine_type
  labels       = local.labels

  boot_disk {
    initialize_params {
      image = "projects/ubuntu-os-cloud/global/images/family/ubuntu-2404-lts-amd64"
      size  = var.disk_size_gb
      type  = "pd-balanced"
    }
  }

  network_interface {
    subnetwork = google_compute_subnetwork.this.id
    # An ephemeral external address, for outbound traffic only: the network has
    # no ingress rule.
    access_config {}
  }

  # cloud-platform: Secret Manager's API needs it. The IAM roles above are what
  # limit the account.
  service_account {
    email  = google_service_account.vm.email
    scopes = ["cloud-platform"]
  }

  shielded_instance_config {
    enable_secure_boot          = true
    enable_vtpm                 = true
    enable_integrity_monitoring = true
  }

  metadata = {
    user-data              = module.cloud_init.user_data
    block-project-ssh-keys = "true"
    enable-oslogin         = "TRUE"
  }

  # A new image release must not replace a running install: upgrade with `loams upgrade`.
  lifecycle {
    ignore_changes = [metadata["user-data"], boot_disk[0].initialize_params[0].image]
  }

  depends_on = [
    google_storage_bucket_iam_member.vm,
    google_project_iam_member.secrets,
    google_secret_manager_secret_version.setup_token,
  ]
}
