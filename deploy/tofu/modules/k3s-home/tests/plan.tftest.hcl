# Offline tests (mock providers): `tofu test` in the module directory.
# The k3s-home module renders cloud-init-k3s.yaml through ../shared, makes the
# cluster CAs, the admin kubeconfig and the Access database secret.

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
mock_provider "random" {
  mock_resource "random_password" {
    defaults = {
      result = "Mock0Password1Alphanumeric2Only3x"
    }
  }
}

variables {
  cloud           = "aws"
  release         = "0.1.4"
  hostname        = "loams-t"
  tailnet_domain  = "tail1234.ts.net"
  deployment      = "t"
  bucket          = "loams-t-123456789012"
  region          = "us-east-1"
  secret_prefix   = "loams/t"
  access_db       = "local"
  target_facts    = { cloud = "aws", region = "us-east-1", instance_type = "t3.large" }
  operator_values = { image = { tag = "0.1.4" }, replicas = 1 }
  home_spec = {
    base = {
      profile = "light"
      access = {
        identity = { database = { secretRef = { name = "loams-access-identity-db", key = "dsn" } } }
        openfga  = { database = { secretRef = { name = "loams-access-openfga-db", key = "dsn" } } }
      }
      sql   = { engine = "redb" }
      house = { enabled = true }
    }
  }
}

run "kubeconfig_points_at_tailnet" {
  command = apply

  assert {
    condition     = output.api_url == "https://loams-t.tail1234.ts.net:6443"
    error_message = "the API is the tailnet name on 6443"
  }
  assert {
    condition     = yamldecode(output.kubeconfig).clusters[0].cluster.server == "https://loams-t.tail1234.ts.net:6443"
    error_message = "the kubeconfig names the tailnet URL"
  }
  assert {
    condition     = yamldecode(output.kubeconfig).clusters[0].cluster["certificate-authority-data"] == base64encode(tls_self_signed_cert.server_ca.cert_pem)
    error_message = "the kubeconfig trusts the server CA"
  }
  assert {
    condition     = yamldecode(output.kubeconfig).users[0].user["client-certificate-data"] == base64encode(tls_locally_signed_cert.admin.cert_pem) && yamldecode(output.kubeconfig).users[0].user["client-key-data"] == base64encode(tls_private_key.admin.private_key_pem)
    error_message = "the kubeconfig carries the admin client certificate and key"
  }
  assert {
    condition     = yamldecode(output.kubeconfig)["current-context"] == yamldecode(output.kubeconfig).contexts[0].name
    error_message = "the kubeconfig has a current context"
  }
}

run "admin_cert_is_system_masters_ecdsa_one_year" {
  command = plan

  assert {
    condition     = tls_cert_request.admin.subject[0].common_name == "loams-admin" && tls_cert_request.admin.subject[0].organization == "system:masters"
    error_message = "the admin is CN=loams-admin, O=system:masters"
  }
  assert {
    condition     = tls_locally_signed_cert.admin.validity_period_hours == 8760 && contains(tls_locally_signed_cert.admin.allowed_uses, "client_auth")
    error_message = "a client certificate valid one year"
  }
  assert {
    condition     = alltrue([for k in [tls_private_key.server_ca, tls_private_key.client_ca, tls_private_key.admin] : k.algorithm == "ECDSA" && k.ecdsa_curve == "P256"])
    error_message = "every key is ECDSA P-256"
  }
  assert {
    condition     = alltrue([for c in [tls_self_signed_cert.server_ca, tls_self_signed_cert.client_ca] : c.is_ca_certificate && c.validity_period_hours == 87600])
    error_message = "both CAs are CAs valid 10 years"
  }
  assert {
    condition     = tls_locally_signed_cert.admin.is_ca_certificate == false && tls_locally_signed_cert.admin.ca_private_key_pem == tls_private_key.client_ca.private_key_pem
    error_message = "the admin certificate is signed by the client CA"
  }
}

run "ca_bundle_has_the_four_pems" {
  command = apply

  assert {
    condition     = jsondecode(output.ca_bundle_json).server_ca_crt == tls_self_signed_cert.server_ca.cert_pem && jsondecode(output.ca_bundle_json).server_ca_key == tls_private_key.server_ca.private_key_pem && jsondecode(output.ca_bundle_json).client_ca_crt == tls_self_signed_cert.client_ca.cert_pem && jsondecode(output.ca_bundle_json).client_ca_key == tls_private_key.client_ca.private_key_pem
    error_message = "the k3s-ca secret is {server_ca_crt, server_ca_key, client_ca_crt, client_ca_key}"
  }
}

