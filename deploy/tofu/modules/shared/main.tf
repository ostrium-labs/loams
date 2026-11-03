# Renders deploy/byoc/<cloud>/<template> (cloud-init.yaml, or cloud-init-k3s.yaml
# for the k3s home) with the module's inputs: the same placeholders and the same
# value patterns as deploy/byoc/render.sh, because the values reach a script that
# runs as root on the VM. The k3s bootstrap SOURCES its config.env before it
# validates anything, so every value written there is checked here by a strict
# pattern: no quote, $, backtick, space or newline gets through.

terraform {
  required_version = ">= 1.6"
}

variable "cloud" {
  description = "aws, gcp or azure."
  type        = string
  validation {
    condition     = contains(["aws", "gcp", "azure"], var.cloud)
    error_message = "cloud must be aws, gcp or azure."
  }
}

variable "release" {
  description = "The Loams release to install (a version, never latest)."
  type        = string
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release must be a version such as 0.1.4."
  }
}

variable "bucket" {
  description = "The bucket (AWS, Google Cloud) or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.bucket == "" || can(regex("^[a-z0-9][a-z0-9._-]{1,61}[a-z0-9]$", var.bucket))
    error_message = "bucket is not a valid bucket name."
  }
}

variable "region" {
  description = "The AWS region, or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.region == "" || can(regex("^[a-z]{2}(-[a-z]+)+-[0-9]$", var.region))
    error_message = "region is not a valid AWS region."
  }
}

variable "project" {
  description = "The Google Cloud project, or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.project == "" || can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.project))
    error_message = "project is not a valid project ID."
  }
}

variable "container" {
  description = "The Blob container (Azure), or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.container == "" || can(regex("^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$", var.container))
    error_message = "container is not a valid container name."
  }
}

variable "storage_account" {
  description = "The storage account (Azure), or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.storage_account == "" || can(regex("^[a-z0-9]{3,24}$", var.storage_account))
    error_message = "storage_account must be 3 to 24 lowercase letters and digits."
  }
}

variable "vault" {
  description = "The Key Vault (Azure), or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.vault == "" || can(regex("^[A-Za-z0-9][A-Za-z0-9-]{1,22}[A-Za-z0-9]$", var.vault))
    error_message = "vault is not a valid Key Vault name."
  }
}

variable "client_id" {
  description = "The user-assigned managed identity's client ID (Azure), or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.client_id == "" || can(regex("^[0-9a-fA-F-]{36}$", var.client_id))
    error_message = "client_id must be a GUID."
  }
}

variable "secret_prefix" {
  description = "The secrets' prefix: loams/<install> on AWS, loams-<install> elsewhere."
  type        = string
  validation {
    condition     = can(regex("^[A-Za-z0-9][A-Za-z0-9/_-]{0,100}$", var.secret_prefix))
    error_message = "secret_prefix may hold letters, digits, / _ and -."
  }
}

variable "profiles" {
  description = "COMPOSE_PROFILES, comma-separated (tikv, rustfs, ...). Choose before the first start."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^[a-z0-9,-]*$", var.profiles))
    error_message = "profiles takes lowercase profile names, comma-separated."
  }
}

variable "hostname" {
  description = "The engine's tailnet name."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z0-9][a-z0-9-]{0,62}$", var.hostname))
    error_message = "hostname is a DNS label."
  }
}

variable "data_device" {
  description = "A disk to format ext4 and mount at /var/lib/docker, or empty."
  type        = string
  default     = ""
  validation {
    condition     = var.data_device == "" || can(regex("^/dev/[A-Za-z0-9/_.:-]+$", var.data_device))
    error_message = "data_device is a /dev path."
  }
}

variable "confluent_integrations" {
  description = "Confluent integrations whose keys the VM fetches from the secret store (names only, design 80 §7.2)."
  type        = list(string)
  default     = []
  validation {
    condition     = alltrue([for i in var.confluent_integrations : can(regex("^[a-z0-9][a-z0-9-]{0,30}$", i))])
    error_message = "an integration name is lowercase letters, digits and - (up to 31)."
  }
}

variable "template" {
  description = "cloud-init.yaml (compose on one VM) or cloud-init-k3s.yaml (k3s home)."
  type        = string
  default     = "cloud-init.yaml"
  validation {
    condition     = contains(["cloud-init.yaml", "cloud-init-k3s.yaml"], var.template)
    error_message = "template must be cloud-init.yaml or cloud-init-k3s.yaml."
  }
}

# The k3s template's placeholders. Each is empty for the compose template.

