variable "cloud" {
  description = "aws, gcp, azure or server (a host set up over SSH: no user data)."
  type        = string
  validation {
    condition     = contains(["aws", "gcp", "azure", "server"], var.cloud)
    error_message = "cloud must be aws, gcp, azure or server."
  }
}

variable "release" {
  description = "The Loams release to install (a version, never latest). It is also the operator chart's tag."
  type        = string
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release must be a version such as 0.1.4."
  }
}

variable "hostname" {
  description = "The k3s node's tailnet name."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,62}$", var.hostname))
    error_message = "hostname is a DNS label."
  }
}

variable "tailnet_domain" {
  description = "The tailnet's DNS name, for example tail1234.ts.net. The API is https://<hostname>.<tailnet_domain>:6443."
  type        = string
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9.-]{0,251}$", var.tailnet_domain))
    error_message = "tailnet_domain is a lowercase DNS name (letters, digits, . and -)."
  }
}

variable "deployment" {
  description = "The deployment's name."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^[A-Za-z0-9._-]{0,63}$", var.deployment))
    error_message = "deployment may hold letters, digits, . _ and - (up to 63)."
  }
}

variable "home_spec" {
  description = "The home LoamsCluster's spec (the chart's home.spec). The Secrets it names (access identity and OpenFGA databases) are derived from it."
  type        = any
}

variable "operator_values" {
  description = "Other values for the operator chart; home.create and home.spec are set by the module."
  type        = any
  default     = {}
}

variable "access_db" {
  description = "local (a Postgres StatefulSet in the home) or external (access_db_dsn)."
  type        = string
  default     = "local"
  validation {
    condition     = contains(["local", "external"], var.access_db)
    error_message = "access_db is local or external."
  }
}

variable "access_db_dsn" {
  description = "The managed Postgres DSN when access_db is external."
  type        = string
  default     = null
  sensitive   = true
  validation {
    condition     = var.access_db_dsn == null || can(regex("^postgres(ql)?://", var.access_db_dsn))
    error_message = "access_db_dsn is a postgres:// or postgresql:// URL."
  }
}

variable "target_facts" {
  description = "Facts about the target (cloud, region, size, ...), written to the loams-target ConfigMap."
  type        = map(string)
  default     = {}
}

variable "chart_digest" {
  description = "Pins the operator chart by its manifest digest (sha256:<64 hex>). Empty means not pinned; the bootstrap records the digest it saw."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^(sha256:[0-9a-f]{64})?$", var.chart_digest))
    error_message = "chart_digest is sha256:<64 hex digits>, or empty."
  }
}

# Passed through to ../shared (the cloud's own settings). Not used for `server`.

variable "secret_prefix" {
  description = "The secrets' prefix: loams/<install> on AWS, loams-<install> elsewhere. Needed on a cloud."
  type        = string
  default     = ""
}

variable "bucket" {
  description = "The bucket (AWS, Google Cloud) or empty."
  type        = string
  default     = ""
}

variable "region" {
  description = "The AWS region, or empty."
  type        = string
  default     = ""
}

variable "project" {
  description = "The Google Cloud project, or empty."
  type        = string
  default     = ""
}

variable "container" {
  description = "The Blob container (Azure), or empty."
  type        = string
  default     = ""
}

variable "storage_account" {
  description = "The storage account (Azure), or empty."
  type        = string
  default     = ""
}

variable "vault" {
  description = "The Key Vault (Azure), or empty."
  type        = string
  default     = ""
}

variable "client_id" {
  description = "The user-assigned managed identity's client ID (Azure), or empty."
  type        = string
  default     = ""
}

variable "profiles" {
  description = "COMPOSE_PROFILES, comma-separated. Kept for the shared config; the k3s home does not use compose."
  type        = string
  default     = ""
}

variable "data_device" {
  description = "A disk to format ext4 and mount at /var/lib/rancher, or empty."
  type        = string
  default     = ""
}

variable "confluent_integrations" {
  description = "Confluent integrations whose keys the VM fetches from the secret store (names only)."
  type        = list(string)
  default     = []
}