run "helm_values_set_home_create" {
  command = apply

  assert {
    condition     = jsondecode(output.helm_values_json).home.create == true
    error_message = "home.create is true"
  }
  assert {
    condition     = jsondecode(output.helm_values_json).image.tag == "0.1.4" && jsondecode(output.helm_values_json).replicas == 1
    error_message = "the operator values are kept"
  }
  assert {
    condition     = jsondecode(output.helm_values_json).home.spec.base.profile == "light"
    error_message = "the home spec is under home.spec"
  }
  assert {
    condition     = output.helm_values_json == jsonencode(merge(var.operator_values, { home = { create = true, spec = var.home_spec } }))
    error_message = "the values are the merge the brief gives, minified"
  }
}

run "helm_values_reach_the_user_data_as_base64" {
  command = apply

  assert {
    condition     = can(regex("HOME_VALUES_B64=${base64encode(output.helm_values_json)}\n", output.user_data))
    error_message = "HOME_VALUES_B64 is the base64 of helm_values_json"
  }
  assert {
    condition     = can(regex("TARGET_FACTS_B64=${base64encode(jsonencode(var.target_facts))}\n", output.user_data))
    error_message = "TARGET_FACTS_B64 is the base64 of the facts"
  }
}

run "access_db_local_has_password_external_has_dsn" {
  command = apply

  assert {
    condition     = jsondecode(output.access_db_json).password == random_password.access_db[0].result && !contains(keys(jsondecode(output.access_db_json)), "dsn")
    error_message = "local: {password}"
  }
  assert {
    condition     = random_password.access_db[0].length == 32 && random_password.access_db[0].special == false
    error_message = "the password is 32 characters, no specials"
  }
  assert {
    condition     = can(regex("ACCESS_DB=local\n", output.user_data))
    error_message = "the VM is told the database is local"
  }
}

run "access_db_external_has_dsn" {
  command = apply

  variables {
    access_db     = "external"
    access_db_dsn = "postgres://u:p@db.example.com:5432/loams?sslmode=require"
  }

  assert {
    condition     = jsondecode(output.access_db_json).dsn == "postgres://u:p@db.example.com:5432/loams?sslmode=require" && !contains(keys(jsondecode(output.access_db_json)), "password")
    error_message = "external: {dsn}"
  }
  assert {
    condition     = length(random_password.access_db) == 0
    error_message = "no password is made for an external database"
  }
  assert {
    condition     = can(regex("ACCESS_DB=external\n", output.user_data)) && !can(regex("db.example.com", output.user_data))
    error_message = "the DSN never reaches user data"
  }
}

run "access_db_external_needs_a_dsn" {
  command = plan

  variables {
    access_db = "external"
  }

  expect_failures = [output.access_db_json]
}

run "server_has_no_user_data" {
  command = apply

  variables {
    cloud = "server"
  }

  assert {
    condition     = output.user_data == null && output.user_data_base64gz == null
    error_message = "a server is set up over SSH: no user data"
  }
  assert {
    condition     = output.api_url == "https://loams-t.tail1234.ts.net:6443" && jsondecode(output.helm_values_json).home.create == true
    error_message = "the rest of the outputs still exist"
  }
}

run "no_unfilled_placeholders" {
  command = apply

  assert {
    condition     = !can(regex("(?m)^[^#\\n]*@@[A-Z_]+@@", output.user_data))
    error_message = "the k3s cloud-init has an unfilled @@placeholder@@"
  }
  assert {
    condition     = can(regex("TAILNET_DOMAIN=tail1234.ts.net\n", output.user_data)) && can(regex("DEPLOYMENT=t\n", output.user_data)) && can(regex("TS_HOSTNAME=loams-t\n", output.user_data)) && can(regex("LOAMS_VERSION=0.1.4\n", output.user_data))
    error_message = "the k3s settings are filled in"
  }
  assert {
    condition     = can(regex("/usr/local/bin/loams-k3s-bootstrap", output.user_data))
    error_message = "this is the k3s file, not the compose one"
  }
}

run "no_unfilled_placeholders_gcp" {
  command = apply

  variables {
    cloud         = "gcp"
    project       = "my-project-1234"
    bucket        = "loams-t-bucket"
    secret_prefix = "loams-t"
  }

  assert {
    condition     = !can(regex("(?m)^[^#\\n]*@@[A-Z_]+@@", output.user_data)) && can(regex("/usr/local/bin/loams-k3s-bootstrap", output.user_data))
    error_message = "the GCP k3s file is filled"
  }
}

run "no_unfilled_placeholders_azure" {
  command = apply

  variables {
    cloud           = "azure"
    container       = "loams"
    storage_account = "loamst1234"
    vault           = "loams-t-kv"
    client_id       = "00000000-1111-2222-3333-444444444444"
    secret_prefix   = "loams-t"
  }

  assert {
    condition     = !can(regex("(?m)^[^#\\n]*@@[A-Z_]+@@", output.user_data)) && can(regex("/usr/local/bin/loams-k3s-bootstrap", output.user_data))
    error_message = "the Azure k3s file is filled"
  }
}

