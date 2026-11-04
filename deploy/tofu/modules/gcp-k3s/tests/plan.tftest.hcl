# Offline tests (mock providers): `tofu test` in this directory.
# The gcp-k3s module is gcp-vm's network, bucket, key and service account around the k3s home.

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
mock_provider "tls" {
  mock_resource "tls_private_key" {
    defaults = {
      private_key_pem = "-----BEGIN EC PRIVATE KEY-----\nTESTKEY\n-----END EC PRIVATE KEY-----\n"
      public_key_pem  = "-----BEGIN PUBLIC KEY-----\nTESTPUB\n-----END PUBLIC KEY-----\n"
    }
  }
  mock_resource "tls_self_signed_cert" {
    defaults = {
      cert_pem = "-----BEGIN CERTIFICATE-----\nTESTCA\n-----END CERTIFICATE-----\n"
    }
  }
  mock_resource "tls_cert_request" {
    defaults = {
      cert_request_pem = "-----BEGIN CERTIFICATE REQUEST-----\nTESTCSR\n-----END CERTIFICATE REQUEST-----\n"
    }
  }
  mock_resource "tls_locally_signed_cert" {
    defaults = {
      cert_pem = "-----BEGIN CERTIFICATE-----\nTESTCLIENT\n-----END CERTIFICATE-----\n"
    }
  }
}

variables {
  project        = "my-project"
  name           = "tt"
  release        = "0.1.4"
  tailnet_domain = "tail1234.ts.net"
  home_spec = {
    base = {
      profile = "light"
      access = {
        identity = { database = { secretRef = { name = "loams-access-identity-db", key = "dsn" } } }
        openfga  = { database = { secretRef = { name = "loams-access-openfga-db", key = "dsn" } } }
      }
    }
  }
  operator_values = { replicas = 1 }
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
    condition     = google_compute_instance.this.shielded_instance_config[0].enable_secure_boot && google_compute_instance.this.shielded_instance_config[0].enable_vtpm && google_compute_instance.this.shielded_instance_config[0].enable_integrity_monitoring
    error_message = "a Shielded VM"
  }
  assert {
    condition     = google_compute_instance.this.machine_type == "e2-standard-2"
    error_message = "small is e2-standard-2"
  }
  assert {
    condition     = google_compute_instance.this.metadata["block-project-ssh-keys"] == "true" && google_compute_instance.this.metadata["enable-oslogin"] == "TRUE"
    error_message = "project SSH keys are blocked and OS Login is on, as in gcp-vm"
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
    error_message = "secretAccessor, limited by condition to this install's secrets (which cover the k3s CA and Access database secrets too)"
  }
  assert {
    condition     = !can(regex("GOOGLE_SERVICE_ACCOUNT|private_key|GOOGLE_APPLICATION_CREDENTIALS", module.home.user_data))
    error_message = "no service account key in the user data"
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

run "user_data_is_k3s_not_compose" {
  command = plan

  assert {
    condition     = can(regex("/usr/local/bin/loams-k3s-bootstrap", module.home.user_data))
    error_message = "the user data installs the k3s bootstrap"
  }
  assert {
    condition     = !can(regex("docker-ce", module.home.user_data))
    error_message = "the k3s home does not install Docker"
  }
  assert {
    condition     = google_compute_instance.this.metadata["user-data"] == module.home.user_data
    error_message = "the cloud-init file is the instance's user-data metadata, uncompressed"
  }
  assert {
    condition     = can(regex("LOAMS_BUCKET_URL=gs://loams-tt-my-project/loams", module.home.user_data)) && can(regex("GCP_PROJECT=my-project", module.home.user_data)) && can(regex("SECRET_PREFIX=loams-tt", module.home.user_data))
    error_message = "the rendered config names the bucket, project and secret prefix"
  }
}

run "secrets_created" {
  command = plan

  assert {
    condition     = google_secret_manager_secret.k3s_ca.secret_id == "loams-tt-k3s-ca" && google_secret_manager_secret.access_db.secret_id == "loams-tt-access-db"
    error_message = "the bootstrap reads <prefix>-k3s-ca and <prefix>-access-db"
  }
  assert {
    condition     = google_secret_manager_secret_version.k3s_ca.secret_data == module.home.ca_bundle_json && google_secret_manager_secret_version.access_db.secret_data == module.home.access_db_json
    error_message = "the secrets hold k3s-home's CA bundle and Access database JSON"
  }
  assert {
    condition     = google_secret_manager_secret.k3s_ca.labels["loams-deploy"] == "tt" && google_secret_manager_secret.access_db.labels["loams-deploy"] == "tt"
    error_message = "the secrets are labelled"
  }
}

run "target_facts_reach_the_user_data" {
  command = plan

  assert {
    condition     = can(regex("TARGET_FACTS_B64=[A-Za-z0-9+/=]+", module.home.user_data))
    error_message = "the loams-target facts are in the config"
  }
  assert {
    condition     = can(regex("TAILNET_DOMAIN=tail1234.ts.net\n", module.home.user_data))
    error_message = "the tailnet domain is in the config"
  }
  assert {
    condition     = jsondecode(base64decode(regex("TARGET_FACTS_B64=([A-Za-z0-9+/=]+)", module.home.user_data)[0])) == { deployment = "tt", target = "gcp-k3s", cloud = "gcp", region = "us-central1", bucket_url = "gs://loams-tt-my-project", workload_identity = google_service_account.vm.email }
    error_message = "the facts: deployment, target, cloud, region, bucket root URL and the VM service account"
  }
}

run "chart_digest_is_passed_through" {
  command = plan

  variables {
    chart_digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  }

  assert {
    condition     = can(regex("CHART_DIGEST=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n", module.home.user_data))
    error_message = "the chart digest pin reaches the user data"
  }
}

run "rejects_a_malformed_chart_digest" {
  command = plan

  variables {
    chart_digest = "latest"
  }

  expect_failures = [var.chart_digest]
}

run "outputs_follow_the_contract" {
  command = plan

  assert {
    condition     = output.cloud == "gcp" && output.region == "us-central1" && output.secret_prefix == "loams-tt"
    error_message = "cloud, region and secret_prefix"
  }
  assert {
    condition     = output.api_url == "https://loams-tt.tail1234.ts.net:6443"
    error_message = "the API is on the tailnet under loams-<name>"
  }
  assert {
    condition     = output.instance_name == "loams-tt" && output.service_account_email == google_service_account.vm.email
    error_message = "instance and service account"
  }
}

run "cost_items_small" {
  command = plan

  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/gce/e2-standard-2|730|hour")
    error_message = "e2-standard-2 for 730 hours"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/pd/balanced|60|gib-month")
    error_message = "60 GiB of pd-balanced"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/network/external-ipv4|730|hour")
    error_message = "one external IPv4 for 730 hours"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/kms/key-version|1|month")
    error_message = "the module's own key"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/secretmanager/secret-version|4|month")
    error_message = "four secrets: tailnet key, setup token, k3s CA, Access database"
  }
  assert {
    condition     = length(output.cost_items) == 5
    error_message = "nothing else is listed"
  }
}

