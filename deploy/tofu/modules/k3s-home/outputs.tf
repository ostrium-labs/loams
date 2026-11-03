output "user_data" {
  description = "The rendered k3s cloud-init file (null for server)."
  value       = one(module.shared[*].user_data)
}

output "user_data_base64gz" {
  description = "The rendered file, gzipped and base64-encoded, for EC2's user_data_base64 (null for server)."
  value       = one(module.shared[*].user_data_base64gz)
}

output "ca_bundle_json" {
  description = "The k3s-ca secret: {server_ca_crt, server_ca_key, client_ca_crt, client_ca_key}."
  value = jsonencode({
    server_ca_crt = tls_self_signed_cert.server_ca.cert_pem
    server_ca_key = tls_private_key.server_ca.private_key_pem
    client_ca_crt = tls_self_signed_cert.client_ca.cert_pem
    client_ca_key = tls_private_key.client_ca.private_key_pem
  })
  sensitive = true
}

output "access_db_json" {
  description = "The access-db secret: {password} for a local database, {dsn} for an external one."
  value       = var.access_db == "local" ? jsonencode({ password = random_password.access_db[0].result }) : jsonencode({ dsn = var.access_db_dsn })
  sensitive   = true

  precondition {
    condition     = var.access_db != "external" || var.access_db_dsn != null
    error_message = "access_db is external, so access_db_dsn is needed."
  }
}

output "helm_values_json" {
  description = "The operator chart's values: operator_values plus home.create=true and home.spec."
  value       = jsonencode(local.helm_values)
}

output "home_secrets" {
  description = "The Secrets (in loams-system) the home names: [{name, key, database}]. k8s-operator creates the same ones."
  value       = local.home_secrets

  precondition {
    condition     = alltrue([for s in local.home_secrets : can(regex("^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$", s.name))])
    error_message = "a home Secret name must be a DNS-1123 label (lowercase letters, digits and -, up to 63)."
  }
  precondition {
    condition     = alltrue([for s in local.home_secrets : can(regex("^[-._a-zA-Z0-9]{1,253}$", s.key))])
    error_message = "a home Secret key may hold letters, digits, . _ and - only."
  }
  precondition {
    condition     = alltrue([for s in local.home_secrets : can(regex("^[a-z_][a-z0-9_]{0,62}$", s.database))])
    error_message = "a home Secret name must give a plain database name (the name with - as _): lowercase letters, digits and _, not starting with a digit."
  }
  precondition {
    condition     = length(distinct([for s in local.home_secrets : s.name])) == length(local.home_secrets)
    error_message = "the home's Secret names must differ."
  }
  precondition {
    condition     = length(setintersection(toset([for s in local.home_secrets : s.name]), toset(local.reserved_names))) == 0
    error_message = "a home Secret name collides with an object the bootstrap creates (loams-access-db-auth, loams-access-db-init, loams-target)."
  }
}

output "api_url" {
  description = "The k3s API on the tailnet."
  value       = local.api_url
}

output "kubeconfig" {
  description = "An admin kubeconfig (client certificate, O=system:masters; valid one year). It works from a machine on the tailnet."
  sensitive   = true
  value = yamlencode({
    apiVersion = "v1"
    kind       = "Config"
    clusters = [{
      name = "loams-${var.hostname}"
      cluster = {
        server                       = local.api_url
        "certificate-authority-data" = base64encode(tls_self_signed_cert.server_ca.cert_pem)
      }
    }]
    users = [{
      name = "loams-admin@${var.hostname}"
      user = {
        "client-certificate-data" = base64encode(tls_locally_signed_cert.admin.cert_pem)
        "client-key-data"         = base64encode(tls_private_key.admin.private_key_pem)
      }
    }]
    contexts = [{
      name = "loams-${var.hostname}"
      context = {
        cluster = "loams-${var.hostname}"
        user    = "loams-admin@${var.hostname}"
      }
    }]
    "current-context" = "loams-${var.hostname}"
  })
}
