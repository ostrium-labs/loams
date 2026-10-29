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
  description = "Names the VM, service account, network and (by default) the bucket."
  type        = string
  default     = "loams"
  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{1,20}$", var.name))
    error_message = "name starts with a letter: lowercase letters, digits and - (up to 21)."
  }
}

variable "release" {
  description = "The Loams release to install. The bundle's signature is checked on the VM."
  type        = string
  default     = "0.1.3"
  validation {
    condition     = can(regex("^[0-9]+\\.[0-9]+\\.[0-9]+([-.][0-9A-Za-z.]+)?$", var.release))
    error_message = "release is a version such as 0.1.3, never latest."
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
  description = "The pd-balanced boot disk, which holds Docker's volumes (the stores)."
  type        = number
  default     = 60
  validation {
    condition     = var.disk_size_gb >= 30 && var.disk_size_gb <= 16000
    error_message = "disk_size_gb is between 30 and 16000."
  }
}

variable "profiles" {
  description = "COMPOSE_PROFILES (tikv, rustfs, ...). Choose before the first start: the engine refuses to switch stores on existing data."
  type        = string
  default     = ""
}

variable "hostname" {
  description = "The engine's tailnet name (https://<hostname>.<tailnet>.ts.net)."
  type        = string
  default     = "loams"
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
  description = "Secret Manager names are <prefix>-tailnet-authkey (must exist: create it with your Tailscale auth key, never in this module's variables), <prefix>-setup-token and optionally <prefix>-api-keys. null is loams-<name>."
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
  description = "Extra labels."
  type        = map(string)
  default     = {}
}

variable "confluent_integrations" {
  description = "Confluent integrations whose keys the VM fetches from the secret store into the connect role's secret files (names only; design 80 §7.2). Create the keys with `loams byoc confluent key create`."
  type        = list(string)
  default     = []
}
