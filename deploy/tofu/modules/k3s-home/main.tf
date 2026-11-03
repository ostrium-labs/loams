# The k3s home's shared pieces: the cloud-init file (through ../shared), the two
# cluster CAs that k3s is seeded with, an admin kubeconfig, the Access database
# secret and the operator chart's values. The cloud modules (aws-k3s, gcp-k3s,
# azure-k3s) and server-ssh call this, then store ca_bundle_json and
# access_db_json in the cloud's secret store for the VM to read.

locals {
  server = var.cloud == "server"

  # The Secrets the home needs, from the spec itself (OP2-A's chart): each is a
  # Postgres DSN in `key`, for the database named after the Secret. The bootstrap
  # creates them in loams-system, and k8s-operator must create the same ones.
  identity_ref  = try(var.home_spec.base.access.identity.database.secretRef, {})
  openfga_ref   = try(var.home_spec.base.access.openfga.database.secretRef, {})
  identity_name = try(coalesce(try(local.identity_ref.name, null), "loams-access-identity-db"), "loams-access-identity-db")
  openfga_name  = try(coalesce(try(local.openfga_ref.name, null), "loams-access-openfga-db"), "loams-access-openfga-db")
  home_secrets = [
    {
      name     = local.identity_name
      key      = try(coalesce(try(local.identity_ref.key, null), "dsn"), "dsn")
      database = replace(local.identity_name, "-", "_")
    },
    {
      name     = local.openfga_name
      key      = try(coalesce(try(local.openfga_ref.key, null), "dsn"), "dsn")
      database = replace(local.openfga_name, "-", "_")
    },
  ]

  # Objects the bootstrap makes itself; a home Secret must not take their names.
  reserved_names = ["loams-access-db-auth", "loams-access-db-init", "loams-target"]

  helm_values = merge(var.operator_values, {
    home = {
      create = true
      spec   = var.home_spec
    }
  })

  api_url = "https://${var.hostname}.${var.tailnet_domain}:6443"
}

module "shared" {
  count  = local.server ? 0 : 1
  source = "../shared"

  template               = "cloud-init-k3s.yaml"
  cloud                  = var.cloud
  release                = var.release
  bucket                 = var.bucket
  region                 = var.region
  project                = var.project
  container              = var.container
  storage_account        = var.storage_account
  vault                  = var.vault
  client_id              = var.client_id
  secret_prefix          = var.secret_prefix
  profiles               = var.profiles
  hostname               = var.hostname
  data_device            = var.data_device
  confluent_integrations = var.confluent_integrations

  tailnet_domain   = var.tailnet_domain
  deployment       = var.deployment
  access_db        = var.access_db
  home_values_b64  = base64encode(jsonencode(local.helm_values))
  home_secrets_b64 = base64encode(jsonencode(local.home_secrets))
  target_facts_b64 = base64encode(jsonencode(var.target_facts))
  chart_digest     = var.chart_digest
}

# The two CAs k3s is seeded with (server-ca and client-ca): the kubeconfig's
# trust anchor, and the signer of its admin certificate.
resource "tls_private_key" "server_ca" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_private_key" "client_ca" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_private_key" "admin" {
  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_self_signed_cert" "server_ca" {
  private_key_pem       = tls_private_key.server_ca.private_key_pem
  is_ca_certificate     = true
  validity_period_hours = 87600 # 10 years
  allowed_uses          = ["cert_signing", "crl_signing", "digital_signature", "key_encipherment"]

  subject {
    common_name  = "loams-k3s-server-ca"
    organization = "Loams"
  }
}

resource "tls_self_signed_cert" "client_ca" {
  private_key_pem       = tls_private_key.client_ca.private_key_pem
  is_ca_certificate     = true
  validity_period_hours = 87600 # 10 years
  allowed_uses          = ["cert_signing", "crl_signing", "digital_signature", "key_encipherment"]

  subject {
    common_name  = "loams-k3s-client-ca"
    organization = "Loams"
  }
}

# The admin: O=system:masters is cluster-admin in k3s. Valid one year.
resource "tls_cert_request" "admin" {
  private_key_pem = tls_private_key.admin.private_key_pem

  subject {
    common_name  = "loams-admin"
    organization = "system:masters"
  }
}

resource "tls_locally_signed_cert" "admin" {
  cert_request_pem      = tls_cert_request.admin.cert_request_pem
  ca_private_key_pem    = tls_private_key.client_ca.private_key_pem
  ca_cert_pem           = tls_self_signed_cert.client_ca.cert_pem
  validity_period_hours = 8760
  allowed_uses          = ["client_auth", "digital_signature", "key_encipherment"]
}

# 32 letters and digits: the bootstrap accepts 16 to 128 of [A-Za-z0-9._~-].
resource "random_password" "access_db" {
  count   = var.access_db == "local" ? 1 : 0
  length  = 32
  special = false
}
