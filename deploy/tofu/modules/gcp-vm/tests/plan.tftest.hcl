# Offline tests (mock providers): `tofu test` in this directory.

mock_provider "google" {
  mock_data "google_project" {
    defaults = { number = "123456789012" }
  }
  mock_data "google_storage_project_service_account" {
    defaults = { member = "serviceAccount:service-123456789012@gs-project-accounts.iam.gserviceaccount.com" }
  }
  mock_data "google_secret_manager_secret" {
    defaults = { id = "projects/my-project/secrets/loams-tt-tailnet-authkey" }
  }
}
mock_provider "random" {}

variables {
  project = "my-project"
  name    = "tt"
}

run "defaults_never_public" {
  command = plan

  assert {
    condition     = length(google_compute_firewall.iap_ssh) == 0
    error_message = "no firewall rule at all by default: the VPC has no ingress"
  }
  assert {
    condition     = google_compute_network.this.auto_create_subnetworks == false
    error_message = "a custom network, so the default network's allow-ssh rule does not apply"
  }
  assert {
    condition     = google_storage_bucket.this[0].public_access_prevention == "enforced" && google_storage_bucket.this[0].uniform_bucket_level_access
    error_message = "the bucket prevents public access and uses uniform access"
  }
  assert {
    condition     = google_storage_bucket.this[0].versioning[0].enabled
    error_message = "versioning is on"
  }
  assert {
    condition     = google_compute_instance.this.shielded_instance_config[0].enable_secure_boot && google_compute_instance.this.shielded_instance_config[0].enable_vtpm
    error_message = "a Shielded VM"
  }
  assert {
    condition     = google_compute_instance.this.machine_type == "e2-standard-2"
    error_message = "small is e2-standard-2"
  }
  assert {
    condition     = output.bucket_url == "gs://loams-tt-my-project/loams"
    error_message = "the bucket URL"
  }
}

run "the_vm_account_has_only_its_roles_and_no_key" {
  command = plan

  assert {
    condition     = google_storage_bucket_iam_member.vm.role == "roles/storage.objectUser"
    error_message = "objectUser on the bucket"
  }
  assert {
    condition     = google_project_iam_member.secrets.role == "roles/secretmanager.secretAccessor" && can(regex("secrets/loams-tt-", google_project_iam_member.secrets.condition[0].expression))
    error_message = "secretAccessor, limited by condition to this install's secrets"
  }
  assert {
    condition     = !can(regex("GOOGLE_SERVICE_ACCOUNT|private_key|GOOGLE_APPLICATION_CREDENTIALS", module.cloud_init.user_data))
    error_message = "no service account key in the user data"
  }
  assert {
    condition     = can(regex("LOAMS_BUCKET_URL=gs://loams-tt-my-project/loams", module.cloud_init.user_data)) && can(regex("GCP_PROJECT=my-project", module.cloud_init.user_data))
    error_message = "the rendered config names the bucket and project"
  }
  assert {
    condition     = contains(google_compute_instance.this.service_account[0].scopes, "cloud-platform")
    error_message = "cloud-platform scope (Secret Manager needs it; IAM limits the account)"
  }
}

run "the_bucket_key_is_for_the_storage_agent_only" {
  command = plan

  assert {
    condition     = google_kms_crypto_key_iam_member.gcs_agent[0].role == "roles/cloudkms.cryptoKeyEncrypterDecrypter"
    error_message = "the Cloud Storage service agent gets the key role"
  }
  assert {
    condition     = google_kms_crypto_key.bucket[0].rotation_period == "7776000s"
    error_message = "the key rotates every 90 days"
  }
}

run "iap_ssh_is_opt_in_and_not_the_internet" {
  command = plan

  variables {
    allow_iap_ssh = true
  }

  assert {
    condition     = google_compute_firewall.iap_ssh[0].source_ranges == toset(["35.235.240.0/20"])
    error_message = "only IAP's range"
  }
}

run "rejects_a_latest_release" {
  command = plan

  variables {
    release = "latest"
  }

  expect_failures = [var.release]
}
