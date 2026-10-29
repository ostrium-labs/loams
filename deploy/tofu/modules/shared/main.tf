# Renders deploy/byoc/<cloud>/cloud-init.yaml with the module's inputs: the same
# placeholders and the same value patterns as deploy/byoc/render.sh, because the
# values reach a script that runs as root on the VM.

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
    error_message = "release must be a version such as 0.1.3."
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

locals {
  raw = file("${path.module}/../../../byoc/${var.cloud}/cloud-init.yaml")
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

  user_data = local.r13
}

# A placeholder that is left over means the template and this module disagree.
check "no_unfilled_placeholders" {
  assert {
    condition     = !can(regex("(?m)^[^#\\n]*@@[A-Z_]+@@", local.user_data))
    error_message = "cloud-init.yaml still has an unfilled @@placeholder@@."
  }
}

output "user_data" {
  description = "The rendered cloud-init file."
  value       = local.user_data
}

output "user_data_base64gz" {
  description = "The rendered file, gzipped and base64-encoded (EC2 user_data_base64 takes it; the plain file is over EC2's 16 KB limit)."
  value       = base64gzip(local.user_data)
}