run "chart_digest_reaches_user_data" {
  command = apply

  variables {
    chart_digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
  }

  assert {
    condition     = can(regex("CHART_DIGEST=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n", output.user_data))
    error_message = "a pinned chart digest is written to the VM's config"
  }
}

run "chart_digest_default_is_empty" {
  command = apply

  assert {
    condition     = can(regex("CHART_DIGEST=\n", output.user_data))
    error_message = "no digest: empty, which the bootstrap reads as not pinned"
  }
}

run "rejects_a_malformed_chart_digest" {
  command = plan

  variables {
    chart_digest = "sha256:XYZ"
  }

  expect_failures = [var.chart_digest]
}

run "home_secrets_follow_the_home_spec" {
  command = apply

  assert {
    condition = output.home_secrets == [
      { name = "loams-access-identity-db", key = "dsn", database = "loams_access_identity_db" },
      { name = "loams-access-openfga-db", key = "dsn", database = "loams_access_openfga_db" },
    ]
    error_message = "one entry per database secretRef in the spec: {name, key, database}"
  }
  assert {
    condition     = can(regex("HOME_SECRETS_B64=${base64encode(jsonencode(output.home_secrets))}\n", output.user_data))
    error_message = "HOME_SECRETS_B64 is the base64 of the same list"
  }
  assert {
    condition     = jsondecode(base64decode(regex("HOME_SECRETS_B64=([A-Za-z0-9+/=]+)\n", output.user_data)[0]))[0].database == "loams_access_identity_db"
    error_message = "the VM reads the list back as JSON"
  }
}

run "home_secrets_defaults_when_the_spec_is_silent" {
  command = apply

  variables {
    home_spec = { base = { profile = "light" } }
  }

  assert {
    condition = output.home_secrets == [
      { name = "loams-access-identity-db", key = "dsn", database = "loams_access_identity_db" },
      { name = "loams-access-openfga-db", key = "dsn", database = "loams_access_openfga_db" },
    ]
    error_message = "absent paths default to the documented names and key dsn"
  }
}

run "home_secrets_honour_custom_names_and_keys" {
  command = apply

  variables {
    home_spec = {
      base = {
        access = {
          identity = { database = { secretRef = { name = "my-identity", key = "url" } } }
          openfga  = { database = { secretRef = { name = "my-fga" } } }
        }
      }
    }
  }

  assert {
    condition = output.home_secrets == [
      { name = "my-identity", key = "url", database = "my_identity" },
      { name = "my-fga", key = "dsn", database = "my_fga" },
    ]
    error_message = "a custom name and key are used; the key defaults to dsn; the database is the name with - as _"
  }
}

run "home_secrets_reject_a_bad_name" {
  command = plan

  variables {
    home_spec = { base = { access = { identity = { database = { secretRef = { name = "Bad_Name" } } } } } }
  }

  expect_failures = [output.home_secrets]
}

run "home_secrets_reject_a_dotted_name" {
  command = plan

  variables {
    home_spec = { base = { access = { identity = { database = { secretRef = { name = "a.b" } } } } } }
  }

  expect_failures = [output.home_secrets]
}

run "home_secrets_reject_the_bootstraps_own_secret" {
  command = plan

  variables {
    home_spec = { base = { access = { identity = { database = { secretRef = { name = "loams-access-db-auth" } } } } } }
  }

  expect_failures = [output.home_secrets]
}

run "home_secrets_reject_a_duplicate_name" {
  command = plan

  variables {
    home_spec = {
      base = {
        access = {
          identity = { database = { secretRef = { name = "same" } } }
          openfga  = { database = { secretRef = { name = "same" } } }
        }
      }
    }
  }

  expect_failures = [output.home_secrets]
}

run "home_secrets_reject_a_key_with_a_space" {
  command = plan

  variables {
    home_spec = { base = { access = { identity = { database = { secretRef = { name = "ok", key = "a b" } } } } } }
  }

  expect_failures = [output.home_secrets]
}

run "user_data_gzip_fits_ec2_16KB" {
  command = apply

  # EC2 takes 16,384 bytes of user data; base64 of that is at most 21,848 characters.
  assert {
    condition     = length(output.user_data_base64gz) <= 21848
    error_message = "the gzipped k3s user data is over EC2's 16 KB limit"
  }
}

run "rejects_unsafe_values" {
  command = plan

  variables {
    tailnet_domain = "x.ts.net\"; rm -rf /"
  }

  expect_failures = [var.tailnet_domain]
}

run "rejects_an_unsafe_deployment" {
  command = plan

  variables {
    deployment = "a b"
  }

  expect_failures = [var.deployment]
}

run "rejects_an_unknown_cloud" {
  command = plan

  variables {
    cloud = "digitalocean"
  }

  expect_failures = [var.cloud]
}

run "rejects_a_latest_release" {
  command = plan

  variables {
    release = "latest"
  }

  expect_failures = [var.release]
}