variable "tailnet_domain" {
  description = "The tailnet's DNS name (k3s template), for example tail1234.ts.net."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^([a-z0-9][a-z0-9.-]{0,251})?$", var.tailnet_domain))
    error_message = "tailnet_domain is a lowercase DNS name (letters, digits, . and -)."
  }
}

variable "deployment" {
  description = "The deployment's name (k3s template)."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^[A-Za-z0-9._-]{0,63}$", var.deployment))
    error_message = "deployment may hold letters, digits, . _ and - (up to 63)."
  }
}

variable "access_db" {
  description = "local (a Postgres StatefulSet in the home) or external (a DSN), k3s template."
  type        = string
  default     = ""
  validation {
    condition     = contains(["", "local", "external"], var.access_db)
    error_message = "access_db is local or external."
  }
}

variable "home_values_b64" {
  description = "The operator chart's values as JSON, base64-encoded (k3s template)."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^[A-Za-z0-9+/=]*$", var.home_values_b64))
    error_message = "home_values_b64 must be base64."
  }
}

variable "home_secrets_b64" {
  description = "The Secrets the home names, as JSON [{name, key, database}], base64-encoded (k3s template)."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^[A-Za-z0-9+/=]*$", var.home_secrets_b64))
    error_message = "home_secrets_b64 must be base64."
  }
}

variable "target_facts_b64" {
  description = "The target's facts as a JSON object, base64-encoded (k3s template)."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^[A-Za-z0-9+/=]*$", var.target_facts_b64))
    error_message = "target_facts_b64 must be base64."
  }
}

variable "chart_digest" {
  description = "The operator chart's manifest digest to pin (sha256:<64 hex>), or empty for not pinned (k3s template)."
  type        = string
  default     = ""
  validation {
    condition     = can(regex("^(sha256:[0-9a-f]{64})?$", var.chart_digest))
    error_message = "chart_digest is sha256:<64 hex digits>, or empty."
  }
}

locals {
  raw = file("${path.module}/../../../byoc/${var.cloud}/${var.template}")
  r1  = replace(local.raw, "@@VERSION@@", var.release)
  r2  = replace(local.r1, "@@BUCKET@@", var.bucket)
  r3  = replace(local.r2, "@@REGION@@", var.region)
  r4  = replace(local.r3, "@@PROJECT@@", var.project)
  r5  = replace(local.r4, "@@CONTAINER@@", var.container)
  r6  = replace(local.r5, "@@STORAGE_ACCOUNT@@", var.storage_account)
  r7  = replace(local.r6, "@@VAULT@@", var.vault)
  r8  = replace(local.r7, "@@CLIENT_ID@@", var.client_id)
  r9  = replace(local.r8, "@@SECRET_PREFIX@@", var.secret_prefix)
  r10 = replace(local.r9, "@@PROFILES@@", var.profiles)
  r11 = replace(local.r10, "@@HOSTNAME@@", var.hostname)
  r12 = replace(local.r11, "@@DATA_DEVICE@@", var.data_device)
  r13 = replace(local.r12, "@@CONFLUENT_INTEGRATIONS@@", join(",", var.confluent_integrations))
  r14 = replace(local.r13, "@@TAILNET_DOMAIN@@", var.tailnet_domain)
  r15 = replace(local.r14, "@@DEPLOYMENT@@", var.deployment)
  r16 = replace(local.r15, "@@ACCESS_DB@@", var.access_db)
  r17 = replace(local.r16, "@@HOME_VALUES_B64@@", var.home_values_b64)
  r18 = replace(local.r17, "@@HOME_SECRETS_B64@@", var.home_secrets_b64)
  r19 = replace(local.r18, "@@TARGET_FACTS_B64@@", var.target_facts_b64)
  r20 = replace(local.r19, "@@CHART_DIGEST@@", var.chart_digest)

  user_data = local.r20
}

# A placeholder that is left over means the template and this module disagree.
check "no_unfilled_placeholders" {
  assert {
    condition     = !can(regex("(?m)^[^#\\n]*@@[A-Z_]+@@", local.user_data))
    error_message = "the cloud-init file still has an unfilled @@placeholder@@."
  }
}

output "user_data" {
  description = "The rendered cloud-init file."
  value       = local.user_data

  # The k3s bootstrap refuses to start without these; fail at plan, not on the VM.
  precondition {
    condition     = var.template != "cloud-init-k3s.yaml" || (var.tailnet_domain != "" && var.access_db != "" && var.home_values_b64 != "")
    error_message = "the k3s template needs tailnet_domain, access_db and home_values_b64."
  }
}

output "user_data_base64gz" {
  description = "The rendered file, gzipped and base64-encoded (EC2 user_data_base64 takes it; the plain file is over EC2's 16 KB limit)."
  value       = base64gzip(local.user_data)
}