run "cost_items_medium_existing_bucket_and_key" {
  command = plan

  variables {
    size         = "medium"
    disk_size_gb = 100
    bucket       = "my-existing-bucket"
    kms_key_id   = "projects/my-project/locations/us-central1/keyRings/r/cryptoKeys/k"
  }

  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/gce/e2-standard-4|730|hour")
    error_message = "e2-standard-4 for 730 hours"
  }
  assert {
    condition     = contains([for i in output.cost_items : "${i.sku}|${i.quantity}|${i.unit}"], "gcp/pd/balanced|100|gib-month")
    error_message = "100 GiB of pd-balanced"
  }
  assert {
    condition     = length([for i in output.cost_items : i if i.sku == "gcp/kms/key-version"]) == 0
    error_message = "a key the module did not create is not billed to it"
  }
}

run "every_resource_labelled" {
  command = plan

  variables {
    labels = { "loams-deploy" = "someone-else", team = "x" }
  }

  assert {
    condition     = google_compute_instance.this.labels["loams-deploy"] == "tt" && google_storage_bucket.this[0].labels["loams-deploy"] == "tt" && google_kms_crypto_key.bucket[0].labels["loams-deploy"] == "tt"
    error_message = "the instance, bucket and key carry loams-deploy=<name>, whatever the user's labels say"
  }
  assert {
    condition     = google_secret_manager_secret.k3s_ca.labels["loams-deploy"] == "tt" && google_secret_manager_secret.access_db.labels["loams-deploy"] == "tt" && google_secret_manager_secret.setup_token[0].labels["loams-deploy"] == "tt"
    error_message = "every secret is labelled"
  }
  assert {
    condition     = google_compute_instance.this.labels["team"] == "x" && google_compute_instance.this.boot_disk[0].initialize_params[0].labels["loams-deploy"] == "tt"
    error_message = "the boot disk is labelled and the user's labels are kept"
  }
}

run "existing_bucket_creates_none" {
  command = plan

  variables {
    bucket = "my-existing-bucket"
  }

  assert {
    condition     = length(google_storage_bucket.this) == 0 && length(google_kms_crypto_key.bucket) == 0
    error_message = "an existing bucket means no bucket and no key are created"
  }
  assert {
    condition     = output.bucket_url == "gs://my-existing-bucket/loams"
    error_message = "the URL names the existing bucket"
  }
}

run "medium_size" {
  command = plan

  variables {
    size = "medium"
  }

  assert {
    condition     = google_compute_instance.this.machine_type == "e2-standard-4"
    error_message = "medium is e2-standard-4"
  }
}

run "rejects_a_latest_release" {
  command = plan

  variables {
    release = "latest"
  }

  expect_failures = [var.release]
}

run "rejects_a_zone_outside_the_region" {
  command = plan

  variables {
    zone = "europe-west1-b"
  }

  expect_failures = [google_compute_instance.this]
}

run "huge_operator_values_do_not_fit_user_data" {
  command = plan

  variables {
    # Hex digests do not compress and base64 adds a third: this is far over
    # Compute Engine's 256 KB for one metadata value.
    operator_values = { blob = [for i in range(1000) : "${sha256("a${i}")}${sha256("b${i}")}${sha256("c${i}")}${sha256("d${i}")}${sha256("e${i}")}${sha256("f${i}")}"] }
  }

  expect_failures = [google_compute_instance.this]
}
