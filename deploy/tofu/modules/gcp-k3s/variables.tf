variable "project" {
  description = "The Google Cloud project that holds everything."
  type        = string
}

variable "region" {
  description = "The region of the subnet and the bucket."
  type        = string
  default     = "us-central1"
}

variable "zone" {
  description = "The VM's zone."
  type        = string
  default     = "us-central1-a"
}

variable "name" {
  description = "Names the VM, service account, network and (by default) the bucket; the deployment's name."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,20}$", var.name))
    error_message = "name starts with a letter: lowercase letters, digits and - (up to 21)."
  }
}

variable "release" {
  description = "The Loams release to install (also the operator chart's tag). The bundle's signature is checked on the VM."
  type        = string
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release is a version such as 0.1.4, never latest."
  }
}

variable "size" {
  description = "small (e2-standard-2, 2 vCPU and 8 GB) or medium (e2-standard-4, 4 vCPU and 16 GB)."
  type        = string
  default     = "small"
  validation {
    condition     = contains(["small", "medium"], var.size)
    error_message = "size is small or medium."
  }
}

variable "machine_type" {
  description = "Overrides size's machine type (x86-64 only until every image is arm64, D1384)."
  type        = string
  default     = null
}

variable "disk_size_gb" {
  description = "The pd-balanced boot disk, which holds k3s's data (/var/lib/rancher)."
  type        = number
  default     = 60
  validation {
    condition     = var.disk_size_gb >= 30 && var.disk_size_gb <= 16000
    error_message = "disk_size_gb is between 30 and 16000."
  }
}

variable "bucket" {
  description = "An existing bucket to use. null creates one (uniform access, public access prevented, versioned, CMEK) named loams-<name>-<project>."
  type        = string
  default     = null
}

variable "kms_key_id" {
  description = "An existing Cloud KMS key (projects/.../cryptoKeys/...) for the bucket. null creates a key ring and key when the module creates the bucket."
  type        = string
  default     = null
}

variable "secret_prefix" {
  description = "Secret Manager names are <prefix>-tailnet-authkey (must exist: create it with your Tailscale auth key, never in this module's variables), <prefix>-k3s-ca, <prefix>-access-db and <prefix>-setup-token. null is loams-<name>."
  type        = string
  default     = null
}

variable "create_setup_token" {
  description = "Create <prefix>-setup-token with a random value. The value is in this configuration's state; set false to create the secret yourself."
  type        = bool
  default     = true
}

variable "enable_apis" {
  description = "Enable the compute, storage, secretmanager, cloudkms and iam APIs in the project."
  type        = bool
  default     = false
}

variable "allow_iap_ssh" {
  description = "Add a firewall rule that lets Identity-Aware Proxy (35.235.240.0/20, not the internet) reach SSH, for `gcloud compute ssh --tunnel-through-iap`."
  type        = bool
  default     = false
}

variable "subnet_cidr" {
  description = "The new subnet's range."
  type        = string
  default     = "10.10.0.0/24"
}

variable "labels" {
  description = "Extra labels on every resource. loams-deploy is always the deployment's name."
  type        = map(string)
  default     = {}
}


variable "home_spec" {
  description = "The home LoamsCluster's spec (the operator chart's home.spec). Keep it small: it travels in the VM's user data."
  type        = any
}

variable "operator_values" {
  description = "Other values for the operator chart. Keep them small: they travel in the VM's user data."
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
  description = "The managed Postgres DSN when access_db is external. It is stored in the <prefix>-access-db secret and in this configuration's (encrypted) state."
  type        = string
  default     = null
  sensitive   = true
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

variable "tailnet_domain" {
  description = "Your tailnet's DNS name, for example tail1234.ts.net. The API is https://loams-<name>.<tailnet_domain>:6443."
  type        = string
}
